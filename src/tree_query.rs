//! Dynamically assembles the single-statement tree query a nested repo's
//! generated read fns execute at runtime: a tagged `UNION ALL` over one CTE
//! per tree node, so a parent and all of its nested children (recursively)
//! come back in one statement instead of one per level.

use std::collections::HashMap;

use crate::{
    db,
    events::{GenericEvent, HydrationRow},
};

/// Static description of one node (repo) in a nested tree, used to build the query.
#[derive(Debug, Clone)]
pub struct TreeSpec {
    pub table_name: &'static str,
    pub events_table_name: &'static str,
    pub parent_column: Option<&'static str>,
    pub soft_delete: bool,
    pub forgettable_table_name: Option<&'static str>,
    pub event_context: bool,
    /// `Some(<tbl>_snapshots)` when this node's repo enables `snapshot`.
    pub snapshot_table_name: Option<&'static str>,
    /// This node's `<Entity as EsEntity>::Snapshot as EsSnapshot>::FINGERPRINT`.
    /// Meaningless when `snapshot_table_name` is `None`.
    pub snapshot_fingerprint: i64,
    pub children: Vec<TreeSpec>,
}

/// The root user query plus the bits needed to fold it into the tree query.
pub struct TreeQuerySource<Id> {
    pub user_sql: &'static str,
    pub order_by_cols: &'static [&'static str],
    pub n_user_args: usize,
    pub decode: fn(&db::Row) -> Result<HydrationRow<Id>, sqlx::Error>,
    /// The root's fingerprint bind expression, as passed to `es_query!`.
    /// `NO_SNAPSHOT_FINGERPRINT` signals a full-history load: every node in
    /// the tree (not just the root) binds `NO_SNAPSHOT_FINGERPRINT`, forcing
    /// every snapshot join in the tree to match nothing.
    pub snapshot_fingerprint: i64,
}

pub fn decode_tag(row: &db::Row) -> Result<i32, sqlx::Error> {
    sqlx::Row::try_get(row, "tag")
}

/// Decodes a row from a tree branch with no snapshot columns (the branch's
/// own node has no snapshot table, and no sibling forced padding). Identical
/// to what every tree branch decoded before snapshotting existed.
pub fn decode_tagged_row<Id>(row: &db::Row) -> Result<HydrationRow<Id>, sqlx::Error>
where
    Id: for<'r> sqlx::Decode<'r, db::Db> + sqlx::Type<db::Db>,
{
    use sqlx::Row;
    Ok(GenericEvent {
        entity_id: row.try_get("entity_id")?,
        sequence: row.try_get("sequence")?,
        event: row.try_get("event")?,
        context: row.try_get("context")?,
        recorded_at: row.try_get("recorded_at")?,
        forgettable_payload: row.try_get("forgettable_payload")?,
    }
    .into())
}

/// Decodes a row from a snapshot-enabled node's own branch: `event` and
/// `recorded_at` may be `NULL` (the lone snapshot-only row when the tail is
/// empty), and the five `snapshot*` columns are present.
pub fn decode_tagged_snapshot_row<Id>(row: &db::Row) -> Result<HydrationRow<Id>, sqlx::Error>
where
    Id: for<'r> sqlx::Decode<'r, db::Db> + sqlx::Type<db::Db>,
{
    use sqlx::Row;
    Ok(HydrationRow {
        entity_id: row.try_get("entity_id")?,
        sequence: row.try_get("sequence")?,
        event: row.try_get("event")?,
        context: row.try_get("context")?,
        recorded_at: row.try_get("recorded_at")?,
        forgettable_payload: row.try_get("forgettable_payload")?,
        snapshot: row.try_get("snapshot")?,
        snapshot_sequence: row.try_get("snapshot_sequence")?,
        snapshot_recorded_at: row.try_get("snapshot_recorded_at")?,
        snapshot_first_recorded_at: row.try_get("snapshot_first_recorded_at")?,
        snapshot_forgettable_payload: row.try_get("snapshot_forgettable_payload")?,
    })
}

/// `true` if this node or any descendant enables `snapshot`. Determines
/// whether every branch in the tree's `UNION ALL` needs the same (wider)
/// column list — a tree with no snapshot node anywhere emits exactly
/// today's SQL, byte for byte.
pub fn tree_has_snapshot(spec: &TreeSpec) -> bool {
    spec.snapshot_table_name.is_some() || spec.children.iter().any(tree_has_snapshot)
}

/// Walks the tree in the same DFS order `build_tree_query` assigns
/// fingerprint bind-parameter indices in (root, then children depth-first),
/// returning the fingerprint of every node that has a snapshot table.
pub fn snapshot_fingerprints(spec: &TreeSpec) -> Vec<i64> {
    let mut out = Vec::new();
    collect_snapshot_fingerprints(spec, &mut out);
    out
}

fn collect_snapshot_fingerprints(spec: &TreeSpec, out: &mut Vec<i64>) {
    if spec.snapshot_table_name.is_some() {
        out.push(spec.snapshot_fingerprint);
    }
    for child in &spec.children {
        collect_snapshot_fingerprints(child, out);
    }
}

pub fn partition_by_tag(rows: Vec<db::Row>) -> Result<HashMap<i32, Vec<db::Row>>, sqlx::Error> {
    let mut by_tag: HashMap<i32, Vec<db::Row>> = HashMap::new();
    for row in rows {
        let tag = decode_tag(&row)?;
        by_tag.entry(tag).or_default().push(row);
    }
    Ok(by_tag)
}

#[allow(clippy::too_many_arguments)]
fn branch_sql(
    tag: i32,
    cte_name: &str,
    node: &TreeSpec,
    is_root: bool,
    ctx_param_idx: usize,
    fp_idx: Option<usize>,
    uniform_snapshot_columns: bool,
) -> String {
    let context_expr = if is_root {
        format!("CASE WHEN ${ctx_param_idx} THEN e.context ELSE NULL::jsonb END")
    } else if node.event_context {
        "e.context".to_string()
    } else {
        "NULL::jsonb".to_string()
    };
    let (payload_expr, forgettable_join) = match node.forgettable_table_name {
        Some(tbl) => (
            "p.payload".to_string(),
            format!(" LEFT JOIN {tbl} p ON e.id = p.entity_id AND e.sequence = p.sequence"),
        ),
        None => ("NULL::jsonb".to_string(), String::new()),
    };
    let ord_expr = if is_root { "i.__ord" } else { "NULL::BIGINT" };
    let events_table = node.events_table_name;

    if let Some(snap_tbl) = node.snapshot_table_name {
        let fp_idx = fp_idx.expect("a snapshot node always has a fingerprint bind index");
        let (snapshot_payload_expr, snapshot_payload_join) =
            match (node.forgettable_table_name, node.snapshot_table_name) {
                (Some(fp_tbl), Some(_)) => (
                    "sp.payload".to_string(),
                    format!(" LEFT JOIN {fp_tbl} sp ON sp.entity_id = i.id AND sp.sequence = 0"),
                ),
                _ => ("NULL::jsonb".to_string(), String::new()),
            };
        format!(
            "SELECT {tag} AS tag, i.id AS entity_id, COALESCE(e.sequence, s.sequence) AS sequence, \
             e.event, {context_expr} AS context, e.recorded_at, {payload_expr} AS forgettable_payload, \
             CASE WHEN e.sequence IS NULL OR e.sequence = s.sequence + 1 THEN s.snapshot END AS snapshot, \
             s.sequence AS snapshot_sequence, s.recorded_at AS snapshot_recorded_at, \
             s.first_recorded_at AS snapshot_first_recorded_at, \
             {snapshot_payload_expr} AS snapshot_forgettable_payload, {ord_expr} AS __ord \
             FROM {cte_name} i \
             LEFT JOIN {snap_tbl} s ON s.id = i.id AND s.fingerprint = ${fp_idx} \
             LEFT JOIN {events_table} e ON e.id = i.id AND e.sequence > COALESCE(s.sequence, 0)\
             {forgettable_join}{snapshot_payload_join}"
        )
    } else if uniform_snapshot_columns {
        format!(
            "SELECT {tag} AS tag, i.id AS entity_id, e.sequence, e.event, {context_expr} AS context, \
             e.recorded_at, {payload_expr} AS forgettable_payload, \
             NULL::jsonb AS snapshot, NULL::INT AS snapshot_sequence, \
             NULL::TIMESTAMPTZ AS snapshot_recorded_at, NULL::TIMESTAMPTZ AS snapshot_first_recorded_at, \
             NULL::jsonb AS snapshot_forgettable_payload, {ord_expr} AS __ord \
             FROM {cte_name} i JOIN {events_table} e ON i.id = e.id{forgettable_join}"
        )
    } else {
        format!(
            "SELECT {tag} AS tag, i.id AS entity_id, e.sequence, e.event, {context_expr} AS context, \
             e.recorded_at, {payload_expr} AS forgettable_payload, {ord_expr} AS __ord \
             FROM {cte_name} i JOIN {events_table} e ON i.id = e.id{forgettable_join}"
        )
    }
}

pub fn build_tree_query(
    user_sql: &str,
    order_by_cols: &[&str],
    spec: &TreeSpec,
    include_deleted: bool,
    ctx_param_idx: usize,
) -> String {
    let order_clause = if order_by_cols.is_empty() {
        "id".to_string()
    } else {
        order_by_cols.join(", ")
    };

    let mut ctes = vec![
        format!("__user AS ({user_sql})"),
        format!(
            "entities AS (SELECT *, ROW_NUMBER() OVER (ORDER BY {order_clause}) AS __ord FROM __user)"
        ),
    ];

    let uniform = tree_has_snapshot(spec);
    let mut fp_cursor = ctx_param_idx + 1;
    let root_fp_idx = take_fp_idx(spec, &mut fp_cursor);
    let mut branches = vec![branch_sql(
        0,
        "entities",
        spec,
        true,
        ctx_param_idx,
        root_fp_idx,
        uniform,
    )];
    let mut cursor: i32 = 1;

    walk_children(
        spec,
        "entities",
        include_deleted,
        ctx_param_idx,
        &mut fp_cursor,
        &mut cursor,
        &mut ctes,
        &mut branches,
        uniform,
    );

    format!(
        "WITH {} {} ORDER BY tag, __ord, entity_id, sequence",
        ctes.join(", "),
        branches.join(" UNION ALL "),
    )
}

fn take_fp_idx(node: &TreeSpec, fp_cursor: &mut usize) -> Option<usize> {
    if node.snapshot_table_name.is_some() {
        let idx = *fp_cursor;
        *fp_cursor += 1;
        Some(idx)
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn walk_children(
    node: &TreeSpec,
    parent_cte: &str,
    include_deleted: bool,
    ctx_param_idx: usize,
    fp_cursor: &mut usize,
    cursor: &mut i32,
    ctes: &mut Vec<String>,
    branches: &mut Vec<String>,
    uniform: bool,
) {
    for child in &node.children {
        let tag = *cursor;
        *cursor += 1;
        let cte_name = format!("n{tag}");
        let parent_col = child
            .parent_column
            .expect("non-root tree node must declare a parent column");
        let deleted_cond = if child.soft_delete && !include_deleted {
            " AND deleted = FALSE"
        } else {
            ""
        };
        ctes.push(format!(
            "{cte_name} AS (SELECT id FROM {} WHERE {parent_col} IN (SELECT id FROM {parent_cte}){deleted_cond})",
            child.table_name
        ));
        let fp_idx = take_fp_idx(child, fp_cursor);
        branches.push(branch_sql(
            tag,
            &cte_name,
            child,
            false,
            ctx_param_idx,
            fp_idx,
            uniform,
        ));
        walk_children(
            child,
            &cte_name,
            include_deleted,
            ctx_param_idx,
            fp_cursor,
            cursor,
            ctes,
            branches,
            uniform,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::NO_SNAPSHOT_FINGERPRINT;

    fn leaf(table: &'static str, events: &'static str, parent_col: &'static str) -> TreeSpec {
        TreeSpec {
            table_name: table,
            events_table_name: events,
            parent_column: Some(parent_col),
            soft_delete: false,
            forgettable_table_name: None,
            event_context: false,
            snapshot_table_name: None,
            snapshot_fingerprint: NO_SNAPSHOT_FINGERPRINT,
            children: Vec::new(),
        }
    }

    fn snapshotted(spec: TreeSpec, tbl: &'static str, fingerprint: i64) -> TreeSpec {
        TreeSpec {
            snapshot_table_name: Some(tbl),
            snapshot_fingerprint: fingerprint,
            ..spec
        }
    }

    fn root(table: &'static str, events: &'static str, children: Vec<TreeSpec>) -> TreeSpec {
        TreeSpec {
            table_name: table,
            events_table_name: events,
            parent_column: None,
            soft_delete: false,
            forgettable_table_name: None,
            event_context: false,
            snapshot_table_name: None,
            snapshot_fingerprint: NO_SNAPSHOT_FINGERPRINT,
            children,
        }
    }

    #[test]
    fn leaf_only_root() {
        let spec = root("subscriptions", "subscription_events", Vec::new());
        let sql = build_tree_query(
            "SELECT id FROM subscriptions WHERE id = $1",
            &[],
            &spec,
            false,
            2,
        );
        assert_eq!(
            sql,
            "WITH __user AS (SELECT id FROM subscriptions WHERE id = $1), \
             entities AS (SELECT *, ROW_NUMBER() OVER (ORDER BY id) AS __ord FROM __user) \
             SELECT 0 AS tag, i.id AS entity_id, e.sequence, e.event, \
             CASE WHEN $2 THEN e.context ELSE NULL::jsonb END AS context, e.recorded_at, \
             NULL::jsonb AS forgettable_payload, i.__ord AS __ord \
             FROM entities i JOIN subscription_events e ON i.id = e.id \
             ORDER BY tag, __ord, entity_id, sequence"
        );
    }

    #[test]
    fn one_child() {
        let child = TreeSpec {
            soft_delete: true,
            ..leaf(
                "billing_periods",
                "billing_period_events",
                "subscription_id",
            )
        };
        let spec = root("subscriptions", "subscription_events", vec![child]);
        let sql = build_tree_query(
            "SELECT id FROM subscriptions WHERE id = $1",
            &[],
            &spec,
            false,
            2,
        );
        assert!(sql.contains(
            "n1 AS (SELECT id FROM billing_periods WHERE subscription_id IN (SELECT id FROM entities) AND deleted = FALSE)"
        ));
        assert!(sql.contains(
            "SELECT 1 AS tag, i.id AS entity_id, e.sequence, e.event, NULL::jsonb AS context, \
             e.recorded_at, NULL::jsonb AS forgettable_payload, NULL::BIGINT AS __ord \
             FROM n1 i JOIN billing_period_events e ON i.id = e.id"
        ));
    }

    #[test]
    fn two_children_first_has_grandchild() {
        let grandchild = leaf("line_items", "line_item_events", "billing_period_id");
        let child_a = TreeSpec {
            children: vec![grandchild],
            ..leaf(
                "billing_periods",
                "billing_period_events",
                "subscription_id",
            )
        };
        let child_b = leaf("invoices", "invoice_events", "subscription_id");
        let spec = root(
            "subscriptions",
            "subscription_events",
            vec![child_a, child_b],
        );
        let sql = build_tree_query(
            "SELECT id FROM subscriptions WHERE id = $1",
            &[],
            &spec,
            false,
            2,
        );
        assert!(sql.contains("n1 AS (SELECT id FROM billing_periods WHERE subscription_id IN (SELECT id FROM entities)"));
        assert!(sql.contains(
            "n2 AS (SELECT id FROM line_items WHERE billing_period_id IN (SELECT id FROM n1)"
        ));
        assert!(sql.contains(
            "n3 AS (SELECT id FROM invoices WHERE subscription_id IN (SELECT id FROM entities)"
        ));
        assert!(sql.contains("SELECT 1 AS tag"));
        assert!(sql.contains("SELECT 2 AS tag"));
        assert!(sql.contains("SELECT 3 AS tag"));
    }

    #[test]
    fn soft_delete_and_forgettable_and_inlined_context() {
        let child = TreeSpec {
            soft_delete: true,
            forgettable_table_name: Some("order_items_forgettable_payloads"),
            event_context: true,
            ..leaf("order_items", "order_item_events", "order_id")
        };
        let spec = root("orders", "order_events", vec![child]);
        let sql = build_tree_query("SELECT id FROM orders WHERE id = $1", &[], &spec, false, 2);
        assert!(sql.contains("AND deleted = FALSE)"));
        assert!(sql.contains(
            "LEFT JOIN order_items_forgettable_payloads p ON e.id = p.entity_id AND e.sequence = p.sequence"
        ));
        assert!(sql.contains("p.payload AS forgettable_payload"));
        assert!(sql.contains(
            "SELECT 1 AS tag, i.id AS entity_id, e.sequence, e.event, e.context AS context"
        ));
    }

    #[test]
    fn include_deleted_drops_deleted_condition_transitively() {
        let grandchild = TreeSpec {
            soft_delete: true,
            ..leaf("line_items", "line_item_events", "billing_period_id")
        };
        let child = TreeSpec {
            soft_delete: true,
            children: vec![grandchild],
            ..leaf(
                "billing_periods",
                "billing_period_events",
                "subscription_id",
            )
        };
        let spec = root("subscriptions", "subscription_events", vec![child]);
        let sql = build_tree_query(
            "SELECT id FROM subscriptions WHERE id = $1",
            &[],
            &spec,
            true,
            2,
        );
        assert!(!sql.contains("deleted = FALSE"));
    }

    #[test]
    fn empty_order_by_defaults_to_id() {
        let spec = root("subscriptions", "subscription_events", Vec::new());
        let sql = build_tree_query(
            "SELECT id FROM subscriptions WHERE id = $1",
            &[],
            &spec,
            false,
            2,
        );
        assert!(sql.contains("ROW_NUMBER() OVER (ORDER BY id) AS __ord"));
    }

    #[test]
    fn explicit_order_by_is_unprefixed_in_the_entities_cte() {
        let spec = root("entities", "entity_events", Vec::new());
        let sql = build_tree_query(
            "SELECT name, id FROM entities ORDER BY name, id LIMIT $1",
            &["name", "id"],
            &spec,
            false,
            2,
        );
        assert!(sql.contains("ROW_NUMBER() OVER (ORDER BY name, id) AS __ord"));
    }

    #[test]
    fn plain_tree_has_no_snapshot_columns_at_all() {
        // A tree with no snapshot node anywhere emits exactly today's SQL —
        // no `snapshot` columns, no fingerprint bind, byte for byte.
        let child = leaf("meters", "meter_events", "site_id");
        let spec = root("sites", "site_events", vec![child]);
        assert!(!tree_has_snapshot(&spec));
        let sql = build_tree_query("SELECT id FROM sites WHERE id = $1", &[], &spec, false, 2);
        assert!(!sql.contains("snapshot"));
        assert!(sql.contains(
            "SELECT 0 AS tag, i.id AS entity_id, e.sequence, e.event, \
             CASE WHEN $2 THEN e.context ELSE NULL::jsonb END AS context, e.recorded_at, \
             NULL::jsonb AS forgettable_payload, i.__ord AS __ord \
             FROM entities i JOIN site_events e ON i.id = e.id"
        ));
    }

    #[test]
    fn snapshot_root_plain_child() {
        let child = leaf("meters", "meter_events", "site_id");
        let spec = snapshotted(
            root("sites", "site_events", vec![child]),
            "site_snapshots",
            111,
        );
        assert!(tree_has_snapshot(&spec));
        let sql = build_tree_query("SELECT id FROM sites WHERE id = $1", &[], &spec, false, 2);
        // root branch: snapshot-aware, fingerprint bound at $3 (ctx_param_idx=2, +1)
        assert!(sql.contains("LEFT JOIN site_snapshots s ON s.id = i.id AND s.fingerprint = $3"));
        assert!(sql.contains("COALESCE(e.sequence, s.sequence) AS sequence"));
        // child branch: plain, but padded with NULL snapshot columns to match
        assert!(sql.contains(
            "SELECT 1 AS tag, i.id AS entity_id, e.sequence, e.event, NULL::jsonb AS context, \
             e.recorded_at, NULL::jsonb AS forgettable_payload, \
             NULL::jsonb AS snapshot, NULL::INT AS snapshot_sequence, \
             NULL::TIMESTAMPTZ AS snapshot_recorded_at, NULL::TIMESTAMPTZ AS snapshot_first_recorded_at, \
             NULL::jsonb AS snapshot_forgettable_payload, NULL::BIGINT AS __ord \
             FROM n1 i JOIN meter_events e ON i.id = e.id"
        ));
    }

    #[test]
    fn plain_root_snapshot_child() {
        let child = snapshotted(
            leaf("meters", "meter_events", "site_id"),
            "meter_snapshots",
            222,
        );
        let spec = root("sites", "site_events", vec![child]);
        assert!(tree_has_snapshot(&spec));
        let sql = build_tree_query("SELECT id FROM sites WHERE id = $1", &[], &spec, false, 2);
        // root branch: plain, padded (no fingerprint bind consumed for root)
        assert!(sql.contains(
            "SELECT 0 AS tag, i.id AS entity_id, e.sequence, e.event, \
             CASE WHEN $2 THEN e.context ELSE NULL::jsonb END AS context, e.recorded_at, \
             NULL::jsonb AS forgettable_payload, \
             NULL::jsonb AS snapshot, NULL::INT AS snapshot_sequence, \
             NULL::TIMESTAMPTZ AS snapshot_recorded_at, NULL::TIMESTAMPTZ AS snapshot_first_recorded_at, \
             NULL::jsonb AS snapshot_forgettable_payload, i.__ord AS __ord \
             FROM entities i JOIN site_events e ON i.id = e.id"
        ));
        // child branch: snapshot-aware, fingerprint bound at $3 (root has none, child is first)
        assert!(sql.contains("LEFT JOIN meter_snapshots s ON s.id = i.id AND s.fingerprint = $3"));
    }

    #[test]
    fn both_snapshot_param_numbering() {
        let child = snapshotted(
            leaf("meters", "meter_events", "site_id"),
            "meter_snapshots",
            222,
        );
        let spec = snapshotted(
            root("sites", "site_events", vec![child]),
            "site_snapshots",
            111,
        );
        let sql = build_tree_query("SELECT id FROM sites WHERE id = $1", &[], &spec, false, 2);
        // root's fingerprint is $3 (ctx_param_idx + 1), child's is $4 (DFS order).
        assert!(sql.contains("LEFT JOIN site_snapshots s ON s.id = i.id AND s.fingerprint = $3"));
        assert!(sql.contains("LEFT JOIN meter_snapshots s ON s.id = i.id AND s.fingerprint = $4"));
    }

    #[test]
    fn snapshot_fingerprints_walks_dfs_order_only_snapshot_nodes() {
        let grandchild = snapshotted(
            leaf("line_items", "line_item_events", "billing_period_id"),
            "line_item_snapshots",
            3,
        );
        let child_a = TreeSpec {
            children: vec![grandchild],
            ..snapshotted(
                leaf(
                    "billing_periods",
                    "billing_period_events",
                    "subscription_id",
                ),
                "billing_period_snapshots",
                2,
            )
        };
        let child_b = leaf("invoices", "invoice_events", "subscription_id");
        let spec = snapshotted(
            root(
                "subscriptions",
                "subscription_events",
                vec![child_a, child_b],
            ),
            "subscription_snapshots",
            1,
        );
        assert_eq!(snapshot_fingerprints(&spec), vec![1, 2, 3]);
    }
}

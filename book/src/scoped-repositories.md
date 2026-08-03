# Scoped Repositories

Multi-tenant applications need data access that is impossible to misuse: a
query that *forgets* to filter by the tenant column is a silent cross-tenant
leak that compiles and passes all same-tenant tests. Scoped repositories make
that mistake unrepresentable — on a scoped repo, **every generated read
function requires a scope argument**, enforced by the compiler.

## Declaring a scope column

Mark exactly one column with `scope`:

```rust,ignore
#[derive(EsRepo)]
#[es_repo(
    entity = "Customer",
    columns(
        partner_id(ty = "PartnerId", scope),
        email(ty = "String"),
    )
)]
pub struct Customers {
    pool: PgPool,
}
```

`partner_id` remains an ordinary persisted column — it is populated on
`create` from the `NewCustomer`'s field like any other column. The `scope`
marker additionally generates an entity-named scope enum:

```rust,ignore
pub enum CustomerScope {
    All,               // no filter — reads across all scopes
    Only(PartnerId),   // restricts every read to this scope value
}

impl From<PartnerId> for CustomerScope { /* => Only */ }
impl From<&PartnerId> for CustomerScope { /* => Only */ }
```

There is deliberately **no** `From<Option<PartnerId>>`: mapping `None` to
`All` would turn a stray `None` into silent all-scope access. All-scope reads
must be written explicitly — `CustomerScope::All` is greppable and auditable.

## The scoped read surface

Every generated read function gains a leading `scope: impl Into<{Entity}Scope>`
argument:

```rust,ignore
customers.find_by_id(partner_id, id).await?;              // Into => Only
customers.find_by_id(CustomerScope::All, id).await?;      // explicit escape hatch
customers.maybe_find_by_email(partner_id, email).await?;
customers.find_all::<Customer>(partner_id, &ids).await?;
customers.list_by_created_at(partner_id, args, direction).await?;
customers.list_for_filters(partner_id, filters, sort, args).await?;

customers.find_by_id(id).await?;  // does not exist — compile error
```

At runtime each function dispatches between two static, compile-time-checked
SQL variants:

- `All` executes exactly the SQL an unscoped repo would.
- `Only(value)` executes a variant with an additional `partner_id = $n`
  conjunct in every `WHERE` clause.

Both arms are plain equality predicates — sargable against a scope-column-led
index (see below). Under `Only`, a row from another scope behaves exactly like
a missing row: `find_by_*` returns `NotFound`, `maybe_find_by_*` returns
`None`, `find_all` silently omits the id, and lists never contain the row.
**Missing and not-yours look identical.**

## The bound view: `repo.scoped(scope)`

When a request performs several reads under one subject, threading the scope
into every call gets repetitive. Scoped repos additionally generate a
**bound view** — `Scoped{Repo}` — that captures the scope once:

```rust,ignore
let customers = self.customers.scoped(sub.scope());   // ScopedCustomers<'_>

customers.find_by_id(id).await?;                      // no per-call scope arg
customers.maybe_find_by_email(email).await?;
customers.list_by_created_at(args, direction).await?;
customers.find_by_id_in_op(&mut op, id).await?;       // _in_op variants too
customers.scope();                                    // the bound CustomerScope
```

Every view method simply delegates to the corresponding scope-argument fn
with the bound scope — no new SQL, identical semantics. The view **borrows**
the repository (`ScopedCustomers<'a>` holds `&'a Customers`), so it is
naturally request-scoped: it cannot be stored beyond the repo borrow, which
keeps a bound all-access or tenant view from quietly outliving the request
that justified it.

## Writes are custody-guarded

`create`, `create_all`, `update`, `update_all` and `delete` keep their
unscoped signatures. The reasoning: mutations operate on an entity value that
could only have been obtained through a scoped read (or built by domain logic
that stamped the scope column). Scope enforcement happens at the boundary that
turns ids and queries into entity data; once you hold the entity, custody of
the value is the guarantee.

## Cursors carry no filter authority

Pagination cursors are position markers only. Every page executes with the
scope conjunct in its own `WHERE` clause, so a tampered, fabricated, or
foreign cursor can only reposition pagination within the caller's own scoped
rows — it can never widen the result set, and cursor values are compared, not
dereferenced, so they cannot be used to probe for the existence of foreign
ids. Replaying a cursor minted under a different scope yields well-defined
(scoped) but position-shifted results.

## Filtering on the scope column

By default the scope column generates no query surface of its own — every
read is already filtered by it, and per-scope listing *is* the ordinary
scoped `list_by_*(Only(value), ..)`. But some callers legitimately filter by
the scope column *through the normal query surface*: an all-access admin
listing that narrows to one tenant, for example. The scope value itself
(typically authz-derived) must never be touched by caller input — the
caller's choice belongs in the `Filters` struct like any other filter.

For that, the scope column may **opt into** `find_by = true`, `list_by` or
`list_for`:

```rust,ignore
partner_id(ty = "PartnerId", scope, find_by = true, list_for(by(created_at))),
```

This generates the usual fns (`find_by_partner_id`, `list_for_partner_id_by_*`)
and includes `partner_id: Option<PartnerId>` in the generated `Filters`
struct. The caller value **composes** with the scope — it can narrow, never
widen:

| Scope     | Caller value | Result                                            |
|-----------|--------------|---------------------------------------------------|
| `All`     | none         | unfiltered                                        |
| `All`     | `p`          | `WHERE partner_id = p`                            |
| `Only(a)` | none         | `WHERE partner_id = a`                            |
| `Only(a)` | `b`          | `WHERE partner_id = b AND partner_id = a` — **empty unless `a == b`** |

Under `Only`, the column is simply double-specified — once as the caller's
filter, once as the scope conjunct, exactly like any other filter column. A
mismatching caller value is a contradictory predicate that honestly returns
an empty result (`NotFound`/`None` for `find_by_*`) instead of being
silently ignored — a caller filter can narrow but never widen the scope.
Both predicates are plain equalities, so the query stays sargable against a
scope-led index.

```rust,ignore
// admin listing: scope from authz, partner choice from the request
let scope = self.authz.enforce_permission(sub, obj, act).await?; // untouched
self.repo
    .list_for_filters(
        scope,
        CustomerFilters { partner_id: request.partner_id, ..Default::default() },
        sort,
        args,
    )
    .await?
```

## Validation rules

The macro rejects at compile time:

- more than one `scope` column per repo
- an `Option<T>` or `nullable`-annotated scope column (nullable scope columns
  are not supported — every row must belong to exactly one scope)
- a `Forgettable<T>` scope column
- `scope` on nested repos — children are custody-guarded via their (scoped)
  parent

Without an explicit opt-in (see above) the scope column generates no
`find_by_partner_id` accessors: `scope` flips the column's `find_by` default
to `false`, and the scope argument replaces them.

## Index requirements

The `Only` arm adds a leading equality on the scope column to every read, so
composite indexes should lead with it:

```sql
-- list_by_created_at under Only(p)
CREATE INDEX ON customers (partner_id, created_at DESC, id DESC);

-- list_for_status_by_created_at under Only(p)
CREATE INDEX ON customers (partner_id, status, created_at DESC, id DESC);

-- find_by_email under Only(p)
CREATE INDEX ON customers (partner_id, email);
```

Plain single-column indexes keep working (Postgres can still apply the scope
conjunct as an index qual or filter), but scope-led composites let the
paginated lists ride the index order with an early-exit `LIMIT`.

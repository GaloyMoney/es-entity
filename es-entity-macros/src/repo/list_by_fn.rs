use convert_case::{Case, Casing};
use darling::ToTokens;
use proc_macro2::{Span, TokenStream};
use quote::{TokenStreamExt, quote};

use super::options::*;

/// Cursor pagination states that each get their own SQL text, so that no
/// variant needs the non-sargable `COALESCE(..., $ IS NULL)` catch-all.
///
/// The generated list fns dispatch on these states at runtime (on the
/// `Some`-ness of the destructured cursor values) and every emitted query is
/// a static `es_query!` literal — compile-time checked and index-friendly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorState {
    /// Page 1 (no cursor): no cursor predicate at all — the query rides the
    /// index ordering with an early-exit `LIMIT`.
    First,
    /// Cursor present on a non-NULL sort value: bare `(col, id)` row
    /// comparison — sargable against a composite index.
    After,
    /// Cursor present on a NULL sort value (only possible for `Option<T>`
    /// sort columns): explicit NULL-aware predicate.
    AfterNull,
    /// Cursor present but NULL-ness is undetectable from Rust (non-`Option`
    /// type annotated `nullable`, where a custom `sqlx::Encode` writes NULL):
    /// keep the legacy COALESCE predicate which handles all cases.
    AfterLegacy,
}

/// Assemble a `SELECT ... [WHERE ...] ORDER BY ... LIMIT $n` query string
/// from individual predicates.
pub fn assemble_select(
    select_columns: &str,
    table_name: &str,
    conditions: &[String],
    order_by: &str,
    limit_param_idx: u32,
) -> String {
    let mut query = format!("SELECT {select_columns} FROM {table_name}");
    if !conditions.is_empty() {
        query.push_str(" WHERE ");
        query.push_str(&conditions.join(" AND "));
    }
    query.push_str(&format!(" ORDER BY {order_by} LIMIT ${limit_param_idx}"));
    query
}

/// The `deleted = FALSE` predicate for repos with soft delete, on the
/// non-`include_deleted` fn variants.
pub fn not_deleted_predicate(delete: DeleteOption) -> Option<String> {
    if delete.is_soft() {
        Some("deleted = FALSE".to_string())
    } else {
        None
    }
}

pub struct CursorStruct<'a> {
    pub id: &'a syn::Ident,
    pub entity: &'a syn::Ident,
    pub column: &'a Column,
    pub cursor_mod: &'a syn::Ident,
}

impl CursorStruct<'_> {
    fn name(&self) -> String {
        let entity_name = format!("{}", self.entity);
        format!("{}_by_{}_cursor", entity_name, self.column.name()).to_case(Case::UpperCamel)
    }

    pub fn ident(&self) -> syn::Ident {
        syn::Ident::new(&self.name(), Span::call_site())
    }

    pub fn cursor_mod(&self) -> &syn::Ident {
        self.cursor_mod
    }

    pub fn select_columns(&self, for_column: Option<&syn::Ident>) -> String {
        let mut for_column_str = String::new();
        if let Some(for_column) = for_column
            && self.column.name() != for_column
        {
            for_column_str = format!("{for_column}, ");
        }
        if self.column.is_id() {
            format!("{for_column_str}id")
        } else {
            format!("{}{}, id", for_column_str, self.column.name())
        }
    }

    pub fn order_by(&self, ascending: bool) -> String {
        let dir = if ascending { "ASC" } else { "DESC" };
        let nulls = if ascending { "FIRST" } else { "LAST" };
        if self.column.is_id() {
            format!("id {dir}")
        } else if self.column.is_nullable_column() {
            format!("{0} {dir} NULLS {nulls}, id {dir}", self.column.name())
        } else {
            format!("{} {dir}, id {dir}", self.column.name())
        }
    }

    pub fn condition(&self, offset: u32, ascending: bool) -> String {
        let comp = if ascending { ">" } else { "<" };
        let id_offset = offset + 2;
        let column_offset = offset + 3;

        if self.column.is_id() {
            format!("COALESCE(id {comp} ${id_offset}, true)")
        } else if self.column.is_nullable_column() {
            // The OR-clause's COALESCE fires when `col {comp} ${cursor}` is NULL,
            // which happens whenever either side of the comparison is NULL. The
            // fallback decides whether the row is "after" the cursor in
            // (col, id) ordering — and the correct answer depends on direction
            // (NULLS FIRST for ASC, NULLS LAST for DESC) and on whether we are
            // on page 1 vs. paginating from a cursor sitting on a NULL row.
            //
            // - ASC NULLS FIRST: a non-NULL row with a NULL cursor is "after"
            //   (non-NULL sorts after NULL), whether the NULL cursor represents
            //   page 1 (no cursor) or a cursor sitting on a NULL row. Fallback
            //   `col IS NOT NULL` covers both: TRUE for non-NULL rows, FALSE
            //   for NULL rows (NULL rows with a non-NULL cursor were on page 1
            //   already).
            //
            // - DESC NULLS LAST: asymmetric. NULL rows sort *last*, so:
            //     • Page 1 (`$id_offset` IS NULL, no cursor): include all rows
            //       — equivalent to "the sentinel before everything".
            //     • Cursor on NULL row (`$id_offset` set, `${column_offset}`
            //       NULL): non-NULL rows already shown → exclude.
            //     • Cursor on non-NULL row, row NULL: NULL sorts after → include.
            //   The DESC fallback `${id_offset} IS NULL OR
            //   (col IS NULL AND ${column_offset} IS NOT NULL)` captures all
            //   three: `${id_offset} IS NULL` short-circuits to TRUE on page 1,
            //   while the second disjunct catches the "include NULL rows after
            //   non-NULL cursor" case without re-including already-shown rows.
            //
            // The cursor=NULL + row=NULL case is handled by the AND-clause's
            // `IS NOT DISTINCT FROM` + the id comparison, so the OR-clause's
            // fallback must NOT fire then. Verified by truth-table on PR.
            let null_handling = if ascending {
                format!("{0} IS NOT NULL", self.column.name())
            } else {
                format!(
                    "${id_offset} IS NULL OR ({0} IS NULL AND ${column_offset} IS NOT NULL)",
                    self.column.name(),
                )
            };
            format!(
                "({0} IS NOT DISTINCT FROM ${column_offset}) AND COALESCE(id {comp} ${id_offset}, true) OR COALESCE({0} {comp} ${column_offset}, {null_handling})",
                self.column.name(),
            )
        } else {
            format!(
                "COALESCE(({0}, id) {comp} (${column_offset}, ${id_offset}), ${id_offset} IS NULL)",
                self.column.name(),
            )
        }
    }

    /// The cursor states this sort column needs distinct SQL for.
    pub fn cursor_states(&self) -> &'static [CursorState] {
        if self.column.is_id() || !self.column.is_nullable_column() {
            &[CursorState::First, CursorState::After]
        } else if self.column.is_optional() {
            &[
                CursorState::First,
                CursorState::After,
                CursorState::AfterNull,
            ]
        } else {
            // `nullable`-annotated non-Option type: NULL-ness of the cursor
            // value is invisible to Rust, so the cursor-present variant must
            // keep the legacy all-cases predicate.
            &[CursorState::First, CursorState::AfterLegacy]
        }
    }

    /// The cursor predicate for a specialized state, or `None` for
    /// [`CursorState::First`] (page 1 needs no predicate).
    ///
    /// `offset` is the number of query parameters preceding the `LIMIT`
    /// parameter (i.e. LIMIT lands on `$(offset + 1)`).
    ///
    /// The non-legacy forms are sargable: a bare `(col, id)` row comparison
    /// is an index qual against a composite index, unlike the legacy
    /// `COALESCE((col, id) < ($c, $i), $i IS NULL)` catch-all which defeats
    /// index extraction. The NULL-cursor forms replicate the exact edge
    /// semantics documented on [`Self::condition`]:
    ///
    /// - ASC (NULLS FIRST), cursor on a NULL row: all non-NULL rows plus
    ///   NULL rows with a greater id come "after" → `col IS NOT NULL OR id >
    ///   $i`.
    /// - DESC (NULLS LAST), cursor on a NULL row: only NULL rows with a
    ///   smaller id come after → `col IS NULL AND id < $i`.
    /// - DESC (NULLS LAST), cursor on a non-NULL row: NULL rows sort last,
    ///   so they are all still "after" → `col IS NULL OR (col, id) < ($c,
    ///   $i)`.
    pub fn condition_for_state(
        &self,
        state: CursorState,
        offset: u32,
        ascending: bool,
    ) -> Option<String> {
        let comp = if ascending { ">" } else { "<" };
        let id_offset = offset + 2;
        let column_offset = offset + 3;

        match state {
            CursorState::First => None,
            CursorState::AfterLegacy => Some(self.condition(offset, ascending)),
            CursorState::After => {
                if self.column.is_id() {
                    Some(format!("id {comp} ${id_offset}"))
                } else if !self.column.is_nullable_column() {
                    Some(format!(
                        "({0}, id) {comp} (${column_offset}, ${id_offset})",
                        self.column.name()
                    ))
                } else if ascending {
                    Some(format!(
                        "({0}, id) > (${column_offset}, ${id_offset})",
                        self.column.name()
                    ))
                } else {
                    Some(format!(
                        "({0} IS NULL OR ({0}, id) < (${column_offset}, ${id_offset}))",
                        self.column.name()
                    ))
                }
            }
            CursorState::AfterNull => {
                if ascending {
                    Some(format!(
                        "({0} IS NOT NULL OR id > ${id_offset})",
                        self.column.name()
                    ))
                } else {
                    Some(format!(
                        "({0} IS NULL AND id < ${id_offset})",
                        self.column.name()
                    ))
                }
            }
        }
    }

    /// Scrutinee elements (one or two bool expressions over the destructured
    /// cursor locals) identifying the cursor state at runtime.
    pub fn state_scrutinee_elems(&self) -> Vec<TokenStream> {
        if self.column.is_nullable_column() && self.column.is_optional() {
            let column_name = self.column.name();
            vec![quote! { id.is_some() }, quote! { #column_name.is_some() }]
        } else {
            vec![quote! { id.is_none() }]
        }
    }

    /// Pattern elements matching [`Self::state_scrutinee_elems`] for one
    /// state.
    pub fn state_pattern_elems(&self, state: CursorState) -> Vec<TokenStream> {
        if self.column.is_nullable_column() && self.column.is_optional() {
            match state {
                CursorState::First => vec![quote! { false }, quote! { _ }],
                CursorState::After => vec![quote! { true }, quote! { true }],
                CursorState::AfterNull => vec![quote! { true }, quote! { false }],
                CursorState::AfterLegacy => {
                    unreachable!("Option columns never use AfterLegacy")
                }
            }
        } else {
            match state {
                CursorState::First => vec![quote! { true }],
                _ => vec![quote! { false }],
            }
        }
    }

    /// Cursor value bindings (without the `LIMIT` binding) for a state.
    pub fn cursor_arg_tokens_for_state(&self, state: CursorState) -> TokenStream {
        let id = self.id;

        match state {
            CursorState::First => quote! {},
            CursorState::AfterNull => quote! {
                id as Option<#id>,
            },
            _ => self.cursor_arg_tokens(),
        }
    }

    fn cursor_arg_tokens(&self) -> TokenStream {
        let id = self.id;

        if self.column.is_id() {
            quote! {
                id as Option<#id>,
            }
        } else if self.column.is_optional() {
            let column_name = self.column.name();
            let column_type = self.column.ty();
            quote! {
                id as Option<#id>,
                #column_name as #column_type,
            }
        } else {
            let column_name = self.column.name();
            let column_type = self.column.ty();
            quote! {
                id as Option<#id>,
                #column_name as Option<#column_type>,
            }
        }
    }

    pub fn query_arg_tokens(&self) -> TokenStream {
        let cursor_args = self.cursor_arg_tokens();
        quote! {
            (first + 1) as i64,
            #cursor_args
        }
    }

    pub fn destructure_tokens(&self) -> TokenStream {
        let column_name = self.column.name();

        let mut after_args = quote! {
            (id, #column_name)
        };
        let mut after_destruction = quote! {
            (Some(after.id), Some(after.#column_name))
        };
        let mut after_default = quote! {
            (None, None)
        };

        if self.column.is_id() {
            after_args = quote! {
                id
            };
            after_destruction = quote! {
                Some(after.id)
            };
            after_default = quote! {
                None
            };
        } else if self.column.is_optional() {
            after_destruction = quote! {
                (Some(after.id), after.#column_name)
            };
        }

        quote! {
            let es_entity::PaginatedQueryArgs { first, after } = cursor;
            let #after_args = if let Some(after) = after {
                #after_destruction
            } else {
                #after_default
            };
        }
    }

    #[cfg(feature = "graphql")]
    pub fn gql_cursor(&self) -> TokenStream {
        let ident = self.ident();
        quote! {
            impl es_entity::graphql::async_graphql::connection::CursorType for #ident {
                type Error = String;

                fn encode_cursor(&self) -> String {
                    use es_entity::graphql::base64::{engine::general_purpose, Engine as _};
                    let json = es_entity::prelude::serde_json::to_string(&self).expect("could not serialize token");
                    general_purpose::STANDARD_NO_PAD.encode(json.as_bytes())
                }

                fn decode_cursor(s: &str) -> Result<Self, Self::Error> {
                    use es_entity::graphql::base64::{engine::general_purpose, Engine as _};
                    let bytes = general_purpose::STANDARD_NO_PAD
                        .decode(s.as_bytes())
                        .map_err(|e| e.to_string())?;
                    let json = String::from_utf8(bytes).map_err(|e| e.to_string())?;
                    es_entity::prelude::serde_json::from_str(&json).map_err(|e| e.to_string())
                }
            }
        }
    }
}

impl ToTokens for CursorStruct<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = self.entity;
        let accessor = &self.column.accessor();
        let ident = self.ident();
        let id = &self.id;

        let (field, from_impl) = if self.column.is_id() {
            (quote! {}, quote! {})
        } else {
            let column_name = self.column.name();
            let column_type = self.column.ty();
            (
                quote! {
                    pub #column_name: #column_type,
                },
                quote! {
                    #column_name: entity.#accessor.clone(),
                },
            )
        };

        tokens.append_all(quote! {
            #[derive(Debug, serde::Serialize, serde::Deserialize)]
            pub struct #ident {
                pub id: #id,
                #field
            }

            impl From<&#entity> for #ident {
                fn from(entity: &#entity) -> Self {
                    Self {
                        id: entity.id.clone(),
                        #from_impl
                    }
                }
            }
        });
    }
}

pub struct ListByFn<'a> {
    ignore_prefix: Option<&'a syn::LitStr>,
    id: &'a syn::Ident,
    entity: &'a syn::Ident,
    column: &'a Column,
    table_name: &'a str,
    query_error: syn::Ident,
    delete: DeleteOption,
    cursor_mod: syn::Ident,
    any_nested: bool,
    post_hydrate_error: Option<&'a syn::Type>,
    forgettable_table_name: Option<&'a str>,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> ListByFn<'a> {
    pub fn new(column: &'a Column, opts: &'a RepositoryOptions) -> Self {
        Self {
            ignore_prefix: opts.table_prefix(),
            column,
            id: opts.id(),
            entity: opts.entity(),
            table_name: opts.table_name(),
            query_error: opts.query_error(),
            delete: opts.delete,
            cursor_mod: opts.cursor_mod(),
            any_nested: opts.any_nested(),
            post_hydrate_error: opts.post_hydrate_hook.as_ref().map(|h| &h.error),
            forgettable_table_name: opts.forgettable_table_name(),
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }

    pub fn cursor(&'a self) -> CursorStruct<'a> {
        CursorStruct {
            column: self.column,
            id: self.id,
            entity: self.entity,
            cursor_mod: &self.cursor_mod,
        }
    }
}

impl ToTokens for ListByFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = self.entity;
        let column_name = self.column.name();
        let cursor = self.cursor();
        let cursor_ident = cursor.ident();
        let cursor_mod = cursor.cursor_mod();
        let query_error = &self.query_error;
        let query_fn_generics = RepositoryOptions::query_fn_generics(self.any_nested);
        let query_fn_op_arg = RepositoryOptions::query_fn_op_arg(self.any_nested);
        let query_fn_op_traits = RepositoryOptions::query_fn_op_traits(self.any_nested);
        let query_fn_get_op = RepositoryOptions::query_fn_get_op(self.any_nested);

        let destructure_tokens = self.cursor().destructure_tokens();
        let select_columns = cursor.select_columns(None);

        for delete in [DeleteOption::No, DeleteOption::Soft] {
            let fn_name = syn::Ident::new(
                &format!(
                    "list_by_{}{}",
                    column_name,
                    delete.include_deletion_fn_postfix()
                ),
                Span::call_site(),
            );
            let fn_in_op = syn::Ident::new(
                &format!(
                    "list_by_{}{}_in_op",
                    column_name,
                    delete.include_deletion_fn_postfix()
                ),
                Span::call_site(),
            );

            let forgettable_tbl_arg = if let Some(tbl) = self.forgettable_table_name {
                quote! { forgettable_tbl = #tbl, }
            } else {
                quote! {}
            };

            let make_es_query = |query: &str, args: &TokenStream| -> TokenStream {
                if let Some(prefix) = self.ignore_prefix {
                    quote! {
                        es_entity::es_query!(
                            tbl_prefix = #prefix,
                            #forgettable_tbl_arg
                            #query,
                            #args
                        )
                    }
                } else {
                    quote! {
                        es_entity::es_query!(
                            entity = #entity,
                            #forgettable_tbl_arg
                            #query,
                            #args
                        )
                    }
                }
            };

            let mut query_arms = TokenStream::new();
            for state in cursor.cursor_states() {
                for ascending in [true, false] {
                    let mut conditions: Vec<String> = Vec::new();
                    if let Some(condition) = cursor.condition_for_state(*state, 0, ascending) {
                        conditions.push(format!("({condition})"));
                    }
                    if delete == DeleteOption::No
                        && let Some(not_deleted) = not_deleted_predicate(self.delete)
                    {
                        conditions.push(not_deleted);
                    }
                    let query = assemble_select(
                        &select_columns,
                        self.table_name,
                        &conditions,
                        &cursor.order_by(ascending),
                        1,
                    );
                    let cursor_args = cursor.cursor_arg_tokens_for_state(*state);
                    let args = quote! {
                        (first + 1) as i64,
                        #cursor_args
                    };
                    let es_query_call = make_es_query(&query, &args);
                    let direction_pattern = if ascending {
                        quote! { es_entity::ListDirection::Ascending }
                    } else {
                        quote! { es_entity::ListDirection::Descending }
                    };
                    let state_pattern = cursor.state_pattern_elems(*state);
                    query_arms.append_all(quote! {
                        (#direction_pattern, #(#state_pattern),*) => {
                            #es_query_call.fetch_n(op, first).await?
                        },
                    });
                }
            }
            let cursor_state_scrutinee = cursor.state_scrutinee_elems();

            #[cfg(feature = "instrument")]
            let (
                instrument_attr,
                extract_has_cursor,
                record_fields,
                record_results,
                error_recording,
            ) = {
                let entity_name = entity.to_string();
                let repo_name = &self.repo_name_snake;
                let span_name = format!("{}.list_by_{}", repo_name, column_name);
                (
                    quote! {
                        #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, first, has_cursor, direction = tracing::field::debug(&direction), count = tracing::field::Empty, has_next_page = tracing::field::Empty, ids = tracing::field::Empty, error = tracing::field::Empty, exception.message = tracing::field::Empty, exception.type = tracing::field::Empty))]
                    },
                    quote! {
                        let has_cursor = cursor.after.is_some();
                    },
                    quote! {
                        tracing::Span::current().record("first", first);
                        tracing::Span::current().record("has_cursor", has_cursor);
                    },
                    quote! {
                        let result_ids: Vec<_> = entities.iter().map(|e| &e.id).collect();
                        tracing::Span::current().record("count", result_ids.len());
                        tracing::Span::current().record("has_next_page", has_next_page);
                        tracing::Span::current().record("ids", tracing::field::debug(&result_ids));
                    },
                    quote! {
                        if let Err(ref e) = __result {
                            tracing::Span::current().record("error", true);
                            tracing::Span::current().record("exception.message", tracing::field::display(e));
                            tracing::Span::current().record("exception.type", std::any::type_name_of_val(e));
                        }
                    },
                )
            };
            #[cfg(not(feature = "instrument"))]
            let (
                instrument_attr,
                extract_has_cursor,
                record_fields,
                record_results,
                error_recording,
            ) = (quote! {}, quote! {}, quote! {}, quote! {}, quote! {});

            let post_hydrate_check = if self.post_hydrate_error.is_some() {
                quote! {
                    for __entity in &entities {
                        self.execute_post_hydrate_hook(__entity).map_err(#query_error::PostHydrateError)?;
                    }
                }
            } else {
                quote! {}
            };

            tokens.append_all(quote! {
                pub async fn #fn_name(
                    &self,
                    cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                    direction: es_entity::ListDirection,
                ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #query_error> {
                    self.#fn_in_op(#query_fn_get_op, cursor, direction).await
                }

                #instrument_attr
                pub async fn #fn_in_op #query_fn_generics(
                    &self,
                    #query_fn_op_arg,
                    cursor: es_entity::PaginatedQueryArgs<#cursor_mod::#cursor_ident>,
                    direction: es_entity::ListDirection,
                ) -> Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #query_error>
                   where
                       OP: #query_fn_op_traits
                 {
                    let __result: Result<es_entity::PaginatedQueryRet<#entity, #cursor_mod::#cursor_ident>, #query_error> = async {
                        #extract_has_cursor
                        #destructure_tokens
                        #record_fields

                        let (entities, has_next_page) = match (direction, #(#cursor_state_scrutinee),*) {
                            #query_arms
                        };

                        #post_hydrate_check
                        #record_results

                        let end_cursor = entities.last().map(#cursor_mod::#cursor_ident::from);

                        Ok(es_entity::PaginatedQueryRet {
                            entities,
                            has_next_page,
                            end_cursor,
                        })
                    }.await;

                    #error_recording
                    __result
                }
            });

            if delete == self.delete || self.delete == DeleteOption::SoftWithoutQueries {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::Span;
    use syn::Ident;

    #[test]
    fn cursor_struct_by_id() {
        let id_type = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let by_column = Column::for_id(syn::parse_str("EntityId").unwrap());
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let cursor = CursorStruct {
            column: &by_column,
            id: &id_type,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };

        let mut tokens = TokenStream::new();
        cursor.to_tokens(&mut tokens);

        let expected = quote! {
            #[derive(Debug, serde::Serialize, serde::Deserialize)]
            pub struct EntityByIdCursor {
                pub id: EntityId,
            }

            impl From<&Entity> for EntityByIdCursor {
                fn from(entity: &Entity) -> Self {
                    Self {
                        id: entity.id.clone(),
                    }
                }
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn cursor_struct_by_created_at() {
        let id_type = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let by_column = Column::for_created_at();
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let cursor = CursorStruct {
            column: &by_column,
            id: &id_type,
            entity: &entity,
            cursor_mod: &cursor_mod,
        };

        let mut tokens = TokenStream::new();
        cursor.to_tokens(&mut tokens);

        let expected = quote! {
            #[derive(Debug, serde::Serialize, serde::Deserialize)]
            pub struct EntityByCreatedAtCursor {
                pub id: EntityId,
                pub created_at: es_entity::prelude::chrono::DateTime<es_entity::prelude::chrono::Utc>,
            }

            impl From<&Entity> for EntityByCreatedAtCursor {
                fn from(entity: &Entity) -> Self {
                    Self {
                        id: entity.id.clone(),
                        created_at: entity.events()
                            .entity_first_persisted_at()
                            .expect("entity not persisted")
                            .clone(),
                    }
                }
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn list_by_fn() {
        let id_type = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let query_error = syn::Ident::new("EntityQueryError", Span::call_site());
        let column = Column::for_id(syn::parse_str("EntityId").unwrap());
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let persist_fn = ListByFn {
            ignore_prefix: None,
            column: &column,
            id: &id_type,
            entity: &entity,
            table_name: "entities",
            query_error,
            delete: DeleteOption::SoftWithoutQueries,
            cursor_mod,
            any_nested: false,
            post_hydrate_error: None,
            forgettable_table_name: None,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
        pub async fn list_by_id (& self , cursor : es_entity :: PaginatedQueryArgs < cursor_mod :: EntityByIdCursor > , direction : es_entity :: ListDirection ,) -> Result < es_entity :: PaginatedQueryRet < Entity , cursor_mod :: EntityByIdCursor > , EntityQueryError > { self . list_by_id_in_op (self . pool () , cursor , direction) . await } pub async fn list_by_id_in_op < 'a , OP > (& self , op : OP , cursor : es_entity :: PaginatedQueryArgs < cursor_mod :: EntityByIdCursor > , direction : es_entity :: ListDirection ,) -> Result < es_entity :: PaginatedQueryRet < Entity , cursor_mod :: EntityByIdCursor > , EntityQueryError > where OP : es_entity :: IntoOneTimeExecutor < 'a > { let __result : Result < es_entity :: PaginatedQueryRet < Entity , cursor_mod :: EntityByIdCursor > , EntityQueryError > = async { let es_entity :: PaginatedQueryArgs { first , after } = cursor ; let id = if let Some (after) = after { Some (after . id) } else { None } ; let (entities , has_next_page) = match (direction , id . is_none ()) { (es_entity :: ListDirection :: Ascending , true) => { es_entity :: es_query ! (entity = Entity , "SELECT id FROM entities WHERE deleted = FALSE ORDER BY id ASC LIMIT $1" , (first + 1) as i64 ,) . fetch_n (op , first) . await ? } , (es_entity :: ListDirection :: Descending , true) => { es_entity :: es_query ! (entity = Entity , "SELECT id FROM entities WHERE deleted = FALSE ORDER BY id DESC LIMIT $1" , (first + 1) as i64 ,) . fetch_n (op , first) . await ? } , (es_entity :: ListDirection :: Ascending , false) => { es_entity :: es_query ! (entity = Entity , "SELECT id FROM entities WHERE (id > $2) AND deleted = FALSE ORDER BY id ASC LIMIT $1" , (first + 1) as i64 , id as Option < EntityId > ,) . fetch_n (op , first) . await ? } , (es_entity :: ListDirection :: Descending , false) => { es_entity :: es_query ! (entity = Entity , "SELECT id FROM entities WHERE (id < $2) AND deleted = FALSE ORDER BY id DESC LIMIT $1" , (first + 1) as i64 , id as Option < EntityId > ,) . fetch_n (op , first) . await ? } , } ; let end_cursor = entities . last () . map (cursor_mod :: EntityByIdCursor :: from) ; Ok (es_entity :: PaginatedQueryRet { entities , has_next_page , end_cursor , }) } . await ; __result }
                };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn list_by_fn_with_soft_delete_include_deleted() {
        let id_type = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let query_error = syn::Ident::new("EntityQueryError", Span::call_site());
        let column = Column::for_id(syn::parse_str("EntityId").unwrap());
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let persist_fn = ListByFn {
            ignore_prefix: None,
            column: &column,
            id: &id_type,
            entity: &entity,
            table_name: "entities",
            query_error,
            delete: DeleteOption::Soft,
            cursor_mod,
            any_nested: false,
            post_hydrate_error: None,
            forgettable_table_name: None,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let token_str = tokens.to_string();
        assert!(token_str.contains("list_by_id_include_deleted"));
    }

    #[test]
    fn list_by_fn_name() {
        let id_type = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let query_error = syn::Ident::new("EntityQueryError", Span::call_site());
        let column = Column::new(
            syn::Ident::new("name", proc_macro2::Span::call_site()),
            syn::parse_str("String").unwrap(),
        );
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let persist_fn = ListByFn {
            ignore_prefix: None,
            column: &column,
            id: &id_type,
            entity: &entity,
            table_name: "entities",
            query_error,
            delete: DeleteOption::No,
            cursor_mod,
            any_nested: false,
            post_hydrate_error: None,
            forgettable_table_name: None,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
        pub async fn list_by_name (& self , cursor : es_entity :: PaginatedQueryArgs < cursor_mod :: EntityByNameCursor > , direction : es_entity :: ListDirection ,) -> Result < es_entity :: PaginatedQueryRet < Entity , cursor_mod :: EntityByNameCursor > , EntityQueryError > { self . list_by_name_in_op (self . pool () , cursor , direction) . await } pub async fn list_by_name_in_op < 'a , OP > (& self , op : OP , cursor : es_entity :: PaginatedQueryArgs < cursor_mod :: EntityByNameCursor > , direction : es_entity :: ListDirection ,) -> Result < es_entity :: PaginatedQueryRet < Entity , cursor_mod :: EntityByNameCursor > , EntityQueryError > where OP : es_entity :: IntoOneTimeExecutor < 'a > { let __result : Result < es_entity :: PaginatedQueryRet < Entity , cursor_mod :: EntityByNameCursor > , EntityQueryError > = async { let es_entity :: PaginatedQueryArgs { first , after } = cursor ; let (id , name) = if let Some (after) = after { (Some (after . id) , Some (after . name)) } else { (None , None) } ; let (entities , has_next_page) = match (direction , id . is_none ()) { (es_entity :: ListDirection :: Ascending , true) => { es_entity :: es_query ! (entity = Entity , "SELECT name, id FROM entities ORDER BY name ASC, id ASC LIMIT $1" , (first + 1) as i64 ,) . fetch_n (op , first) . await ? } , (es_entity :: ListDirection :: Descending , true) => { es_entity :: es_query ! (entity = Entity , "SELECT name, id FROM entities ORDER BY name DESC, id DESC LIMIT $1" , (first + 1) as i64 ,) . fetch_n (op , first) . await ? } , (es_entity :: ListDirection :: Ascending , false) => { es_entity :: es_query ! (entity = Entity , "SELECT name, id FROM entities WHERE ((name, id) > ($3, $2)) ORDER BY name ASC, id ASC LIMIT $1" , (first + 1) as i64 , id as Option < EntityId > , name as Option < String > ,) . fetch_n (op , first) . await ? } , (es_entity :: ListDirection :: Descending , false) => { es_entity :: es_query ! (entity = Entity , "SELECT name, id FROM entities WHERE ((name, id) < ($3, $2)) ORDER BY name DESC, id DESC LIMIT $1" , (first + 1) as i64 , id as Option < EntityId > , name as Option < String > ,) . fetch_n (op , first) . await ? } , } ; let end_cursor = entities . last () . map (cursor_mod :: EntityByNameCursor :: from) ; Ok (es_entity :: PaginatedQueryRet { entities , has_next_page , end_cursor , }) } . await ; __result }
                };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn list_by_fn_optional_column() {
        let id_type = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let query_error = syn::Ident::new("EntityQueryError", Span::call_site());
        let column = Column::new(
            syn::Ident::new("value", proc_macro2::Span::call_site()),
            syn::parse_str("Option<rust_decimal::Decimal>").unwrap(),
        );
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let persist_fn = ListByFn {
            ignore_prefix: None,
            column: &column,
            id: &id_type,
            entity: &entity,
            table_name: "entities",
            query_error,
            delete: DeleteOption::No,
            cursor_mod,
            any_nested: false,
            post_hydrate_error: None,
            forgettable_table_name: None,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
        pub async fn list_by_value (& self , cursor : es_entity :: PaginatedQueryArgs < cursor_mod :: EntityByValueCursor > , direction : es_entity :: ListDirection ,) -> Result < es_entity :: PaginatedQueryRet < Entity , cursor_mod :: EntityByValueCursor > , EntityQueryError > { self . list_by_value_in_op (self . pool () , cursor , direction) . await } pub async fn list_by_value_in_op < 'a , OP > (& self , op : OP , cursor : es_entity :: PaginatedQueryArgs < cursor_mod :: EntityByValueCursor > , direction : es_entity :: ListDirection ,) -> Result < es_entity :: PaginatedQueryRet < Entity , cursor_mod :: EntityByValueCursor > , EntityQueryError > where OP : es_entity :: IntoOneTimeExecutor < 'a > { let __result : Result < es_entity :: PaginatedQueryRet < Entity , cursor_mod :: EntityByValueCursor > , EntityQueryError > = async { let es_entity :: PaginatedQueryArgs { first , after } = cursor ; let (id , value) = if let Some (after) = after { (Some (after . id) , after . value) } else { (None , None) } ; let (entities , has_next_page) = match (direction , id . is_some () , value . is_some ()) { (es_entity :: ListDirection :: Ascending , false , _) => { es_entity :: es_query ! (entity = Entity , "SELECT value, id FROM entities ORDER BY value ASC NULLS FIRST, id ASC LIMIT $1" , (first + 1) as i64 ,) . fetch_n (op , first) . await ? } , (es_entity :: ListDirection :: Descending , false , _) => { es_entity :: es_query ! (entity = Entity , "SELECT value, id FROM entities ORDER BY value DESC NULLS LAST, id DESC LIMIT $1" , (first + 1) as i64 ,) . fetch_n (op , first) . await ? } , (es_entity :: ListDirection :: Ascending , true , true) => { es_entity :: es_query ! (entity = Entity , "SELECT value, id FROM entities WHERE ((value, id) > ($3, $2)) ORDER BY value ASC NULLS FIRST, id ASC LIMIT $1" , (first + 1) as i64 , id as Option < EntityId > , value as Option < rust_decimal :: Decimal > ,) . fetch_n (op , first) . await ? } , (es_entity :: ListDirection :: Descending , true , true) => { es_entity :: es_query ! (entity = Entity , "SELECT value, id FROM entities WHERE ((value IS NULL OR (value, id) < ($3, $2))) ORDER BY value DESC NULLS LAST, id DESC LIMIT $1" , (first + 1) as i64 , id as Option < EntityId > , value as Option < rust_decimal :: Decimal > ,) . fetch_n (op , first) . await ? } , (es_entity :: ListDirection :: Ascending , true , false) => { es_entity :: es_query ! (entity = Entity , "SELECT value, id FROM entities WHERE ((value IS NOT NULL OR id > $2)) ORDER BY value ASC NULLS FIRST, id ASC LIMIT $1" , (first + 1) as i64 , id as Option < EntityId > ,) . fetch_n (op , first) . await ? } , (es_entity :: ListDirection :: Descending , true , false) => { es_entity :: es_query ! (entity = Entity , "SELECT value, id FROM entities WHERE ((value IS NULL AND id < $2)) ORDER BY value DESC NULLS LAST, id DESC LIMIT $1" , (first + 1) as i64 , id as Option < EntityId > ,) . fetch_n (op , first) . await ? } , } ; let end_cursor = entities . last () . map (cursor_mod :: EntityByValueCursor :: from) ; Ok (es_entity :: PaginatedQueryRet { entities , has_next_page , end_cursor , }) } . await ; __result }
                };

        assert_eq!(tokens.to_string(), expected.to_string());
    }

    #[test]
    fn list_by_fn_nullable_attribute_emits_nullable_aware_sql_for_non_option_type() {
        // The `nullable` attribute lets non-Option<T> Rust types (e.g. domain
        // enums whose custom sqlx::Encode writes NULL for one variant) opt
        // into the nullable-aware cursor SQL form. The Rust type is NOT
        // syntactically Option<T>, but the emitted SQL should match what an
        // Option<T> column would get: `NULLS FIRST/LAST` ordering, the
        // `IS NOT DISTINCT FROM` cursor form, and the direction-aware NULL
        // fallback.
        //
        // The query parameter cast (else branch of query_arg_tokens) still
        // wraps the Rust type in Option<...> because is_optional() remains
        // false — the type itself drives binding, while the new
        // is_nullable_column() drives SQL shape.
        let id_type = Ident::new("EntityId", Span::call_site());
        let entity = Ident::new("Entity", Span::call_site());
        let query_error = syn::Ident::new("EntityQueryError", Span::call_site());
        let column = Column::new_nullable(
            syn::Ident::new("value", proc_macro2::Span::call_site()),
            syn::parse_str("DomainEnum").unwrap(),
        );
        let cursor_mod = Ident::new("cursor_mod", Span::call_site());

        let persist_fn = ListByFn {
            ignore_prefix: None,
            column: &column,
            id: &id_type,
            entity: &entity,
            table_name: "entities",
            query_error,
            delete: DeleteOption::No,
            cursor_mod,
            any_nested: false,
            post_hydrate_error: None,
            forgettable_table_name: None,
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        persist_fn.to_tokens(&mut tokens);

        let expected = quote! {
        pub async fn list_by_value (& self , cursor : es_entity :: PaginatedQueryArgs < cursor_mod :: EntityByValueCursor > , direction : es_entity :: ListDirection ,) -> Result < es_entity :: PaginatedQueryRet < Entity , cursor_mod :: EntityByValueCursor > , EntityQueryError > { self . list_by_value_in_op (self . pool () , cursor , direction) . await } pub async fn list_by_value_in_op < 'a , OP > (& self , op : OP , cursor : es_entity :: PaginatedQueryArgs < cursor_mod :: EntityByValueCursor > , direction : es_entity :: ListDirection ,) -> Result < es_entity :: PaginatedQueryRet < Entity , cursor_mod :: EntityByValueCursor > , EntityQueryError > where OP : es_entity :: IntoOneTimeExecutor < 'a > { let __result : Result < es_entity :: PaginatedQueryRet < Entity , cursor_mod :: EntityByValueCursor > , EntityQueryError > = async { let es_entity :: PaginatedQueryArgs { first , after } = cursor ; let (id , value) = if let Some (after) = after { (Some (after . id) , Some (after . value)) } else { (None , None) } ; let (entities , has_next_page) = match (direction , id . is_none ()) { (es_entity :: ListDirection :: Ascending , true) => { es_entity :: es_query ! (entity = Entity , "SELECT value, id FROM entities ORDER BY value ASC NULLS FIRST, id ASC LIMIT $1" , (first + 1) as i64 ,) . fetch_n (op , first) . await ? } , (es_entity :: ListDirection :: Descending , true) => { es_entity :: es_query ! (entity = Entity , "SELECT value, id FROM entities ORDER BY value DESC NULLS LAST, id DESC LIMIT $1" , (first + 1) as i64 ,) . fetch_n (op , first) . await ? } , (es_entity :: ListDirection :: Ascending , false) => { es_entity :: es_query ! (entity = Entity , "SELECT value, id FROM entities WHERE ((value IS NOT DISTINCT FROM $3) AND COALESCE(id > $2, true) OR COALESCE(value > $3, value IS NOT NULL)) ORDER BY value ASC NULLS FIRST, id ASC LIMIT $1" , (first + 1) as i64 , id as Option < EntityId > , value as Option < DomainEnum > ,) . fetch_n (op , first) . await ? } , (es_entity :: ListDirection :: Descending , false) => { es_entity :: es_query ! (entity = Entity , "SELECT value, id FROM entities WHERE ((value IS NOT DISTINCT FROM $3) AND COALESCE(id < $2, true) OR COALESCE(value < $3, $2 IS NULL OR (value IS NULL AND $3 IS NOT NULL))) ORDER BY value DESC NULLS LAST, id DESC LIMIT $1" , (first + 1) as i64 , id as Option < EntityId > , value as Option < DomainEnum > ,) . fetch_n (op , first) . await ? } , } ; let end_cursor = entities . last () . map (cursor_mod :: EntityByValueCursor :: from) ; Ok (es_entity :: PaginatedQueryRet { entities , has_next_page , end_cursor , }) } . await ; __result }
                };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}

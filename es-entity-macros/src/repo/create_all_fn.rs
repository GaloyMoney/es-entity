use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

pub struct CreateAllFn<'a> {
    entity: &'a syn::Ident,
    table_name: &'a str,
    columns: &'a Columns,
    error: &'a syn::Type,
    nested_fn_names: Vec<syn::Ident>,
    additional_op_constraint: proc_macro2::TokenStream,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> From<&'a RepositoryOptions> for CreateAllFn<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            table_name: opts.table_name(),
            entity: opts.entity(),
            error: opts.err(),
            nested_fn_names: opts
                .all_nested()
                .map(|f| f.create_nested_fn_name())
                .collect(),
            columns: &opts.columns,
            additional_op_constraint: opts.additional_op_constraint(),
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }
}

impl ToTokens for CreateAllFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = self.entity;
        let error = self.error;
        let additional_op_constraint = &self.additional_op_constraint;

        let nested = self.nested_fn_names.iter().map(|f| {
            quote! {
                self.#f(op, &mut entity).await?;
            }
        });
        let maybe_mut_entity = if self.nested_fn_names.is_empty() {
            quote! { entity }
        } else {
            quote! { mut entity }
        };

        let table_name = self.table_name;

        let column_names = self.columns.insert_column_names();
        let placeholders = self.columns.insert_placeholders(1);
        let (arg_collection, bindings) = self
            .columns
            .create_all_arg_collection(syn::parse_quote! { new_entity });

        let query = format!(
            "INSERT INTO {} (created_at, {}) \
            SELECT COALESCE($1, NOW()), unnested.{} \
            FROM UNNEST({}) \
            AS unnested({})",
            table_name,
            column_names.join(", "),
            column_names.join(", unnested."),
            placeholders,
            column_names.join(", "),
        );

        #[cfg(feature = "instrument")]
        let (instrument_attr, error_recording) = {
            let entity_name = entity.to_string();
            let repo_name = &self.repo_name_snake;
            let span_name = format!("{}.create_all", repo_name);
            (
                quote! {
                    #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, count = new_entities.len(), exception.message = tracing::field::Empty, exception.type = tracing::field::Empty))]
                },
                quote! {
                    if let Err(ref e) = __result {
                        tracing::Span::current().record("exception.message", tracing::field::display(e));
                        tracing::Span::current().record("exception.type", std::any::type_name_of_val(e));
                    }
                },
            )
        };
        #[cfg(not(feature = "instrument"))]
        let (instrument_attr, error_recording) = (quote! {}, quote! {});

        tokens.append_all(quote! {
            pub async fn create_all(
                &self,
                new_entities: Vec<<#entity as es_entity::EsEntity>::New>
            ) -> Result<Vec<#entity>, #error> {
                let mut op = self.begin_op().await?;
                let res = self.create_all_in_op(&mut op, new_entities).await?;
                op.commit().await?;
                Ok(res)
            }

            #instrument_attr
            pub async fn create_all_in_op<OP>(
                &self,
                op: &mut OP,
                new_entities: Vec<<#entity as es_entity::EsEntity>::New>
            ) -> Result<Vec<#entity>, #error>
            where
                OP: es_entity::AtomicOperation
                #additional_op_constraint
            {
                let __result: Result<Vec<#entity>, #error> = async {
                    let mut res = Vec::new();
                    if new_entities.is_empty() {
                        return Ok(res);
                    }

                    #arg_collection

                    let now = op.now();
                    sqlx::query(#query)
                       .bind(now)
                       #(#bindings)*
                       .fetch_all(op.as_executor())
                       .await?;


                    let mut all_events: Vec<es_entity::EntityEvents<<#entity as es_entity::EsEntity>::Event>> = new_entities.into_iter().map(Self::convert_new).collect();
                    let mut n_persisted = self.persist_events_batch(op, &mut all_events).await?;

                    for events in all_events.into_iter() {
                        let n_events = n_persisted.remove(events.id()).expect("n_events exists");
                        let #maybe_mut_entity = Self::hydrate_entity(events)?;

                        #(#nested)*

                        self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await?;
                        res.push(entity);
                    }

                    Ok(res)
                }.await;

                #error_recording
                __result
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::Span;
    use syn::Ident;

    #[test]
    fn create_all_fn() {
        let entity = Ident::new("Entity", Span::call_site());
        let error = syn::parse_str("es_entity::EsRepoError").unwrap();

        use darling::FromMeta;
        let input: syn::Meta = syn::parse_quote!(columns(id = "EntityId", name = "String",));
        let columns = Columns::from_meta(&input).expect("Failed to parse Fields");

        let create_fn = CreateAllFn {
            table_name: "entities",
            entity: &entity,
            error: &error,
            columns: &columns,
            nested_fn_names: Vec::new(),
            additional_op_constraint: quote! {},
            #[cfg(feature = "instrument")]
            repo_name_snake: "test_repo".to_string(),
        };

        let mut tokens = TokenStream::new();
        create_fn.to_tokens(&mut tokens);

        let mut tokens = TokenStream::new();
        create_fn.to_tokens(&mut tokens);

        let expected = quote! {
            pub async fn create_all(
                &self,
                new_entities: Vec<<Entity as es_entity::EsEntity>::New>
            ) -> Result<Vec<Entity>, es_entity::EsRepoError> {
                let mut op = self.begin_op().await?;
                let res = self.create_all_in_op(&mut op, new_entities).await?;
                op.commit().await?;
                Ok(res)
            }

            pub async fn create_all_in_op<OP>(
                &self,
                op: &mut OP,
                new_entities: Vec<<Entity as es_entity::EsEntity>::New>
            ) -> Result<Vec<Entity>, es_entity::EsRepoError>
            where
                OP: es_entity::AtomicOperation
            {
                let mut res = Vec::new();
                if new_entities.is_empty() {
                    return Ok(res);
                }

                let mut id_collection = Vec::new();
                let mut name_collection = Vec::new();

                for new_entity in new_entities.iter() {
                    let id: &EntityId = &new_entity.id;
                    let name: &String = &new_entity.name;

                    id_collection.push(id);
                    name_collection.push(name);
                }

                let now = op.now();
                sqlx::query(
                    "INSERT INTO entities (created_at, id, name) SELECT COALESCE($1, NOW()), unnested.id, unnested.name FROM UNNEST($2, $3) AS unnested(id, name)")
                    .bind(now)
                    .bind(id_collection)
                    .bind(name_collection)
                    .fetch_all(op.as_executor())
                    .await?;


                let mut all_events: Vec<es_entity::EntityEvents<<#entity as es_entity::EsEntity>::Event>> = new_entities.into_iter().map(Self::convert_new).collect();
                let mut n_persisted = self.persist_events_batch(op, &mut all_events).await?;

                for events in all_events.into_iter() {
                    let n_events = n_persisted.remove(events.id()).expect("n_events exists");
                    let entity = Self::hydrate_entity(events)?;

                    self.execute_post_persist_hook(op, &entity, entity.events().last_persisted(n_events)).await?;
                    res.push(entity);
                }

                Ok(res)
            }
        };

        assert_eq!(tokens.to_string(), expected.to_string());
    }
}

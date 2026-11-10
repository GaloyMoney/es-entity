use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

/// `create_all` / `create_all_in_op`.
///
/// SQLite: there is no `UNNEST`, so instead of one batch statement each new
/// entity goes through `create_in_op`, all inside one savepoint. The
/// savepoint keeps the batch all-or-nothing: a failed SQLite statement does
/// not abort the surrounding transaction the way a Postgres one does, so
/// without it a caller that handled the error and committed would keep the
/// entities created before the failure.
pub struct CreateAllFn<'a> {
    in_op_only: bool,
    entity: &'a syn::Ident,
    /// `RepositoryOptions::create_can_reject`: the error is
    /// `RepoWriteError<{Entity}ConstraintViolation>`.
    can_reject: bool,
    /// The generated signatures' error type, see
    /// `RepositoryOptions::create_error_ty`.
    write_error_ty: TokenStream,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> From<&'a RepositoryOptions> for CreateAllFn<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            write_error_ty: opts.create_error_ty(),
            can_reject: opts.create_can_reject(),
            in_op_only: opts.in_op_only(),
            entity: opts.entity(),
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }
}

impl ToTokens for CreateAllFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let entity = self.entity;
        let write_error_ty = &self.write_error_ty;
        // Rejections carry no attempted values, as from the Postgres batch
        // statement (see `__without_attempted`).
        let clear_attempted = self.can_reject.then(|| {
            quote! {
                .map_rejected(|r| r.__without_attempted())
            }
        });
        let rejection_import = self
            .can_reject
            .then(|| quote! { use es_entity::errlanes::ResultExt as _; });

        #[cfg(feature = "instrument")]
        let (instrument_attr, error_recording) = {
            let entity_name = entity.to_string();
            let repo_name = &self.repo_name_snake;
            let span_name = format!("{}.create_all", repo_name);
            (
                quote! {
                    #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, count = new_entities.len(), error = tracing::field::Empty, exception.message = tracing::field::Empty, exception.type = tracing::field::Empty))]
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
        let (instrument_attr, error_recording) = (quote! {}, quote! {});

        let standalone = (!self.in_op_only).then(|| {
            quote! {
                pub async fn create_all(
                    &self,
                    new_entities: Vec<<#entity as es_entity::EsEntity>::New>
                ) -> Result<Vec<#entity>, #write_error_ty> {
                    let mut op = self.begin_op().await?;
                    let res = self.create_all_in_op(&mut op, new_entities).await?;
                    op.commit().await?;
                    Ok(res)
                }
            }
        });

        tokens.append_all(quote! {
            #standalone

            #instrument_attr
            pub async fn create_all_in_op<OP>(
                &self,
                op: &mut OP,
                new_entities: Vec<<#entity as es_entity::EsEntity>::New>
            ) -> Result<Vec<#entity>, #write_error_ty>
            where
                OP: es_entity::AtomicOperation + ?Sized
            {
                let __result: Result<Vec<#entity>, #write_error_ty> = async {
                    #rejection_import
                    let mut entities = Vec::with_capacity(new_entities.len());
                    if new_entities.is_empty() {
                        return Ok(entities);
                    }

                    let mut __savepoint =
                        es_entity::SavepointOperation::begin_savepoint(&mut *op).await?;
                    for new_entity in new_entities {
                        match self.create_in_op(&mut __savepoint, new_entity).await {
                            Ok(entity) => entities.push(entity),
                            Err(e) => {
                                __savepoint.rollback().await?;
                                return Err(e) #clear_attempted;
                            }
                        }
                    }
                    __savepoint.release().await?;

                    Ok(entities)
                }.await;

                #error_recording
                __result
            }
        });
    }
}

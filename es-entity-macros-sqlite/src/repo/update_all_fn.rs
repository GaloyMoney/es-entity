use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};

use super::options::*;

/// `update_all` / `update_all_in_op` / `update_all_mut_in_op`.
///
/// SQLite: there is no `UNNEST`, so each entity goes through `update_in_op`
/// (nested children, snapshots and hooks included), all inside one savepoint
/// so the batch stays all-or-nothing even when the caller handles the error
/// and keeps using the transaction (see `create_all_fn`).
pub struct UpdateAllFn<'a> {
    in_op_only: bool,
    entity: &'a syn::Ident,
    /// The generated signatures' error type (and the inner `__result`
    /// annotation) of `update_all`/`update_all_in_op`/`update_all_mut_in_op`,
    /// see `RepositoryOptions::update_error_ty`.
    write_error_ty: TokenStream,
    #[cfg(feature = "instrument")]
    repo_name_snake: String,
}

impl<'a> From<&'a RepositoryOptions> for UpdateAllFn<'a> {
    fn from(opts: &'a RepositoryOptions) -> Self {
        Self {
            in_op_only: opts.in_op_only(),
            entity: opts.entity(),
            write_error_ty: opts.update_error_ty(),
            #[cfg(feature = "instrument")]
            repo_name_snake: opts.repo_name_snake_case(),
        }
    }
}

/// Which shape of parent collection a generated bulk-update function
/// operates on: the caller-facing `&mut [Entity]`, or the scattered
/// `&mut Entity` borrows nested children are gathered into.
enum BatchMode {
    OwnedSlice,
    RefVec,
}

impl ToTokens for UpdateAllFn<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        tokens.append_all(self.build(BatchMode::OwnedSlice));
        tokens.append_all(self.build(BatchMode::RefVec));
    }
}

impl UpdateAllFn<'_> {
    fn build(&self, mode: BatchMode) -> TokenStream {
        let entity = self.entity;
        let write_error_ty = &self.write_error_ty;

        let (fn_name, entities_param, entities_prelude, iter_mut_ref) = match mode {
            BatchMode::OwnedSlice => (
                quote::format_ident!("update_all_in_op"),
                quote! { entities: &mut [#entity] },
                None,
                quote! { entities.iter_mut() },
            ),
            BatchMode::RefVec => (
                quote::format_ident!("update_all_mut_in_op"),
                quote! { entities: impl IntoIterator<Item = &mut #entity> },
                Some(quote! {
                    let mut entities: Vec<&mut #entity> = entities.into_iter().collect();
                }),
                quote! { entities.iter_mut().map(|e| &mut **e) },
            ),
        };

        #[cfg(feature = "instrument")]
        let (instrument_attr, error_recording, count_recording) = {
            let entity_name = entity.to_string();
            let repo_name = &self.repo_name_snake;
            let span_suffix = match mode {
                BatchMode::OwnedSlice => "update_all",
                BatchMode::RefVec => "update_all_mut",
            };
            let span_name = format!("{}.{}", repo_name, span_suffix);
            let count_field = match mode {
                BatchMode::OwnedSlice => quote! { count = entities.len(), },
                BatchMode::RefVec => quote! { count = tracing::field::Empty, },
            };
            let count_recording = matches!(mode, BatchMode::RefVec).then(|| {
                quote! {
                    tracing::Span::current().record("count", entities.len());
                }
            });
            (
                quote! {
                    #[tracing::instrument(name = #span_name, skip_all, fields(entity = #entity_name, #count_field error = tracing::field::Empty, exception.message = tracing::field::Empty, exception.type = tracing::field::Empty))]
                },
                quote! {
                    if let Err(ref e) = __result {
                        tracing::Span::current().record("error", true);
                        tracing::Span::current().record("exception.message", tracing::field::display(e));
                        tracing::Span::current().record("exception.type", std::any::type_name_of_val(e));
                    }
                },
                count_recording,
            )
        };
        #[cfg(not(feature = "instrument"))]
        let (instrument_attr, error_recording, count_recording) =
            (quote! {}, quote! {}, None::<TokenStream>);

        let standalone_wrapper =
            (matches!(mode, BatchMode::OwnedSlice) && !self.in_op_only).then(|| {
                quote! {
                    pub async fn update_all(
                        &self,
                        entities: &mut [#entity]
                    ) -> Result<usize, #write_error_ty> {
                        let mut op = self.begin_op().await?;
                        let res = self.update_all_in_op(&mut op, entities).await?;
                        op.commit().await?;
                        Ok(res)
                    }
                }
            });

        quote! {
            #standalone_wrapper

            #instrument_attr
            pub async fn #fn_name<OP>(
                &self,
                op: &mut OP,
                #entities_param
            ) -> Result<usize, #write_error_ty>
            where
                OP: es_entity::AtomicOperation + ?Sized
            {
                let __result: Result<usize, #write_error_ty> = async {
                    #entities_prelude
                    #count_recording

                    if entities.is_empty() {
                        return Ok(0);
                    }

                    let mut __savepoint =
                        es_entity::SavepointOperation::begin_savepoint(&mut *op).await?;
                    let mut total_events = 0usize;
                    for entity in #iter_mut_ref {
                        match self.update_in_op(&mut __savepoint, entity).await {
                            Ok(n_events) => total_events += n_events,
                            Err(e) => {
                                __savepoint.rollback().await?;
                                return Err(e);
                            }
                        }
                    }
                    __savepoint.release().await?;

                    Ok(total_events)
                }.await;

                #error_recording
                __result
            }
        }
    }
}

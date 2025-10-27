use darling::{FromMeta, ast::NestedMeta};
use syn::ItemFn;

#[derive(FromMeta)]
struct MacroArgs {
    any_error: Option<bool>,
    max_retries: Option<u32>,
}

pub fn make(
    args: proc_macro::TokenStream,
    input: ItemFn,
) -> darling::Result<proc_macro2::TokenStream> {
    let attr_args = NestedMeta::parse_meta_list(args.into())?;
    let args = MacroArgs::from_list(&attr_args)?;

    let mut inner_fn = input.clone();
    let inner_ident = syn::Ident::new(
        &format!("{}_exec_one", &input.sig.ident),
        input.sig.ident.span(),
    );
    inner_fn.sig.ident = inner_ident.clone();
    inner_fn.vis = syn::Visibility::Inherited;
    // Keep user-provided attributes (like #[instrument]) on the inner function
    // inner_fn.attrs is preserved

    // Filter out tracing-related attributes for the outer function
    // (they should only be on the inner function)
    let outer_attrs: Vec<_> = input
        .attrs
        .iter()
        .filter(|attr| {
            // Keep non-tracing attributes on outer function
            !attr.path().is_ident("instrument")
                && !(attr.path().segments.len() == 2
                    && attr.path().segments[0].ident == "tracing"
                    && attr.path().segments[1].ident == "instrument")
        })
        .collect();

    let vis = &input.vis;
    let sig = &input.sig;

    let any_error = args.any_error.unwrap_or(false);

    #[cfg(feature = "instrument")]
    let err_match = if any_error {
        quote::quote! {
            if result.is_err() {
                tracing::warn!(
                    attempt = n,
                    max_retries = max_retries,
                    "Error detected, retrying"
                );
                continue;
            }
        }
    } else {
        quote::quote! {
            if let Err(e) = result.as_ref() {
                if e.was_concurrent_modification() {
                    tracing::warn!(
                        attempt = n,
                        max_retries = max_retries,
                        "Concurrent modification detected, retrying"
                    );
                    continue;
                }
            }
        }
    };

    #[cfg(not(feature = "instrument"))]
    let err_match = if any_error {
        quote::quote! {
            if result.is_err() {
                continue;
            }
        }
    } else {
        quote::quote! {
            if let Err(e) = result.as_ref() {
                if e.was_concurrent_modification() {
                    continue;
                }
            }
        }
    };

    let inputs: Vec<_> = input
        .sig
        .inputs
        .iter()
        .filter_map(|input| match input {
            syn::FnArg::Receiver(_) => None,
            syn::FnArg::Typed(pat_type) => Some(&pat_type.pat),
        })
        .collect();

    let max_retries = args.max_retries.unwrap_or(3);

    #[cfg(feature = "instrument")]
    let outer_fn = {
        let fn_name = input.sig.ident.to_string();
        let retry_span_name = format!("{}.retry_wrapper", fn_name);

        quote::quote! {
            #( #outer_attrs )*
            #[tracing::instrument(
                name = #retry_span_name,
                skip_all,
                fields(
                    max_retries = #max_retries,
                    attempt = tracing::field::Empty,
                    retried = false
                )
            )]
            #vis #sig {
                let max_retries = #max_retries;
                for n in 1..=max_retries {
                    tracing::Span::current().record("attempt", n);
                    if n > 1 {
                        tracing::Span::current().record("retried", true);
                    }

                    let result = self.#inner_ident(#(#inputs),*).await;
                    if n == max_retries {
                        return result;
                    }
                    #err_match
                    return result;
                }
                unreachable!();
            }
        }
    };

    #[cfg(not(feature = "instrument"))]
    let outer_fn = {
        quote::quote! {
            #( #outer_attrs )*
            #vis #sig {
                let max_retries = #max_retries;
                for n in 1..=max_retries {
                    let result = self.#inner_ident(#(#inputs),*).await;
                    if n == max_retries {
                        return result;
                    }
                    #err_match
                    return result;
                }
                unreachable!();
            }
        }
    };

    let output = quote::quote! {
        #inner_fn
        #outer_fn
    };
    Ok(output)
}

// Its working - just need to figure out how to parse the attribute args for testing

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use syn::parse_quote;

//     #[test]
//     fn retry_on_concurrent_modification() {
//         let input = parse_quote! {
//             #[retry_on_concurrent_modification]
//             #[instrument(name = "test")]
//             pub async fn test(&self, a: u32) -> Result<(), es_entity::EsRepoError> {
//                 self.repo.update().await?;
//                 Ok(())
//             }
//         };

//         let output = make(input).unwrap();
//         let expected = quote::quote! {
//             async fn test_exec_one(&self, a: u32) -> Result<(), es_entity::EsRepoError> {
//                 self.repo.update().await?;
//                 Ok(())
//             }

//             #[retry_on_concurrent_modification]
//             #[instrument(name = "test")]
//             pub async fn test(&self, a: u32) -> Result<(), es_entity::EsRepoError> {
//                 let max_retries = 3;
//                 for n in 1..=max_retries {
//                     let result = self.test_exec_one(a).await;
//                     if n == max_retries {
//                         return result;
//                     }
//                     if let Err(e) = result.as_ref() {
//                         if e.was_concurrent_modification() {
//                             continue;
//                         }
//                     }
//                     return result;
//                 }
//                 unreachable!();
//             }
//         };
//         assert_eq!(output.to_string(), expected.to_string());
//     }
// }

use proc_macro2::TokenStream as TokenStream2;
use syn::{Ident, ItemFn, Token, parse::Parse, parse::ParseStream, punctuated::Punctuated};

struct MacroArgs {
    args: Vec<Ident>,
}

impl Parse for MacroArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let args = Punctuated::<Ident, Token![,]>::parse_terminated(input)?;
        Ok(MacroArgs {
            args: args.into_iter().collect(),
        })
    }
}

// Wrapper for the proc macro that converts between TokenStream types
pub fn make(
    args: proc_macro::TokenStream,
    input: ItemFn,
) -> darling::Result<proc_macro2::TokenStream> {
    make_internal(args.into(), input)
}

pub fn make_internal(args: TokenStream2, input: ItemFn) -> darling::Result<TokenStream2> {
    let macro_args: MacroArgs =
        syn::parse2(args).map_err(|e| darling::Error::custom(e.to_string()))?;

    let ItemFn {
        attrs,
        vis,
        sig,
        block,
    } = input;

    let is_async = sig.asyncness.is_some();

    let insert_stmts: Vec<_> = macro_args
        .args
        .iter()
        .map(|arg| {
            let arg_name = arg.to_string();
            quote::quote! {
                let _ = ctx.insert(#arg_name, &#arg);
            }
        })
        .collect();

    let inserts = if !insert_stmts.is_empty() {
        quote::quote! {
            {
                let mut ctx = es_entity::context::EventContext::current();
                #(#insert_stmts)*
            }
        }
    } else {
        quote::quote! {}
    };

    let wrapped_body = if is_async {
        quote::quote! {
            use es_entity::context::WithEventContext;
            let data = es_entity::context::EventContext::current().data();
            async {
                #inserts
                #block
            }.with_event_context(data).await
        }
    } else {
        quote::quote! {
            let __es_event_context_guard = es_entity::context::EventContext::fork();
            #inserts
            #block
        }
    };

    Ok(quote::quote! {
        #(#attrs)*
        #vis #sig {
            #wrapped_body
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;
    use syn::parse_quote;

    #[test]
    fn no_async_no_args() {
        let input: ItemFn = parse_quote! {
            pub fn no_async_no_args(&self, a: u32) {
                unimplemented!()
            }
        };

        // Create empty args
        let args = TokenStream2::new();

        let output = make_internal(args, input).unwrap();

        let expected = quote! {
            pub fn no_async_no_args(&self, a: u32) {
                let __es_event_context_guard = es_entity::context::EventContext::fork();
                {
                    unimplemented!()
                }
            }
        };

        assert_eq!(output.to_string(), expected.to_string());
    }

    #[test]
    fn no_async_with_args() {
        let input: ItemFn = parse_quote! {
            pub fn no_async_with_args(&self, arg_one: u32, arg_two: u64) {
                unimplemented!()
            }
        };

        // Create args with some parameters
        let args = quote! { arg_one, arg_two };

        let output = make_internal(args, input).unwrap();

        let expected = quote! {
            pub fn no_async_with_args(&self, arg_one: u32, arg_two: u64) {
                let __es_event_context_guard = es_entity::context::EventContext::fork();
                {
                    let mut ctx = es_entity::context::EventContext::current();
                    let _ = ctx.insert("arg_one", &arg_one);
                    let _ = ctx.insert("arg_two", &arg_two);
                }
                {
                    unimplemented!()
                }
            }
        };

        assert_eq!(output.to_string(), expected.to_string());
    }

    #[test]
    fn async_no_args() {
        let input: ItemFn = parse_quote! {
            pub async fn async_no_args(&self, a: u32) {
                unimplemented!()
            }
        };

        // Create empty args
        let args = TokenStream2::new();

        let output = make_internal(args, input).unwrap();

        let expected = quote! {
            pub async fn async_no_args(&self, a: u32) {
                use es_entity::context::WithEventContext;
                let data = es_entity::context::EventContext::current().data();
                async {
                    {
                        unimplemented!()
                    }
                }.with_event_context(data).await
            }
        };

        assert_eq!(output.to_string(), expected.to_string());
    }

    #[test]
    fn async_with_args() {
        let input: ItemFn = parse_quote! {
            pub async fn async_with_args(&self, arg_one: u32, arg_two: u64) {
                unimplemented!()
            }
        };

        // Create args with some parameters
        let args = quote! { arg_one, arg_two };

        let output = make_internal(args, input).unwrap();

        let expected = quote! {
            pub async fn async_with_args(&self, arg_one: u32, arg_two: u64) {
                use es_entity::context::WithEventContext;
                let data = es_entity::context::EventContext::current().data();
                async {
                    {
                        let mut ctx = es_entity::context::EventContext::current();
                        let _ = ctx.insert("arg_one", &arg_one);
                        let _ = ctx.insert("arg_two", &arg_two);
                    }
                    {
                        unimplemented!()
                    }
                }.with_event_context(data).await
            }
        };

        assert_eq!(output.to_string(), expected.to_string());
    }
}

use std::collections::HashSet;

use proc_macro2::{Delimiter, Group, TokenStream, TokenTree};
use quote::{format_ident, quote};
use syn::ItemFn;

/// The six span fields [`crate`]'s runtime `errlanes::FIELDS` promises;
/// declared here as literal dotted names so the generated `fields(..)` entries
/// match it token for token.
const FIELDS: &[&str] = &[
    "error",
    "error.lane",
    "error.code",
    "error.level",
    "exception.message",
    "exception.type",
];

/// `tracing::field::Empty` is a bare path, not run through `runtime_path()`:
/// a consumer of `#[errlanes::instrument]` has `tracing` itself as a direct
/// dependency by contract, the same way it must for `#[tracing::instrument]`
/// to be in scope at all.
fn field_entry(name: &str) -> TokenStream {
    let segments: Vec<_> = name.split('.').map(|s| format_ident!("{s}")).collect();
    quote! { #(#segments).* = tracing::field::Empty }
}

/// The dotted name an existing `fields(..)` entry declares, ignoring a
/// leading `%`/`?` display sigil and stopping at `=` (or at the entry's end,
/// for a shorthand `name` entry). Entries are matched on these exact name
/// tokens, not on any tracing field grammar beyond that.
fn entry_name(tokens: &[TokenTree]) -> String {
    let mut name = String::new();
    for token in tokens {
        match token {
            TokenTree::Punct(p)
                if name.is_empty() && (p.as_char() == '%' || p.as_char() == '?') => {}
            TokenTree::Ident(ident) => name.push_str(&ident.to_string()),
            TokenTree::Punct(p) if p.as_char() == '.' => name.push('.'),
            TokenTree::Punct(p) if p.as_char() == '=' => break,
            _ => break,
        }
    }
    name
}

/// Top-level (not inside a nested group) comma-separated entries of a
/// `fields(..)` group's contents.
fn split_entries(tokens: TokenStream) -> Vec<Vec<TokenTree>> {
    let mut entries = Vec::new();
    let mut current = Vec::new();
    for token in tokens {
        match &token {
            TokenTree::Punct(p) if p.as_char() == ',' => {
                entries.push(std::mem::take(&mut current));
            }
            _ => current.push(token),
        }
    }
    if !current.is_empty() {
        entries.push(current);
    }
    entries
}

fn declared_names(fields_group: TokenStream) -> HashSet<String> {
    split_entries(fields_group)
        .iter()
        .map(|entry| entry_name(entry))
        .filter(|name| !name.is_empty())
        .collect()
}

/// Whether a token stream already ends in a trailing comma — rustfmt adds one
/// to a multi-line `#[errlanes::instrument(..)]` argument list, or a
/// multi-line `fields(..)` group, and appending another unconditionally would
/// produce `,,` and fail to compile.
fn ends_with_comma(stream: &TokenStream) -> bool {
    matches!(
        stream.clone().into_iter().last(),
        Some(TokenTree::Punct(p)) if p.as_char() == ','
    )
}

/// Appends the [`FIELDS`] entries the caller has not already declared to the
/// `#[tracing::instrument]` argument list: into an existing `fields(..)`
/// group if there is one, else as a new one. The caller's own tokens are
/// otherwise passed through verbatim — this never parses tracing's field
/// grammar beyond recognising a top-level `fields` ident followed by a
/// parenthesised group.
fn inject_fields(args: TokenStream) -> TokenStream {
    let tokens: Vec<TokenTree> = args.into_iter().collect();
    let mut output = TokenStream::new();
    let mut found = false;
    let mut i = 0;
    while i < tokens.len() {
        if let TokenTree::Ident(ident) = &tokens[i]
            && ident == "fields"
            && let Some(TokenTree::Group(group)) = tokens.get(i + 1)
            && group.delimiter() == Delimiter::Parenthesis
        {
            found = true;
            let declared = declared_names(group.stream());
            let missing: Vec<_> = FIELDS
                .iter()
                .filter(|name| !declared.contains(**name))
                .map(|name| field_entry(name))
                .collect();
            let mut stream = group.stream();
            if !missing.is_empty() {
                if !stream.is_empty() && !ends_with_comma(&stream) {
                    stream.extend(quote! { , });
                }
                stream.extend(quote! { #(#missing),* });
            }
            let mut rewritten = Group::new(Delimiter::Parenthesis, stream);
            rewritten.set_span(group.span());
            output.extend([tokens[i].clone(), TokenTree::Group(rewritten)]);
            i += 2;
            continue;
        }
        output.extend([tokens[i].clone()]);
        i += 1;
    }
    if !found {
        if !output.is_empty() && !ends_with_comma(&output) {
            output.extend(quote! { , });
        }
        let entries: Vec<_> = FIELDS.iter().map(|name| field_entry(name)).collect();
        output.extend(quote! { fields(#(#entries),*) });
    }
    output
}

pub fn expand(args: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let func: ItemFn = syn::parse2(item)?;
    let ItemFn {
        attrs,
        vis,
        sig,
        block,
        ..
    } = func;

    // `return`/`?` inside an async block or a closure targets that block or
    // closure, not the enclosing fn, so either shape lets the original body
    // short-circuit while still leaving a `Result` behind to record.
    let wrapped = if sig.asyncness.is_some() {
        quote! {
            {
                let __errlanes_result = async move #block.await;
                if let Err(ref __errlanes_err) = __errlanes_result {
                    errlanes::Laned::record(__errlanes_err, &tracing::Span::current());
                }
                __errlanes_result
            }
        }
    } else {
        quote! {
            {
                let __errlanes_result = (move || #block)();
                if let Err(ref __errlanes_err) = __errlanes_result {
                    errlanes::Laned::record(__errlanes_err, &tracing::Span::current());
                }
                __errlanes_result
            }
        }
    };

    let tracing_args = inject_fields(args);

    Ok(quote! {
        #(#attrs)*
        #[tracing::instrument(#tracing_args)]
        #vis #sig #wrapped
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_missing_fields_to_an_existing_group() {
        let args: TokenStream = quote! { name = "x", fields(job_id = tracing::field::Empty) };
        let out = inject_fields(args).to_string();
        for name in FIELDS {
            assert!(
                out.contains(&name.replace('.', " . ")) || out.contains(name),
                "missing {name} in {out}"
            );
        }
        assert!(out.contains("job_id"));
    }

    #[test]
    fn adds_a_fields_group_when_absent() {
        let args: TokenStream = quote! { skip(self) };
        let out = inject_fields(args).to_string();
        assert!(out.contains("fields"));
        assert!(out.contains("error"));
    }

    #[test]
    fn no_fields_group_with_trailing_comma_does_not_double_comma() {
        // rustfmt adds a trailing comma to a multi-line argument list.
        let args: TokenStream = quote! { name = "x", skip_all, };
        let out = inject_fields(args).to_string();
        assert!(!out.contains(" , ,"), "double comma in {out}");
    }

    #[test]
    fn existing_fields_group_with_trailing_comma_does_not_double_comma() {
        // rustfmt adds a trailing comma to a multi-line `fields(..)` group too.
        let args: TokenStream = quote! { fields(job_id = tracing::field::Empty,) };
        let out = inject_fields(args).to_string();
        assert!(!out.contains(" , ,"), "double comma in {out}");
    }

    #[test]
    fn skips_a_field_the_caller_already_declared() {
        let args: TokenStream = quote! { fields(error = tracing::field::Empty) };
        let out = inject_fields(args);
        let declared = declared_names(
            if let Some(TokenTree::Group(g)) = out.clone().into_iter().nth(1) {
                g.stream()
            } else {
                panic!("expected a fields group")
            },
        );
        assert!(declared.contains("error"));
        // Exactly one `error` entry, not two: the caller's own plus ours would
        // otherwise both appear.
        let occurrences = out.to_string().matches("error =").count();
        assert_eq!(occurrences, 1, "{out}");
    }
}

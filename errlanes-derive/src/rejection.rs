use convert_case::{Case, Casing};
use darling::{FromDeriveInput, FromVariant, ast};
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Field, Ident, Path};

use crate::classify;
use crate::std_error::{self, DisplayDefault, Unit};

#[derive(Debug, FromDeriveInput)]
#[darling(attributes(rejection), supports(enum_any, struct_any))]
struct RejectionInput {
    ident: Ident,
    data: ast::Data<RejectionVariant, ()>,
    #[darling(default)]
    code_prefix: Option<String>,
    /// Required for a struct: there is no variant to carry a leaf code.
    #[darling(default)]
    code: Option<String>,
    #[darling(default)]
    level: Option<String>,
    /// Overrides the catalogue description, which otherwise is the
    /// `#[error("..")]` literal verbatim.
    #[darling(default)]
    description: Option<String>,
    /// Emits `From<Payload>`, with the payload also `source()`'s default.
    #[darling(default)]
    from: bool,
    /// `error = manual`: this type's `Display`/`Error` come from elsewhere
    /// (`thiserror`, or by hand); errlanes emits neither.
    #[darling(default)]
    error: Option<Path>,
}

impl RejectionInput {
    fn error_manual(&self) -> darling::Result<bool> {
        match &self.error {
            None => Ok(false),
            Some(p) if p.is_ident("manual") => Ok(true),
            Some(p) => Err(darling::Error::custom(
                "unknown `error` value, expected `error = manual`",
            )
            .with_span(p)),
        }
    }
}

fn level_expr(level: &Option<String>, span: &Ident) -> darling::Result<TokenStream> {
    Ok(match level.as_deref() {
        Some("trace") => quote! { errlanes::Level::Trace },
        Some("debug") => quote! { errlanes::Level::Debug },
        Some("info") | None => quote! { errlanes::Level::Info },
        Some("warn") => quote! { errlanes::Level::Warn },
        Some("error") => quote! { errlanes::Level::Error },
        Some(other) => {
            return Err(darling::Error::custom(format!(
                "unknown rejection level `{other}`, expected one of trace/debug/info/warn/error"
            ))
            .with_span(span));
        }
    })
}

#[derive(Debug, FromVariant)]
#[darling(attributes(rejection))]
struct RejectionVariant {
    ident: Ident,
    fields: ast::Fields<Field>,
    #[darling(default)]
    code: Option<String>,
    #[darling(default)]
    delegate: bool,
    #[darling(default)]
    code_and_level_from: Option<Path>,
    #[darling(default)]
    #[allow(dead_code)]
    origin: Option<String>,
    #[darling(default)]
    level: Option<String>,
    /// Leaf only: overrides the catalogue description, which otherwise is the
    /// `#[error("..")]` literal verbatim.
    #[darling(default)]
    description: Option<String>,
    /// Emits `From<Payload>`, with the payload also `source()`'s default.
    #[darling(default)]
    from: bool,
}

/// `Some("..")` / `None` as tokens for `CodeInfo::description`.
fn description_tokens(description: &Option<String>) -> TokenStream {
    match description {
        Some(d) => quote! { Some(#d) },
        None => quote! { None },
    }
}

/// §1.4: `description = ".."`, else the `#[error("..")]` literal verbatim
/// (placeholders and all), else nothing.
fn leaf_description(
    description: &Option<String>,
    error: &Option<std_error::ErrorSpec>,
) -> Option<String> {
    description
        .clone()
        .or_else(|| error.as_ref().map(|spec| spec.lit.value()))
}

impl RejectionVariant {
    fn is_delegating(&self) -> bool {
        self.delegate
    }

    fn delegate_ty(&self) -> Option<&syn::Type> {
        self.is_delegating().then(|| &self.fields.fields[0].ty)
    }

    fn level_expr(&self) -> TokenStream {
        match self.level.as_deref() {
            Some("trace") => quote! { errlanes::Level::Trace },
            Some("debug") => quote! { errlanes::Level::Debug },
            Some("info") | None => quote! { errlanes::Level::Info },
            Some("warn") => quote! { errlanes::Level::Warn },
            Some("error") => quote! { errlanes::Level::Error },
            Some(other) => {
                let msg = format!(
                    "unknown rejection level `{other}`, expected one of trace/debug/info/warn/error"
                );
                quote! { compile_error!(#msg) }
            }
        }
    }

    fn leaf_code(&self, prefix: &Option<String>) -> String {
        match &self.code {
            Some(c) => c.clone(),
            None => {
                let base = self.ident.to_string().to_case(Case::Constant);
                match prefix {
                    Some(p) => format!("{p}{base}"),
                    None => base,
                }
            }
        }
    }
}

pub fn derive(ast: &syn::DeriveInput) -> darling::Result<TokenStream> {
    let input = RejectionInput::from_derive_input(ast)?;
    let ident = &input.ident;
    let code_ident = quote::format_ident!("{}Code", ident);

    let variants = match &input.data {
        ast::Data::Enum(v) => v,
        ast::Data::Struct(_) => {
            // A struct is one outcome: no variant to carry a leaf code, no
            // composition schema (that is enum-only — a struct cannot be a
            // diamond's source or destination).
            let code = input.code.clone().ok_or_else(|| {
                darling::Error::custom(
                    "a rejection struct requires #[rejection(code = \"..\")]: there is no \
                     variant to derive one from",
                )
                .with_span(&input.ident)
            })?;
            let level = level_expr(&input.level, ident)?;
            let error_manual = input.error_manual()?;
            let syn::Data::Struct(raw) = &ast.data else {
                unreachable!()
            };

            // With `error = manual` the `#[error]` attribute belongs to
            // `thiserror`, not us: no literal to read.
            let error = if error_manual {
                None
            } else {
                std_error::take_error_lit(&ast.attrs).map_err(darling::Error::from)?
            };
            let description = description_tokens(&leaf_description(&input.description, &error));

            let mut out = TokenStream::new();
            let from_payload = if input.from {
                let payload =
                    classify::payload_field(&raw.fields, ident, "`#[rejection(from)]` on a struct")
                        .map_err(darling::Error::from)?;
                out.extend(
                    classify::from_impl(ident, &ast.generics, None, &payload, ident)
                        .map_err(darling::Error::from)?,
                );
                Some(payload)
            } else {
                None
            };

            out.extend(quote! {
                #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
                pub enum #code_ident { #ident }

                impl #code_ident {
                    pub const ALL: &'static [&'static str] = &[#code];
                }

                impl errlanes::RejectionCode for #code_ident {
                    const CODES: &'static [errlanes::CodeInfo] = &[errlanes::CodeInfo {
                        code: #code,
                        description: #description,
                    }];
                }

                impl std::fmt::Display for #code_ident {
                    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        f.write_str((*self).into())
                    }
                }

                impl From<#code_ident> for &'static str {
                    fn from(code: #code_ident) -> &'static str {
                        match code {
                            #code_ident::#ident => #code,
                        }
                    }
                }

                impl errlanes::Rejection for #ident {
                    type Code = #code_ident;

                    fn code(&self) -> #code_ident {
                        #code_ident::#ident
                    }

                    fn level(&self) -> errlanes::Level {
                        #level
                    }
                }
            });

            if !error_manual {
                std_error::reject_from_field_attr(&raw.fields).map_err(darling::Error::from)?;
                let source = match &from_payload {
                    Some(p) => {
                        let name = std_error::field_binding(&raw.fields, p.index);
                        Some(quote! { #name })
                    }
                    None => std_error::marked_source(&raw.fields, ident)
                        .map_err(darling::Error::from)?,
                };
                let pat = std_error::bind_pattern(quote! { Self }, &raw.fields);
                // `Rejection`'s own impl doesn't infer generic bounds the
                // way `Classify`'s does (that's `classify.rs`'s doing, not
                // this derive's), so the `extra` bounds `emit` returns
                // alongside its tokens have nowhere to go here.
                let (display_tokens, _extra) = std_error::emit(
                    ident,
                    &ast.generics,
                    DisplayDefault::Code,
                    &[Unit {
                        pat,
                        fields: &raw.fields,
                        error,
                        name: ident.clone(),
                        source,
                    }],
                )
                .map_err(darling::Error::from)?;
                out.extend(display_tokens);
            }

            return Ok(out);
        }
    };
    let schema = crate::composition::schema(ast).map_err(darling::Error::from)?;
    let error_manual = input.error_manual()?;

    let mut code_variants = Vec::new();
    let mut into_str_arms = Vec::new();
    let mut code_match_arms = Vec::new();
    let mut level_match_arms = Vec::new();
    let mut code_parts = Vec::new();
    let mut from_impls = TokenStream::new();
    let mut display_units = Vec::new();

    let syn::Data::Enum(raw) = &ast.data else {
        unreachable!()
    };
    let mut metadata = TokenStream::new();
    for (v, raw) in variants.iter().zip(&raw.variants) {
        if let Some(source) = &v.code_and_level_from {
            if source.segments.len() < 2 {
                return Err(darling::Error::custom(
                    "code_and_level_from requires a qualified source variant: Source::Variant",
                )
                .with_span(source));
            }
            if v.code.is_some() || v.level.is_some() || v.delegate {
                return Err(darling::Error::custom(
                    "code_and_level_from conflicts with code, level, and delegate; choose source metadata or local metadata",
                )
                .with_span(&v.ident));
            }
        }
        if v.delegate
            && (v.code.is_some()
                || v.level.is_some()
                || v.fields.style != ast::Style::Tuple
                || v.fields.fields.len() != 1)
        {
            return Err(darling::Error::custom(
                "delegate requires exactly one tuple field and conflicts with leaf code/level",
            )
            .with_span(&v.ident));
        }
        let variant_ident = &v.ident;
        let id = crate::composition::variant_id(&variant_ident.to_string());
        let fields: Vec<_> = v
            .fields
            .fields
            .iter()
            .enumerate()
            .map(|(i, f)| {
                f.ident
                    .clone()
                    .unwrap_or_else(|| quote::format_ident!("field_{i}"))
            })
            .collect();
        let types: Vec<_> = v.fields.fields.iter().map(|f| &f.ty).collect();
        let pat = match v.fields.style {
            ast::Style::Unit => quote!(Self::#variant_ident),
            ast::Style::Tuple => quote!(Self::#variant_ident(#(#fields),*)),
            ast::Style::Struct => quote!(Self::#variant_ident { #(#fields),* }),
        };

        let payload = if v.delegate || v.from {
            Some(
                classify::payload_field(
                    &raw.fields,
                    variant_ident,
                    if v.delegate {
                        "`delegate`"
                    } else {
                        "`#[rejection(from)]` on a variant"
                    },
                )
                .map_err(darling::Error::from)?,
            )
        } else {
            None
        };
        if v.from {
            from_impls.extend(
                classify::from_impl(
                    ident,
                    &ast.generics,
                    Some(variant_ident),
                    payload.as_ref().unwrap(),
                    variant_ident,
                )
                .map_err(darling::Error::from)?,
            );
        }
        // `error = manual` hands `#[error]` to `thiserror`: nothing to read.
        let error = if error_manual {
            None
        } else {
            std_error::reject_from_field_attr(&raw.fields).map_err(darling::Error::from)?;
            std_error::take_error_lit(&raw.attrs).map_err(darling::Error::from)?
        };
        if !error_manual {
            let source = match &payload {
                Some(p) => {
                    let name = std_error::field_binding(&raw.fields, p.index);
                    Some(quote! { #name })
                }
                None => std_error::marked_source(&raw.fields, variant_ident)
                    .map_err(darling::Error::from)?,
            };
            display_units.push(Unit {
                pat: pat.clone(),
                fields: &raw.fields,
                error: error.clone(),
                name: variant_ident.clone(),
                source,
            });
        }

        let mut code_and_level_from = v.code_and_level_from.clone();
        // A whole-value `#[lift(Payload)]` (one segment: a struct source, no
        // variant to take code and level from) identifies its metadata source the same
        // way `delegate` does — by the field's own `Rejection` impl, not by
        // a variant's `RejectionMetadata<ID>` — since there is no variant ID
        // to look one up by.
        let mut whole_value_source: Option<Path> = None;
        // Lift owns conversion generation. Its mapping also identifies the
        // default metadata source; no Lift implementation is required here.
        if code_and_level_from.is_none() && v.code.is_none() && !v.delegate {
            let mappings: Vec<_> = raw
                .attrs
                .iter()
                .filter(|a| a.path().is_ident("lift"))
                .map(|a| a.parse_args::<crate::lift::Mapping>())
                .collect::<syn::Result<_>>()
                .map_err(darling::Error::from)?;
            // Converted fields cannot stand in for the source variant's fields
            // when evaluating metadata, even if only the level is overridden.
            if mappings.iter().any(|mapping| mapping.into) {
                return Err(darling::Error::custom(
                    "an `into` lift requires an explicit rejection code or \
                     `#[rejection(delegate)]`",
                )
                .with_span(&v.ident));
            }
            if v.level.is_none() && mappings.len() > 1 {
                return Err(darling::Error::custom(
                    "multiple source variants require an explicit canonical rejection code",
                )
                .with_span(&v.ident));
            }
            if let Some(mapping) = mappings.into_iter().next() {
                // A `field =` projection, like a `with =` mapper, breaks the
                // assumption metadata forwarding relies on: that the
                // destination's own fields mirror the source variant's
                // fields exactly, so they can stand in for them when asking
                // the source `RejectionMetadata` for its code/level. A
                // projection keeps only the one named field, not the whole
                // source payload, so there is nothing of the right shape to
                // `code_and_level_from` with — same restriction, same message family.
                if v.level.is_none() {
                    if mapping.with.is_none() && mapping.field.is_none() {
                        if mapping.case.segments.len() >= 2 {
                            code_and_level_from = Some(mapping.case);
                        } else {
                            whole_value_source = Some(mapping.case);
                        }
                    } else {
                        return Err(darling::Error::custom(
                            "a payload mapper or a `field` projection requires an explicit \
                             rejection code",
                        )
                        .with_span(&v.ident));
                    }
                }
            }
        }
        let forwards_to = |target: &dyn std::fmt::Display| {
            darling::Error::custom(format!(
                "`description` belongs on the leaf that owns the code; this variant forwards \
                 to `{}`",
                target.to_string().replace(' ', "")
            ))
            .with_span(&v.ident)
        };
        let (code, level, variant_codes) = if let Some(mut source) = code_and_level_from {
            if v.description.is_some() {
                return Err(forwards_to(&quote!(#source)));
            }
            let source_variant = source.segments.pop().unwrap().ident;
            source.segments.pop_punct();
            let source_id = crate::composition::variant_id(&source_variant.to_string());
            code_variants.push(quote!(#variant_ident(<#source as errlanes::Rejection>::Code)));
            into_str_arms.push(quote!(#code_ident::#variant_ident(inner) => inner.into()));
            (
                quote!(#code_ident::#variant_ident(<#source as errlanes::RejectionMetadata<#source_id>>::field_code((#(#fields,)*)))),
                quote!(<#source as errlanes::RejectionMetadata<#source_id>>::field_level((#(#fields,)*))),
                quote!(<#source as errlanes::RejectionMetadata<#source_id>>::CODES),
            )
        } else if let Some(source) = &whole_value_source {
            if v.description.is_some() {
                return Err(forwards_to(&quote!(#source)));
            }
            let Some(inner) = fields.first() else {
                return Err(darling::Error::custom(
                    "a whole-value `#[lift(Source)]` (naming a registered source with no \
                     variant) needs exactly one field to hold the source value",
                )
                .with_span(&v.ident));
            };
            code_variants.push(quote!(#variant_ident(<#source as errlanes::Rejection>::Code)));
            into_str_arms.push(quote!(#code_ident::#variant_ident(inner) => inner.into()));
            (
                quote!(#code_ident::#variant_ident(<#source as errlanes::Rejection>::code(#inner))),
                quote!(<#source as errlanes::Rejection>::level(#inner)),
                quote!(<<#source as errlanes::Rejection>::Code as errlanes::RejectionCode>::CODES),
            )
        } else if let Some(inner_ty) = v.delegate_ty() {
            if v.description.is_some() {
                return Err(forwards_to(&quote!(#inner_ty)));
            }
            let inner = &fields[0];
            code_variants.push(quote!(#variant_ident(<#inner_ty as errlanes::Rejection>::Code)));
            into_str_arms.push(quote!(#code_ident::#variant_ident(inner) => inner.into()));
            (
                quote!(#code_ident::#variant_ident(<#inner_ty as errlanes::Rejection>::code(#inner))),
                quote!(<#inner_ty as errlanes::Rejection>::level(#inner)),
                quote!(<<#inner_ty as errlanes::Rejection>::Code as errlanes::RejectionCode>::CODES),
            )
        } else {
            code_variants.push(quote!(#variant_ident));
            let leaf = v.leaf_code(&input.code_prefix);
            into_str_arms.push(quote!(#code_ident::#variant_ident => #leaf));
            let description = description_tokens(&leaf_description(&v.description, &error));
            (
                quote!(#code_ident::#variant_ident),
                v.level_expr(),
                quote!(&[errlanes::CodeInfo { code: #leaf, description: #description }]),
            )
        };
        let part = quote!(<#ident as errlanes::RejectionMetadata<#id>>::CODES);
        code_parts.push(part);
        code_match_arms.push(quote!(#pat => #code));
        level_match_arms.push(quote!(#pat => #level));
        metadata.extend(quote! {
            impl errlanes::RejectionMetadata<#id> for #ident {
                type Fields<'a> = (#(&'a #types,)*);
                const CODES: &'static [errlanes::CodeInfo] = #variant_codes;
                #[allow(unused_variables, unused_assignments)]
                fn field_code(fields: <Self as errlanes::RejectionMetadata<#id>>::Fields<'_>) -> #code_ident {
                    let (#(#fields,)*) = fields;
                    #code
                }
                #[allow(unused_variables, unused_assignments)]
                fn field_level(fields: <Self as errlanes::RejectionMetadata<#id>>::Fields<'_>) -> errlanes::Level {
                    let (#(#fields,)*) = fields;
                    #level
                }
            }
        });
    }

    // Private helper consts, named after the (unique-in-module) type.
    let parts_ident = quote::format_ident!("__errlanes_code_parts_{}", ident);
    let len_ident = quote::format_ident!("__errlanes_code_len_{}", ident);
    let mut tokens = quote! {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum #code_ident {
            #(#code_variants),*
        }

        #[allow(non_upper_case_globals)]
        const #parts_ident: &[&[errlanes::CodeInfo]] = &[#(#code_parts),*];
        #[allow(non_upper_case_globals)]
        const #len_ident: usize = errlanes::__catalogue::catalogue_len(#parts_ident);

        impl errlanes::RejectionCode for #code_ident {
            const CODES: &'static [errlanes::CodeInfo] =
                &errlanes::__catalogue::catalogue::<#len_ident>(#parts_ident);
        }

        impl #code_ident {
            pub const ALL: &'static [&'static str] = &errlanes::__catalogue::codes_of::<#len_ident>(
                <#code_ident as errlanes::RejectionCode>::CODES,
            );
        }

        impl std::fmt::Display for #code_ident {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str((*self).into())
            }
        }

        impl From<#code_ident> for &'static str {
            fn from(code: #code_ident) -> &'static str {
                match code {
                    #(#into_str_arms),*
                }
            }
        }

        impl errlanes::Rejection for #ident {
            type Code = #code_ident;

            #[allow(unused_variables, unused_assignments)]
            fn code(&self) -> #code_ident {
                match self {
                    #(#code_match_arms),*
                }
            }

            #[allow(unused_variables, unused_assignments)]
            fn level(&self) -> errlanes::Level {
                match self {
                    #(#level_match_arms),*
                }
            }
        }
    };

    tokens.extend(metadata);
    tokens.extend(schema);
    tokens.extend(from_impls);
    if !error_manual {
        // See the sibling call site above: `Rejection`'s own impl doesn't
        // infer generic bounds, so `emit`'s returned `extra` bounds have
        // nowhere to go here.
        let (display_tokens, _extra) =
            std_error::emit(ident, &ast.generics, DisplayDefault::Code, &display_units)
                .map_err(darling::Error::from)?;
        tokens.extend(display_tokens);
    }
    Ok(tokens)
}

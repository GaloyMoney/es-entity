use darling::FromDeriveInput;
use quote::{ToTokens, quote};

#[derive(Debug, Clone, FromDeriveInput)]
#[darling(attributes(es_snapshot))]
struct EsSnapshotInput {
    ident: syn::Ident,
    #[darling(default)]
    version: i64,
}

/// FNV-1a 64, computed at macro-expansion time over the fingerprint input
/// string and embedded as a literal. No dependency needed for a 10-line hash.
fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET_BASIS;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

/// `version | field_ident:type_tokens | …` in declaration order, hashed with
/// FNV-1a 64 and cast to `i64`. `i64::MIN` is reserved as
/// `NO_SNAPSHOT_FINGERPRINT`, so a collision is nudged by one.
fn compute_fingerprint(version: i64, fields: &[(&syn::Ident, &syn::Type)]) -> i64 {
    let mut input = format!("{version}|");
    for (ident, ty) in fields {
        input.push_str(&format!("{ident}:{}|", ty.to_token_stream()));
    }
    let fingerprint = fnv1a64(input.as_bytes()) as i64;
    if fingerprint == i64::MIN {
        fingerprint.wrapping_add(1)
    } else {
        fingerprint
    }
}

pub fn derive(ast: syn::DeriveInput) -> darling::Result<proc_macro2::TokenStream> {
    let input = EsSnapshotInput::from_derive_input(&ast)?;
    let ident = &input.ident;

    let fields: Vec<(&syn::Ident, &syn::Type)> = match &ast.data {
        syn::Data::Struct(data) => data
            .fields
            .iter()
            .filter_map(|f| f.ident.as_ref().map(|i| (i, &f.ty)))
            .collect(),
        _ => {
            return Err(darling::Error::custom(
                "EsSnapshot can only be derived for structs with named fields",
            )
            .with_span(&ast.ident));
        }
    };

    let fingerprint = compute_fingerprint(input.version, &fields);

    let forgettable_fields: Vec<&syn::Ident> = fields
        .iter()
        .filter(|(_, ty)| crate::type_utils::is_forgettable_type(ty))
        .map(|(ident, _)| *ident)
        .collect();
    let has_forgettable = !forgettable_fields.is_empty();

    let forgettable_field_names: Vec<String> =
        forgettable_fields.iter().map(|f| f.to_string()).collect();

    let extract_inserts = forgettable_fields.iter().map(|f| {
        let field_name = f.to_string();
        quote! {
            if let Some(v) = self.#f.__extract_payload_value() {
                payload.insert(#field_name.to_string(), v);
            }
        }
    });

    let forget_assignments = forgettable_fields.iter().map(|f| {
        quote! { self.#f = es_entity::Forgettable::forgotten(); }
    });

    Ok(quote! {
        impl es_entity::EsSnapshot for #ident {
            const IS_SNAPSHOT: bool = true;
            const FINGERPRINT: i64 = #fingerprint;
            const HAS_FORGETTABLE_FIELDS: bool = #has_forgettable;
            const FORGETTABLE_JSON_FIELDS: &'static [&'static str] = &[
                #(#forgettable_field_names),*
            ];

            fn extract_forgettable_payloads(&self) -> Option<es_entity::prelude::serde_json::Value> {
                let mut payload = es_entity::prelude::serde_json::Map::new();
                #(#extract_inserts)*
                if payload.is_empty() { None } else { Some(payload.into()) }
            }

            fn forget_forgettable_payloads(&mut self) {
                #(#forget_assignments)*
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fingerprint_of(input: syn::DeriveInput) -> i64 {
        let out = derive(input).unwrap().to_string();
        let marker = "const FINGERPRINT : i64 = ";
        let start = out.find(marker).expect("fingerprint const present") + marker.len();
        let rest = &out[start..];
        let end = rest.find("i64").expect("i64 suffix present");
        rest[..end]
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .parse()
            .expect("valid i64 literal")
    }

    #[test]
    fn same_struct_same_fingerprint() {
        let a: syn::DeriveInput = syn::parse_quote! {
            struct MeterSnapshot { id: MeterId, count: u32 }
        };
        let b: syn::DeriveInput = syn::parse_quote! {
            struct MeterSnapshot { id: MeterId, count: u32 }
        };
        assert_eq!(fingerprint_of(a), fingerprint_of(b));
    }

    #[test]
    fn renamed_field_changes_fingerprint() {
        let a: syn::DeriveInput = syn::parse_quote! {
            struct MeterSnapshot { id: MeterId, count: u32 }
        };
        let b: syn::DeriveInput = syn::parse_quote! {
            struct MeterSnapshot { id: MeterId, total: u32 }
        };
        assert_ne!(fingerprint_of(a), fingerprint_of(b));
    }

    #[test]
    fn changed_type_changes_fingerprint() {
        let a: syn::DeriveInput = syn::parse_quote! {
            struct MeterSnapshot { id: MeterId, count: u32 }
        };
        let b: syn::DeriveInput = syn::parse_quote! {
            struct MeterSnapshot { id: MeterId, count: u64 }
        };
        assert_ne!(fingerprint_of(a), fingerprint_of(b));
    }

    #[test]
    fn changed_version_changes_fingerprint() {
        let a: syn::DeriveInput = syn::parse_quote! {
            #[es_snapshot(version = 1)]
            struct MeterSnapshot { id: MeterId, count: u32 }
        };
        let b: syn::DeriveInput = syn::parse_quote! {
            #[es_snapshot(version = 2)]
            struct MeterSnapshot { id: MeterId, count: u32 }
        };
        assert_ne!(fingerprint_of(a), fingerprint_of(b));
    }

    #[test]
    fn fingerprint_never_i64_min() {
        for version in 0..2000i64 {
            let input: syn::DeriveInput = syn::parse_quote! {
                struct S { id: MeterId }
            };
            let fields: Vec<(&syn::Ident, &syn::Type)> = match &input.data {
                syn::Data::Struct(data) => data
                    .fields
                    .iter()
                    .filter_map(|f| f.ident.as_ref().map(|i| (i, &f.ty)))
                    .collect(),
                _ => unreachable!(),
            };
            assert_ne!(compute_fingerprint(version, &fields), i64::MIN);
        }
    }

    #[test]
    fn detects_forgettable_fields() {
        let input: syn::DeriveInput = syn::parse_quote! {
            struct ContactSnapshot { id: ContactId, email: Forgettable<String>, changes: u32 }
        };
        let out = derive(input).unwrap().to_string();
        assert!(out.contains("HAS_FORGETTABLE_FIELDS : bool = true"));
        assert!(out.contains("\"email\""));
        assert!(!out.contains("\"changes\""));
    }

    #[test]
    fn no_forgettable_fields() {
        let input: syn::DeriveInput = syn::parse_quote! {
            struct MeterSnapshot { id: MeterId, count: u32 }
        };
        let out = derive(input).unwrap().to_string();
        assert!(out.contains("HAS_FORGETTABLE_FIELDS : bool = false"));
    }
}

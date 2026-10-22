use darling::{FromDeriveInput, FromField, ToTokens};
use proc_macro2::TokenStream;
use quote::{TokenStreamExt, quote};
use syn::{Type, ext::IdentExt};

#[derive(Debug, FromField)]
#[darling(attributes(es_entity))]
struct Field {
    ident: Option<syn::Ident>,
    ty: Type,
    #[darling(default)]
    events: bool,
    #[darling(default)]
    nested: bool,
    #[darling(default)]
    default: bool,
}

impl Field {
    fn option_inner_type(&self) -> Option<&Type> {
        if let Type::Path(type_path) = &self.ty
            && type_path.qself.is_none()
            && let Some(segment) = type_path.path.segments.last()
            && segment.ident == "Option"
            && let syn::PathArguments::AngleBracketed(generic_args) = &segment.arguments
            && let Some(syn::GenericArgument::Type(inner_type)) = generic_args.args.first()
        {
            return Some(inner_type);
        }
        None
    }

    fn defaults_when_unset(&self) -> bool {
        self.default || self.nested
    }

    fn is_events_field(&self) -> bool {
        self.events || self.ident.as_ref().is_some_and(|i| i == "events")
    }

    /// The `S` in `EntityEvents<E, S>`, or `es_entity::NoSnapshot` when the
    /// field is bare `EntityEvents<E>`.
    fn snapshot_type(&self) -> TokenStream {
        if let Type::Path(type_path) = &self.ty
            && let Some(segment) = type_path.path.segments.last()
            && segment.ident == "EntityEvents"
            && let syn::PathArguments::AngleBracketed(generic_args) = &segment.arguments
            && let Some(syn::GenericArgument::Type(snapshot_ty)) = generic_args.args.get(1)
        {
            return quote! { #snapshot_ty };
        }
        quote! { es_entity::NoSnapshot }
    }

    fn extract_nested_entity_type(&self) -> &Type {
        if let Type::Path(type_path) = &self.ty
            && let Some(segment) = type_path.path.segments.last()
            && segment.ident == "Nested"
            && let syn::PathArguments::AngleBracketed(generic_args) = &segment.arguments
            && let Some(syn::GenericArgument::Type(inner_type)) = generic_args.args.first()
        {
            return inner_type;
        }
        panic!("Field must be of type Nested<T>");
    }
}

#[derive(Debug, FromDeriveInput)]
#[darling(supports(struct_named), attributes(es_entity))]
pub struct EsEntity {
    ident: syn::Ident,
    vis: syn::Visibility,
    #[darling(default, rename = "new")]
    new_entity_ident: Option<syn::Ident>,
    #[darling(default, rename = "event")]
    event_ident: Option<syn::Ident>,
    data: darling::ast::Data<(), Field>,
}

impl EsEntity {
    fn find_events_field(&self) -> Option<&Field> {
        match &self.data {
            darling::ast::Data::Struct(fields) => {
                fields.iter().find(|field| field.is_events_field())
            }
            _ => None,
        }
    }

    fn nested_fields(&self) -> Vec<&Field> {
        match &self.data {
            darling::ast::Data::Struct(fields) => {
                fields.iter().filter(|field| field.nested).collect()
            }
            _ => Vec::new(),
        }
    }

    fn hydrated_fields(&self) -> Vec<&Field> {
        match &self.data {
            darling::ast::Data::Struct(fields) => fields
                .iter()
                .filter(|field| !field.is_events_field())
                .collect(),
            _ => Vec::new(),
        }
    }

    fn validate_hydrator(&self) -> darling::Result<()> {
        let mut errors = darling::Error::accumulator();
        let mut setters: Vec<(String, &syn::Ident)> = Vec::new();
        for field in self.hydrated_fields() {
            let Some(ident) = field.ident.as_ref() else {
                continue;
            };
            let name = ident.unraw().to_string();
            if name == "build" {
                errors.push(
                    darling::Error::custom(
                        "a field named `build` collides with the generated hydrator's `build`",
                    )
                    .with_span(ident),
                );
                continue;
            }
            let mut names = vec![name.clone()];
            if field.option_inner_type().is_some() {
                names.push(format!("maybe_{name}"));
            }
            for setter in names {
                if let Some((_, other)) = setters.iter().find(|(existing, _)| *existing == setter) {
                    errors.push(
                        darling::Error::custom(format!(
                            "hydrator setter `{setter}` for field `{name}` collides with the setter of field `{}`",
                            other.unraw()
                        ))
                        .with_span(ident),
                    );
                } else {
                    setters.push((setter, ident));
                }
            }
        }
        errors.finish()
    }

    fn hydrator_tokens(&self, event: &syn::Ident, snapshot: &TokenStream) -> TokenStream {
        let ident = &self.ident;
        let vis = &self.vis;
        let hydrator = syn::Ident::new(&format!("{ident}Hydrator"), ident.span());
        let events_field = self
            .find_events_field()
            .and_then(|f| f.ident.as_ref())
            .expect("Not ident on #[events]");
        let entity_name = ident.to_string();

        let fields = self.hydrated_fields();
        let mut members = Vec::new();
        let mut setters = Vec::new();
        let mut assignments = Vec::new();
        for field in fields {
            let name = field.ident.as_ref().expect("named field");
            let ty = &field.ty;
            let field_name = name.unraw().to_string();
            if let Some(inner) = field.option_inner_type() {
                let maybe = syn::Ident::new(&format!("maybe_{field_name}"), name.span());
                members.push(quote! { #name: #ty });
                setters.push(quote! {
                    #vis fn #name(&mut self, #name: #inner) -> &mut Self {
                        self.#name = Some(#name);
                        self
                    }
                    #vis fn #maybe(&mut self, #name: #ty) -> &mut Self {
                        self.#name = #name;
                        self
                    }
                });
                assignments.push(quote! { #name: self.#name });
            } else {
                members.push(quote! { #name: Option<#ty> });
                setters.push(quote! {
                    #vis fn #name(&mut self, #name: #ty) -> &mut Self {
                        self.#name = Some(#name);
                        self
                    }
                });
                if field.defaults_when_unset() {
                    assignments.push(quote! { #name: self.#name.unwrap_or_default() });
                } else {
                    assignments.push(quote! {
                        #name: self.#name.ok_or(es_entity::EntityHydrationError::MissingField {
                            entity: #entity_name,
                            field: #field_name,
                        })?
                    });
                }
            }
        }

        let hydrator_doc =
            format!("Accumulates the fields of a [`{ident}`] while its events are replayed.");
        let constructor_doc = format!("Starts an empty [`{hydrator}`].");

        quote! {
            #[doc = #hydrator_doc]
            #[derive(Default)]
            #[allow(dead_code)]
            #vis struct #hydrator {
                #(#members,)*
            }

            #[allow(dead_code)]
            impl #ident {
                #[doc = #constructor_doc]
                #vis fn hydrator() -> #hydrator {
                    #hydrator::default()
                }
            }

            #[allow(dead_code)]
            impl #hydrator {
                #(#setters)*

                /// Builds the entity, failing with `MissingField` for the first required
                /// field no event has set.
                #vis fn build(
                    self,
                    events: es_entity::EntityEvents<#event, #snapshot>,
                ) -> Result<#ident, es_entity::EntityHydrationError> {
                    Ok(#ident {
                        #(#assignments,)*
                        #events_field: events,
                    })
                }
            }
        }
    }
}

pub fn derive(ast: syn::DeriveInput) -> darling::Result<proc_macro2::TokenStream> {
    let entity = EsEntity::from_derive_input(&ast)?;
    entity.validate_hydrator()?;
    Ok(quote!(#entity))
}

impl ToTokens for EsEntity {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let ident = &self.ident;
        let events_field_def = self
            .find_events_field()
            .expect("Struct must have a field marked with #[es_entity(events)]");
        let events_field = events_field_def
            .ident
            .as_ref()
            .expect("Not ident on #[events]");
        let snapshot = events_field_def.snapshot_type();

        let event = self.event_ident.clone().unwrap_or_else(|| {
            syn::Ident::new(
                &format!("{}Event", self.ident),
                proc_macro2::Span::call_site(),
            )
        });
        let new = self.new_entity_ident.clone().unwrap_or_else(|| {
            syn::Ident::new(
                &format!("New{}", self.ident),
                proc_macro2::Span::call_site(),
            )
        });

        let nested = self.nested_fields().into_iter().map(|f| {
            let field = &f.ident;
            let ty = f.extract_nested_entity_type();
            quote! {
                impl es_entity::Parent<#ty> for #ident {
                    fn new_children_mut(&mut self) -> &mut Vec<<#ty as es_entity::EsEntity>::New> {
                        self.#field.new_entities_mut()
                    }

                    fn inject_children(&mut self, children: impl IntoIterator<Item = #ty>) {
                        self.#field.load(children)
                    }

                    fn iter_persisted_children_mut(
                        &mut self
                    ) -> std::collections::hash_map::ValuesMut<'_, <<#ty as EsEntity>::Event as EsEvent>::EntityId, #ty>
                    {
                        self.#field.iter_persisted_mut()
                    }
                }
            }
        });

        let hydrator = self.hydrator_tokens(&event, &snapshot);

        tokens.append_all(quote! {
            #hydrator

            impl es_entity::EsEntity for #ident {
                type Event = #event;
                type New = #new;
                type Snapshot = #snapshot;

                fn events_mut(&mut self) -> &mut es_entity::EntityEvents<#event, #snapshot> {
                    &mut self.#events_field
                }
                fn events(&self) -> &es_entity::EntityEvents<#event, #snapshot> {
                    &self.#events_field
                }
                fn __entity_id(&self) -> &<#event as es_entity::EsEvent>::EntityId {
                    &self.id
                }
            }

            #(#nested)*
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;
    use syn::parse_quote;

    #[test]
    fn test_derive_es_entity() {
        let input: syn::DeriveInput = parse_quote! {
            #[derive(EsEntity)]
            pub struct User {
                pub id: UserId,
                pub email: String,
                #[es_entity(events)]
                the_events: EntityEvents<UserEvent>
            }
        };

        let output = derive(input).unwrap();
        let expected = quote! {
            #[doc = "Accumulates the fields of a [`User`] while its events are replayed."]
            #[derive(Default)]
            #[allow(dead_code)]
            pub struct UserHydrator {
                id: Option<UserId>,
                email: Option<String>,
            }

            #[allow(dead_code)]
            impl User {
                #[doc = "Starts an empty [`UserHydrator`]."]
                pub fn hydrator() -> UserHydrator {
                    UserHydrator::default()
                }
            }

            #[allow(dead_code)]
            impl UserHydrator {
                pub fn id(&mut self, id: UserId) -> &mut Self {
                    self.id = Some(id);
                    self
                }
                pub fn email(&mut self, email: String) -> &mut Self {
                    self.email = Some(email);
                    self
                }

                /// Builds the entity, failing with `MissingField` for the first required
                /// field no event has set.
                pub fn build(
                    self,
                    events: es_entity::EntityEvents<UserEvent, es_entity::NoSnapshot>,
                ) -> Result<User, es_entity::EntityHydrationError> {
                    Ok(User {
                        id: self.id.ok_or(es_entity::EntityHydrationError::MissingField {
                            entity: "User",
                            field: "id",
                        })?,
                        email: self.email.ok_or(es_entity::EntityHydrationError::MissingField {
                            entity: "User",
                            field: "email",
                        })?,
                        the_events: events,
                    })
                }
            }

            impl es_entity::EsEntity for User {
                type Event = UserEvent;
                type New = NewUser;
                type Snapshot = es_entity::NoSnapshot;
                fn events_mut(&mut self) -> &mut es_entity::EntityEvents<UserEvent, es_entity::NoSnapshot> {
                    &mut self.the_events
                }
                fn events(&self) -> &es_entity::EntityEvents<UserEvent, es_entity::NoSnapshot> {
                    &self.the_events
                }
                fn __entity_id(&self) -> &<UserEvent as es_entity::EsEvent>::EntityId {
                    &self.id
                }
            }
        };

        assert_eq!(output.to_string(), expected.to_string());
    }

    #[test]
    fn test_derive_without_events_attr() {
        let input: syn::DeriveInput = parse_quote! {
            #[derive(EsEntity)]
            pub struct User {
                pub id: UserId,
                events: EntityEvents<UserEvent>
            }
        };

        let output = derive(input).unwrap();
        let expected = quote! {
            #[doc = "Accumulates the fields of a [`User`] while its events are replayed."]
            #[derive(Default)]
            #[allow(dead_code)]
            pub struct UserHydrator {
                id: Option<UserId>,
            }

            #[allow(dead_code)]
            impl User {
                #[doc = "Starts an empty [`UserHydrator`]."]
                pub fn hydrator() -> UserHydrator {
                    UserHydrator::default()
                }
            }

            #[allow(dead_code)]
            impl UserHydrator {
                pub fn id(&mut self, id: UserId) -> &mut Self {
                    self.id = Some(id);
                    self
                }

                /// Builds the entity, failing with `MissingField` for the first required
                /// field no event has set.
                pub fn build(
                    self,
                    events: es_entity::EntityEvents<UserEvent, es_entity::NoSnapshot>,
                ) -> Result<User, es_entity::EntityHydrationError> {
                    Ok(User {
                        id: self.id.ok_or(es_entity::EntityHydrationError::MissingField {
                            entity: "User",
                            field: "id",
                        })?,
                        events: events,
                    })
                }
            }

            impl es_entity::EsEntity for User {
                type Event = UserEvent;
                type New = NewUser;
                type Snapshot = es_entity::NoSnapshot;
                fn events_mut(&mut self) -> &mut es_entity::EntityEvents<UserEvent, es_entity::NoSnapshot> {
                    &mut self.events
                }
                fn events(&self) -> &es_entity::EntityEvents<UserEvent, es_entity::NoSnapshot> {
                    &self.events
                }
                fn __entity_id(&self) -> &<UserEvent as es_entity::EsEvent>::EntityId {
                    &self.id
                }
            }
        };

        assert_eq!(output.to_string(), expected.to_string());
    }

    #[test]
    fn test_derive_with_nested() {
        let input: syn::DeriveInput = parse_quote! {
            #[derive(EsEntity)]
            pub struct User {
                pub id: UserId,
                #[es_entity(nested)]
                children: Nested<ChildEntity>,
                events: EntityEvents<UserEvent>
            }
        };

        let output = derive(input).unwrap();
        let expected = quote! {
            #[doc = "Accumulates the fields of a [`User`] while its events are replayed."]
            #[derive(Default)]
            #[allow(dead_code)]
            pub struct UserHydrator {
                id: Option<UserId>,
                children: Option<Nested<ChildEntity> >,
            }

            #[allow(dead_code)]
            impl User {
                #[doc = "Starts an empty [`UserHydrator`]."]
                pub fn hydrator() -> UserHydrator {
                    UserHydrator::default()
                }
            }

            #[allow(dead_code)]
            impl UserHydrator {
                pub fn id(&mut self, id: UserId) -> &mut Self {
                    self.id = Some(id);
                    self
                }
                pub fn children(&mut self, children: Nested<ChildEntity>) -> &mut Self {
                    self.children = Some(children);
                    self
                }

                /// Builds the entity, failing with `MissingField` for the first required
                /// field no event has set.
                pub fn build(
                    self,
                    events: es_entity::EntityEvents<UserEvent, es_entity::NoSnapshot>,
                ) -> Result<User, es_entity::EntityHydrationError> {
                    Ok(User {
                        id: self.id.ok_or(es_entity::EntityHydrationError::MissingField {
                            entity: "User",
                            field: "id",
                        })?,
                        children: self.children.unwrap_or_default(),
                        events: events,
                    })
                }
            }

            impl es_entity::EsEntity for User {
                type Event = UserEvent;
                type New = NewUser;
                type Snapshot = es_entity::NoSnapshot;
                fn events_mut(&mut self) -> &mut es_entity::EntityEvents<UserEvent, es_entity::NoSnapshot> {
                    &mut self.events
                }
                fn events(&self) -> &es_entity::EntityEvents<UserEvent, es_entity::NoSnapshot> {
                    &self.events
                }
                fn __entity_id(&self) -> &<UserEvent as es_entity::EsEvent>::EntityId {
                    &self.id
                }
            }

            impl es_entity::Parent<ChildEntity> for User {
                fn new_children_mut(&mut self) -> &mut Vec<<ChildEntity as es_entity::EsEntity>::New> {
                    self.children.new_entities_mut()
                }

                fn inject_children(&mut self, children: impl IntoIterator<Item = ChildEntity>) {
                    self.children.load(children)
                }

                fn iter_persisted_children_mut(
                    &mut self
                ) -> std::collections::hash_map::ValuesMut<'_, <<ChildEntity as EsEntity>::Event as EsEvent>::EntityId, ChildEntity>
                {
                    self.children.iter_persisted_mut()
                }
            }
        };

        assert_eq!(output.to_string(), expected.to_string());
    }

    fn hydrator_section(input: syn::DeriveInput) -> String {
        let output = derive(input).unwrap().to_string();
        let end = output
            .find("impl es_entity :: EsEntity")
            .expect("EsEntity impl is emitted");
        output[..end].to_string()
    }

    #[test]
    fn test_hydrator_option_field_gets_two_setters_and_defaults_to_none() {
        let section = hydrator_section(parse_quote! {
            #[derive(EsEntity)]
            pub struct User {
                pub id: UserId,
                pub email: Option<String>,
                events: EntityEvents<UserEvent>
            }
        });

        let expected_members = quote! { email: Option<String>, }.to_string();
        let expected_setters = quote! {
            pub fn email(&mut self, email: String) -> &mut Self {
                self.email = Some(email);
                self
            }
            pub fn maybe_email(&mut self, email: Option<String>) -> &mut Self {
                self.email = email;
                self
            }
        }
        .to_string();
        let expected_assignment = quote! { email: self.email, }.to_string();
        assert!(section.contains(&expected_members), "{section}");
        assert!(section.contains(&expected_setters), "{section}");
        assert!(section.contains(&expected_assignment), "{section}");
    }

    #[test]
    fn test_hydrator_default_field_falls_back_to_default() {
        let section = hydrator_section(parse_quote! {
            #[derive(EsEntity)]
            pub struct User {
                pub id: UserId,
                #[es_entity(default)]
                pub tags: Vec<Tag>,
                events: EntityEvents<UserEvent>
            }
        });

        let expected_members = quote! { tags: Option<Vec<Tag> >, }.to_string();
        let expected_setter = quote! {
            pub fn tags(&mut self, tags: Vec<Tag>) -> &mut Self {
                self.tags = Some(tags);
                self
            }
        }
        .to_string();
        let expected_assignment = quote! { tags: self.tags.unwrap_or_default(), }.to_string();
        assert!(section.contains(&expected_members), "{section}");
        assert!(section.contains(&expected_setter), "{section}");
        assert!(section.contains(&expected_assignment), "{section}");
    }

    #[test]
    fn test_hydrator_follows_entity_visibility() {
        let section = hydrator_section(parse_quote! {
            #[derive(EsEntity)]
            struct User {
                id: UserId,
                events: EntityEvents<UserEvent>
            }
        });

        let expected_struct = quote! { struct UserHydrator { id: Option<UserId>, } }.to_string();
        let expected_constructor = quote! { fn hydrator() -> UserHydrator }.to_string();
        let expected_setter = quote! { fn id(&mut self, id: UserId) -> &mut Self }.to_string();
        assert!(section.contains(&expected_struct), "{section}");
        assert!(section.contains(&expected_constructor), "{section}");
        assert!(section.contains(&expected_setter), "{section}");
        assert!(section.contains("fn build (self"), "{section}");
        assert!(!section.contains("pub"), "{section}");

        let section = hydrator_section(parse_quote! {
            #[derive(EsEntity)]
            pub(crate) struct User {
                id: UserId,
                events: EntityEvents<UserEvent>
            }
        });
        let expected_constructor = quote! { pub (crate) fn hydrator() -> UserHydrator }.to_string();
        assert!(section.contains(&expected_constructor), "{section}");
    }

    #[test]
    fn test_hydrator_rejects_a_field_named_build() {
        let err = derive(parse_quote! {
            #[derive(EsEntity)]
            pub struct User {
                pub id: UserId,
                pub build: String,
                events: EntityEvents<UserEvent>
            }
        })
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("a field named `build` collides with the generated hydrator's `build`"),
            "{err}"
        );
    }

    #[test]
    fn test_hydrator_rejects_colliding_option_setters() {
        let err = derive(parse_quote! {
            #[derive(EsEntity)]
            pub struct User {
                pub id: UserId,
                pub email: Option<String>,
                pub maybe_email: String,
                events: EntityEvents<UserEvent>
            }
        })
        .unwrap_err();

        assert!(
            err.to_string().contains(
                "hydrator setter `maybe_email` for field `maybe_email` collides with the setter of field `email`"
            ),
            "{err}"
        );
    }
}

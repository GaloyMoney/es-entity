#![cfg_attr(feature = "fail-on-warnings", deny(warnings))]
#![cfg_attr(feature = "fail-on-warnings", deny(clippy::all))]
#![forbid(unsafe_code)]

mod entity;
mod es_event_context;
mod event;
mod index_catalog;
mod query;
mod repo;
mod snapshot;
mod type_utils;

use proc_macro::TokenStream;
use syn::parse_macro_input;

#[proc_macro_derive(EsEvent, attributes(es_event))]
pub fn es_event_derive(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as syn::DeriveInput);
    match event::derive(ast) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.write_errors().into(),
    }
}

/// Automatically captures function arguments into the event context.
///
/// This attribute macro wraps functions to automatically insert specified arguments
/// into the current `EventContext` (`es_entity::context::EventContext`), making them
/// available for audit trails when events are persisted.
///
/// # Behavior
///
/// - **For async functions**: Uses the `WithEventContext` trait
///   (`es_entity::context::WithEventContext`) to propagate context across async boundaries
/// - **For sync functions**: Uses `EventContext::fork()` (`es_entity::context::EventContext::fork`)
///   to create an isolated child context
///
/// # Syntax
///
/// ```rust,ignore
/// #[es_event_context]              // No arguments captured
/// #[es_event_context(arg1)]         // Capture single argument
/// #[es_event_context(arg1, arg2)]   // Capture multiple arguments
/// ```
///
/// # Examples
///
/// ## Async function with argument capture
/// ```rust,ignore
/// use es_entity_macros::es_event_context;
///
/// impl UserService {
///     #[es_event_context(user_id, operation)]
///     async fn update_user(&self, user_id: UserId, operation: &str, data: UserData) -> Result<()> {
///         // user_id and operation are automatically added to context
///         // They will be included when events are persisted
///         self.repo.update(data).await
///     }
/// }
/// ```
///
/// ## Sync function with context isolation
/// ```rust,ignore
/// use es_entity_macros::es_event_context;
///
/// impl Calculator {
///     #[es_event_context(transaction_id)]
///     fn process(&mut self, transaction_id: u64, amount: i64) {
///         // transaction_id is captured in an isolated context
///         // Parent context is restored when function exits
///         self.apply_transaction(amount);
///     }
/// }
/// ```
///
/// ## Manual context additions
/// ```rust,ignore
/// use es_entity_macros::es_event_context;
/// use es_entity::context::EventContext;
///
/// #[es_event_context(request_id)]
/// async fn handle_request(request_id: String, data: RequestData) {
///     // request_id is automatically captured
///     
///     // You can still manually add more context
///     let mut ctx = EventContext::current();
///     ctx.insert("timestamp", &chrono::Utc::now()).unwrap();
///     
///     process_data(data).await;
/// }
/// ```
///
/// # Context Keys
///
/// Arguments are captured using their parameter names as keys. For example,
/// `user_id: UserId` will be stored with key `"user_id"` in the context.
///
/// # See Also
///
/// - `EventContext` (`es_entity::context::EventContext`) - The context management system
/// - `WithEventContext` (`es_entity::context::WithEventContext`) - Async context propagation
/// - Event Context chapter in the book for complete usage patterns
#[proc_macro_attribute]
pub fn es_event_context(args: TokenStream, input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as syn::ItemFn);
    match es_event_context::make(args, ast) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.write_errors().into(),
    }
}

#[proc_macro_derive(EsEntity, attributes(es_entity))]
pub fn es_entity_derive(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as syn::DeriveInput);
    match entity::derive(ast) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.write_errors().into(),
    }
}

#[proc_macro_derive(EsSnapshot, attributes(es_snapshot))]
pub fn es_snapshot_derive(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as syn::DeriveInput);
    match snapshot::derive(ast) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.write_errors().into(),
    }
}

#[proc_macro_derive(EsRepo, attributes(es_repo))]
pub fn es_repo_derive(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as syn::DeriveInput);
    match repo::derive(ast) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.write_errors().into(),
    }
}

#[proc_macro]
#[doc(hidden)]
pub fn expand_es_query(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as query::QueryInput);
    match query::expand(input) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.write_errors().into(),
    }
}

/// Diagnostic accessors for the final, composed repository rejection enum.
/// `Display`/`Error` come from the composed enum's own `errlanes::Rejection`
/// derive (via each variant's `#[error("{0}")]`/`#[source]`), not from here.
#[proc_macro_derive(ConstraintRejection)]
pub fn constraint_rejection(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let ast = syn::parse_macro_input!(input as syn::DeriveInput);
    let ident = ast.ident;
    let syn::Data::Enum(data) = ast.data else {
        return quote::quote!(compile_error!("expected constraint enum");).into();
    };
    let variants: Vec<_> = data.variants.iter().map(|v| &v.ident).collect();
    quote::quote! {
        impl #ident {
            pub fn diagnostics(&self) -> &es_entity::ConstraintDiagnostics {
                match self { #(Self::#variants(conflict) => &conflict.diagnostics),* }
            }
            pub fn constraint_name(&self) -> &str { self.diagnostics().constraint }
            pub fn kind(&self) -> es_entity::ConstraintKind { self.diagnostics().kind }
            pub fn is_unique(&self) -> bool { self.kind() == es_entity::ConstraintKind::Unique }
            pub fn is_foreign_key(&self) -> bool { self.kind() == es_entity::ConstraintKind::ForeignKey }
            pub fn is_check(&self) -> bool { self.kind() == es_entity::ConstraintKind::Check }
        }
    }.into()
}

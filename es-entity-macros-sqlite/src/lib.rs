#![cfg_attr(feature = "fail-on-warnings", deny(warnings))]
#![cfg_attr(feature = "fail-on-warnings", deny(clippy::all))]
#![forbid(unsafe_code)]

mod entity;
mod es_event_context;
mod event;
mod query;
mod repo;
mod retry_on_concurrent_modification;

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

#[proc_macro_attribute]
pub fn retry_on_concurrent_modification(args: TokenStream, input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as syn::ItemFn);
    match retry_on_concurrent_modification::make(args, ast) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.write_errors().into(),
    }
}

/// Automatically captures function arguments into the event context.
///
/// This attribute macro wraps functions to automatically insert specified arguments
/// into the current [`EventContext`](es_entity::context::EventContext), making them
/// available for audit trails when events are persisted.
///
/// # Behavior
///
/// - **For async functions**: Uses the [`WithEventContext`](es_entity::context::WithEventContext)
///   trait to propagate context across async boundaries
/// - **For sync functions**: Uses [`EventContext::fork()`](es_entity::context::EventContext::fork)
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
/// - [`EventContext`](es_entity::context::EventContext) - The context management system
/// - [`WithEventContext`](es_entity::context::WithEventContext) - Async context propagation
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

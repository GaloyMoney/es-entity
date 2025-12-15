//! Handle idempotency in event-sourced systems.

/// Signals if a mutation is applied or was skipped.
///
/// Distinguishes between operations that were executed versus those that were
/// ignored due to idempotency checks.
/// The [`idempotency_guard`][crate::idempotency_guard] macro provides an easy way to do such checks.
///
/// # Examples
///
/// ```rust
/// use es_entity::{idempotency_guard, Idempotent};
/// pub enum UserEvent{
///     Initialized {id: u64, name: String},
///     NameUpdated {name: String}
/// }
///
/// pub struct User{
///     events: Vec<UserEvent>
/// }
///
/// impl User{
///     // This returns `Idempotent<T>` where T is the return value we get after the event is processed
///     pub fn update_name(&mut self, new_name: impl Into<String>) -> Idempotent<()>{
///         let name = new_name.into();
///         idempotency_guard!(
///             self.events.iter().rev(),
///             UserEvent::NameUpdated { name: existing_name } if existing_name == &name
///         );
///         self.events.push(UserEvent::NameUpdated{name});
///         Idempotent::Executed(())
///     }
/// }
///   
/// fn example(){
///     let mut user = User{ events: vec![] };
///     assert!(user.update_name("Alice").did_execute());
///     // updating "Alice" executes as no such event has been processed before.
///     // Signalled by returning `Idempotent::Executed(T)`, validated with `did_execute` helper method
///
///     assert!(user.update_name("Alice").was_already_applied());
///     // updating "Alice" again ignored because same event has been processed before.
///     // Signalled by returning `Idempotent::AlreadyApplied` early, validated with `was_already_applied` helper method
/// }
/// ```
#[must_use]
pub enum Idempotent<T> {
    // Signals if executed and returns T
    Executed(T),
    // Signals if ignored due to idempotency checks
    AlreadyApplied,
}

impl<T> Idempotent<T> {
    /// Returns true if the operation was ignored due to idempotency checks.
    pub fn was_already_applied(&self) -> bool {
        matches!(self, Idempotent::AlreadyApplied)
    }

    /// Returns true if the operation was executed.
    pub fn did_execute(&self) -> bool {
        matches!(self, Idempotent::Executed(_))
    }

    /// Unwraps the value if executed, panics if ignored.
    pub fn unwrap(self) -> T {
        match self {
            Idempotent::Executed(t) => t,
            Idempotent::AlreadyApplied => panic!("Idempotent::AlreadyApplied"),
        }
    }

    /// Unwraps the value if executed, panics with custom message if ignored.
    pub fn expect(self, msg: &str) -> T {
        match self {
            Idempotent::Executed(val) => val,
            Idempotent::AlreadyApplied => panic!("{}", msg),
        }
    }
}

/// Internal trait used by the [`idempotency_guard`][crate::idempotency_guard] macro.
///
/// This internal-only trait is implemented on [`idempotency_guard`][crate::idempotency_guard] for it to create
/// both `Idempotent<T>` and `Result<Idempotent<T>, E>` return types for returning `AlreadyApplied` variant.
pub trait FromAlreadyApplied {
    /// Creates a value representing an already applied idempotent operation.
    fn from_already_applied() -> Self;
}

impl<T> FromAlreadyApplied for Idempotent<T> {
    /// to handle `Idempotent<T>` return type
    fn from_already_applied() -> Self {
        Idempotent::AlreadyApplied
    }
}

impl<T, E> FromAlreadyApplied for Result<Idempotent<T>, E> {
    /// to handle `Result<Idempotent<T>, E>` return type
    fn from_already_applied() -> Self {
        Ok(Idempotent::AlreadyApplied)
    }
}

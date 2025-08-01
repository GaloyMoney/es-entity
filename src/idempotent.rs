/// Enum type for handling idempotent operations in event-sourced systems.
///
/// Distinguishes between operations that were executed versus those that were
/// ignored due to idempotency checks. Signals if a mutation is applied or was skipped.
/// The [crate::idempotency_guard] macro provides an easy way to do such checks.
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
///     // updating "ALice" executes as no such event has been processed before.
///     // Signalled by returning `Idempotent::Executed(T)`, validated with `did_execute` helper method
///
///     assert!(user.update_name("Alice").was_ignored());
///     // updating "ALice" again ignored because same event has been processed before.
///     // Signalled by returning `Idempotent::Ignored` early, validated with `was_ignored` helper method
/// }
/// ```
#[must_use]
pub enum Idempotent<T> {
    /// Signals if the idempotent mutation was executed and returned `T`
    Executed(T),
    /// Signals if the idempotent mutation was ignored
    Ignored,
}

impl<T> Idempotent<T> {
    pub fn was_ignored(&self) -> bool {
        matches!(self, Idempotent::Ignored)
    }

    pub fn did_execute(&self) -> bool {
        matches!(self, Idempotent::Executed(_))
    }

    pub fn unwrap(self) -> T {
        match self {
            Idempotent::Executed(t) => t,
            Idempotent::Ignored => panic!("Idempotent::Ignored"),
        }
    }

    pub fn expect(self, msg: &str) -> T {
        match self {
            Idempotent::Executed(val) => val,
            Idempotent::Ignored => panic!("{}", msg),
        }
    }
}

pub trait FromIdempotentIgnored {
    fn from_ignored() -> Self;
}

impl<T> FromIdempotentIgnored for Idempotent<T> {
    fn from_ignored() -> Self {
        Idempotent::Ignored
    }
}

impl<T, E> FromIdempotentIgnored for Result<Idempotent<T>, E> {
    fn from_ignored() -> Self {
        Ok(Idempotent::Ignored)
    }
}

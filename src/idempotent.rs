/// Enum type for handling idempotent operations in event-sourced systems.
///
/// Distinguishes between operations that were executed versus those that were
/// ignored due to idempotency checks. Prevents duplicate event processing by
/// signaling whether a mutation actually applied changes or was skipped.
/// The [crate::idempotency_guard] macro provides an easy way to do such checks.
///
/// # Examples
/// [See comprehensive usage examples][crate::idempotency_guard]

#[must_use]
pub enum Idempotent<T> {
    /// Signals if the idempotent mutation was executed and returnd `T`
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

//! Classifying a probe failure as contention.

/// Which probe failures are contention — not attributable to the range's
/// contents — and how many re-probes they may buy.
///
/// The default classification is [`is_contention`]: a deadlock victim or a
/// serialization failure, recognised anywhere in the error's source chain.
/// Override it for error types of your own that describe the same thing: a
/// failure that says nothing about the items, only about the interleaving.
///
/// Other transients are left to the search on purpose. An
/// optimistic-concurrency conflict names one stale item, so splitting
/// isolates it where re-probing the same range would only conflict again; a
/// lost connection cannot be re-probed at all, and surfaces as the helper's
/// outer `Err` on the next savepoint instead.
#[derive(Debug, Clone, Copy)]
pub struct TransientPolicy<P> {
    /// Returns `true` when a probe failure carries no information about the
    /// range's contents.
    pub is_transient: P,
    /// How many transient re-probes the whole search may take before it is
    /// abandoned.
    pub max_retries: usize,
}

impl<P> TransientPolicy<P> {
    /// A policy with the default retry allowance.
    pub fn new(is_transient: P) -> Self {
        Self {
            is_transient,
            max_retries: super::DEFAULT_MAX_TRANSIENT_RETRIES,
        }
    }

    /// Overrides the retry allowance.
    #[must_use]
    pub fn with_max_retries(self, max_retries: usize) -> Self {
        Self {
            max_retries,
            ..self
        }
    }
}

/// The default classifier, as a plain function so it can be named in a
/// [`TransientPolicy`] without boxing: [`Fault::classify`] over the whole
/// [`source`](std::error::Error::source) chain, then
/// [`Fault::is_contention`] — so a deadlock (`40P01`) or a serialization
/// failure (`40001`) is recognised whether it arrives as a lane payload a
/// repo op already classified or as a raw [`sqlx::Error`] several layers
/// deep inside a caller's own error type.
///
/// [`Fault::classify`]: crate::errlanes::Fault::classify
/// [`Fault::is_contention`]: crate::errlanes::Fault::is_contention
pub(super) fn is_contention<E: std::error::Error + 'static>(error: &E) -> bool {
    crate::errlanes::Fault::classify(error).is_contention()
}

#[cfg(test)]
mod tests {
    use super::is_contention;
    use crate::errlanes::{Transient, TransientKind};

    #[derive(Debug, thiserror::Error)]
    #[error("wrapped")]
    struct Wrapped(#[source] Transient);

    #[test]
    fn contention_is_found_through_a_callers_wrapper() {
        assert!(is_contention(&Wrapped(Transient::new(
            TransientKind::Deadlock
        ))));
        assert!(is_contention(&Wrapped(Transient::new(
            TransientKind::SerializationFailure
        ))));
    }

    #[test]
    fn other_transients_are_left_to_the_search() {
        assert!(!is_contention(&Wrapped(Transient::new(
            TransientKind::OptimisticConflict
        ))));
        assert!(!is_contention(&Wrapped(Transient::new(
            TransientKind::ConnectionLost
        ))));
        assert!(!is_contention(&std::io::Error::other(
            "not transient at all"
        )));
    }
}

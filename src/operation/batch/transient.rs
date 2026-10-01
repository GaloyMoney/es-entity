//! Classifying a probe failure as transient.

/// Which probe failures are transient, and how many re-probes they may buy.
///
/// The default classification is [`sqlstate_is_transient`]. Override it to add
/// error types of your own that describe contention — an optimistic-concurrency
/// conflict, say — so the search re-probes their ranges too.
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
/// [`TransientPolicy`] without boxing.
///
/// Prefers the lane an error was born with, and otherwise walks the whole
/// [`source`](std::error::Error::source) chain for a raw [`sqlx::Error`] to
/// classify — so a deadlock (`40P01`), a serialization failure (`40001`), a
/// lost connection or a pool timeout is recognised even several layers deep
/// inside a caller's own error type. Each says nothing about the items being
/// probed, only about the contention, so a bisect re-probes the same range
/// unsplit.
pub(super) fn sqlstate_is_transient<E: std::error::Error + 'static>(error: &E) -> bool {
    crate::errlanes::Fault::classify(error).is_transient()
}

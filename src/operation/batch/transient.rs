//! Classifying a probe failure as transient.

/// The Postgres sqlstate of a failure that says nothing about the statement
/// that hit it, or `None`.
///
/// Walks the whole [`source`](std::error::Error::source) chain, so a
/// [`sqlx::Error`] wrapped several layers deep in a caller's own error type is
/// still recognised.
///
/// - `40P01` — deadlock detected. This transaction was chosen as the victim;
///   another one made progress.
/// - `40001` — serialization failure.
///
/// Both are properties of the contention, not of the items being probed, so a
/// bisect re-probes the same range unsplit.
pub fn retryable_conflict_code(err: &(dyn std::error::Error + 'static)) -> Option<&'static str> {
    let mut source = Some(err);
    while let Some(err) = source {
        if let Some(db) = err
            .downcast_ref::<sqlx::Error>()
            .and_then(|err| err.as_database_error())
        {
            match db.code().as_deref() {
                Some("40P01") => return Some("40P01"),
                Some("40001") => return Some("40001"),
                _ => {}
            }
        }
        source = err.source();
    }
    None
}

/// [`retryable_conflict_code`] as a predicate.
pub fn is_retryable_conflict(err: &(dyn std::error::Error + 'static)) -> bool {
    retryable_conflict_code(err).is_some()
}

/// Which probe failures are transient, and how many re-probes they may buy.
///
/// The default classification is [`is_retryable_conflict`]. Override it to add
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
pub(super) fn sqlstate_is_transient<E: std::error::Error + 'static>(error: &E) -> bool {
    is_retryable_conflict(error)
}

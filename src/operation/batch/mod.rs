//! Running a batch of items in one transaction while isolating each failure.
//!
//! Two shapes, both built on [`SavepointOperation::with_savepoint`] and both
//! available on every [`AtomicOperation`](super::AtomicOperation):
//!
//! - [`run_isolated`](BatchIsolation::run_isolated) — one savepoint per item.
//!   Use it when each item needs its own logic and its own error isolation.
//! - [`run_bisected`](BatchIsolation::run_bisected) — one probe over the whole
//!   slice, splitting only on failure. Use it when the closure handles a whole
//!   slice in set-based statements (`create_all` / `update_all` and friends),
//!   so the happy path costs one probe.
//!
//! # The closure
//!
//! `AsyncFnOnce + Clone + Sync`, cloned once per probe so that each probe calls
//! its own clone exactly once.
//!
//! Each such call has a single opaque future type, which auto-trait inference
//! resolves at the call site. A closure that borrows `&self` therefore composes
//! inside an `#[async_trait]` runner or a `tokio::spawn`.
//!
//! Cloning the closure clones its captures, so state shared *across* probes
//! belongs behind something whose clone is the same underlying value: `Arc<…>`,
//! or a borrowed `&Mutex<…>`. `Sync` holds callers to that — `Cell`, `RefCell`
//! and `Rc` are not `Sync`, so the bound rejects them.
//!
//! # What `f` must tolerate
//!
//! A bisect calls `f` repeatedly over **arbitrary contiguous sub-slices, in
//! non-positional order**, and may re-probe the same range after a transient
//! failure. `f` must therefore be a function of the set it is handed, and of
//! the database state at that moment: only `*_in_op` work against the probe's
//! operation, so that a rollback undoes all of it.
//!
//! # Hooks
//!
//! Commit hooks registered inside a probe are staged on its savepoint: folded
//! outward on success, dropped on failure. No hook callback runs at a savepoint
//! boundary, so a rolled-back probe contributes exactly zero hook state to
//! match its zero database state. See [`SavepointOp`].
//!
//! # Observability
//!
//! Tracing is left to the caller, since es-entity's `tracing` dependency is
//! optional. [`BisectOutcomes`] carries `probes_used` and `transient_retries`
//! for reporting under the caller's own target and field names.

mod search;
mod transient;

use std::future::Future;

use super::{SavepointOp, SavepointOperation};

pub use search::*;
pub use transient::*;

/// Batch isolation for every [`AtomicOperation`](super::AtomicOperation).
///
/// Blanket-implemented, like [`SavepointOperation`], so `DbOp`, `SavepointOp`
/// (nesting), `HookOperation`, and operation types defined outside this crate
/// all get it without naming a concrete type — and so a function generic over
/// `impl AtomicOperation` can use it.
pub trait BatchIsolation: SavepointOperation {
    /// Runs `f` once per item, each inside its own `SAVEPOINT`, in item order.
    ///
    /// A failing item unwinds only its own writes and staged hooks; the
    /// transaction stays usable and the loop continues, so its healthy
    /// batch-mates still commit. Outcomes are returned positionally: one entry
    /// per input, `f`'s own `Ok`/`Err` preserved.
    ///
    /// The outer `Err(sqlx::Error)` means the savepoint machinery itself
    /// failed, leaving the enclosing transaction in an indeterminate state:
    /// abandon it.
    fn run_isolated<'a, T, V, E, F>(
        &'a mut self,
        items: &'a [T],
        f: F,
    ) -> impl Future<Output = Result<Vec<Result<V, E>>, sqlx::Error>> + 'a
    where
        T: 'a,
        V: 'a,
        E: 'a,
        F: AsyncFnOnce(&mut SavepointOp<'_>, &T) -> Result<V, E> + Clone + Sync + 'a,
    {
        async move {
            let mut outcomes = Vec::with_capacity(items.len());
            for item in items {
                let f = f.clone();
                outcomes.push(self.with_savepoint(async |sp| f(sp, item).await).await?);
            }
            Ok(outcomes)
        }
    }

    /// Probes the whole slice at once, bisecting only on failure.
    ///
    /// The happy path costs **one** probe. On failure the slice splits and
    /// pending ranges are probed largest-first (earliest start breaking ties)
    /// until `budget` is spent, so clean siblings are salvaged and a culprit
    /// resolves from its own single-item probe, where its error is
    /// attributable to it.
    ///
    /// Deadlock victims and serialization failures re-probe the same range
    /// unsplit and are refunded to the budget — see [`TransientPolicy`]. Use
    /// [`run_bisected_with`](Self::run_bisected_with) to widen that class.
    fn run_bisected<'a, T, E, F>(
        &'a mut self,
        items: &'a [T],
        budget: BisectBudget,
        f: F,
    ) -> impl Future<Output = Result<BisectOutcomes<E>, sqlx::Error>> + 'a
    where
        T: 'a,
        E: std::error::Error + 'static,
        F: AsyncFnOnce(&mut SavepointOp<'_>, &[T]) -> Result<(), E> + Clone + Sync + 'a,
    {
        self.run_bisected_with(
            items,
            budget,
            TransientPolicy::new(sqlstate_is_transient::<E> as fn(&E) -> bool),
            f,
        )
    }

    /// [`run_bisected`](Self::run_bisected) with a caller-supplied notion of
    /// which failures are transient.
    ///
    /// Classification being the caller's, the error bound here is just
    /// [`Display`](std::fmt::Display).
    fn run_bisected_with<'a, T, E, F, P>(
        &'a mut self,
        items: &'a [T],
        budget: BisectBudget,
        policy: TransientPolicy<P>,
        f: F,
    ) -> impl Future<Output = Result<BisectOutcomes<E>, sqlx::Error>> + 'a
    where
        T: 'a,
        E: std::fmt::Display + 'a,
        P: Fn(&E) -> bool + 'a,
        F: AsyncFnOnce(&mut SavepointOp<'_>, &[T]) -> Result<(), E> + Clone + Sync + 'a,
    {
        async move {
            let mut search = BisectSearch::new(items.len(), budget)
                .with_max_transient_retries(policy.max_retries);

            while let Some(range) = search.next_range() {
                let f = f.clone();
                let slice = &items[range.clone()];

                let verdict = match self.with_savepoint(async |sp| f(sp, slice).await).await? {
                    Ok(()) => ProbeVerdict::Clean,
                    Err(error) if (policy.is_transient)(&error) => ProbeVerdict::Transient(error),
                    Err(error) => ProbeVerdict::Failed(error),
                };

                if let Err(limit) = search.report(range, verdict) {
                    // The search learned nothing about the items, so there are
                    // no per-item verdicts to return — the caller re-runs the
                    // whole batch.
                    return Err(sqlx::Error::Protocol(match search.last_error() {
                        Some(error) => format!("{limit}; last error: {error}"),
                        None => limit.to_string(),
                    }));
                }
            }

            Ok(search.into_outcomes())
        }
    }
}

impl<T: SavepointOperation + ?Sized> BatchIsolation for T {}

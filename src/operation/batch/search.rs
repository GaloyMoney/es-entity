//! The bisect search, as a pure state machine.
//!
//! Index bookkeeping over the input's length: it decides *which contiguous
//! range to probe next* and *what a probe's verdict means*, while the caller
//! decides how a probe runs and where its transaction boundary sits. So both of
//! these drive the same algorithm:
//!
//! - [`BatchIsolation::run_bisected`](super::BatchIsolation::run_bisected),
//!   against `with_savepoint` inside a single enclosing transaction.
//! - A caller that wants a transaction boundary *between* probes, committing
//!   each clean range as it lands: it mints and commits its own operations
//!   around [`next_range`](BisectSearch::next_range) and
//!   [`report`](BisectSearch::report).
//!
//! Being plain bookkeeping, the probe sequence is also testable on its own.

use std::{cmp::Ordering, collections::BinaryHeap, ops::Range};

/// How many times a transiently-failed range may be re-probed before the search
/// is abandoned.
///
/// A retry costs up to one `deadlock_timeout` wait, so this caps the time a
/// search spends on contention.
pub const DEFAULT_MAX_TRANSIENT_RETRIES: usize = 2;

/// How many probes a bisect may spend before giving up on the ranges it has
/// not yet resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BisectBudget {
    /// `2·⌈log₂N⌉ + 1` — the depth needed to isolate one failing item and
    /// resolve every other item as clean. 9 probes at N=10, 15 at N=100.
    #[default]
    Auto,
    /// An explicit cap, clamped to at least 1. `MaxProbes(1)` probes once and
    /// leaves every item unresolved if that probe fails.
    MaxProbes(usize),
    /// No cap: keep splitting until every item is resolved. Worst case
    /// `2N - 1` probes for an all-bad batch.
    FullResolution,
}

impl BisectBudget {
    /// The probe cap this budget implies for a batch of `n` items.
    pub fn effective_cap(self, n: usize) -> usize {
        match self {
            Self::Auto => 2 * ceil_log2(n) + 1,
            Self::MaxProbes(max) => max.max(1),
            Self::FullResolution => usize::MAX,
        }
    }
}

/// `ceil(log2(n))`, defined as `0` for `n <= 1`.
fn ceil_log2(n: usize) -> usize {
    if n <= 1 {
        return 0;
    }
    (usize::BITS - (n - 1).leading_zeros()) as usize
}

/// A range still awaiting a probe.
///
/// The [`Ord`] impl is the search's determinism guarantee: a
/// [`BinaryHeap`] pops the **largest** range, and among equal lengths the one
/// with the **earliest start**. For a given input and budget the probe sequence
/// — and therefore the probe count — is reproducible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingRange {
    start: usize,
    end: usize,
}

impl PendingRange {
    fn len(&self) -> usize {
        self.end - self.start
    }
}

impl Ord for PendingRange {
    fn cmp(&self, other: &Self) -> Ordering {
        self.len()
            .cmp(&other.len())
            .then_with(|| other.start.cmp(&self.start))
    }
}

impl PartialOrd for PendingRange {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// What a probe over one range turned out to be.
#[derive(Debug)]
pub enum ProbeVerdict<E> {
    /// Every item in the range succeeded. They resolve here and are never
    /// probed again.
    Clean,
    /// The range failed for a reason attributable to its contents. A
    /// multi-item range splits; a single-item range resolves as that item's
    /// failure.
    Failed(E),
    /// The failure describes contention: a deadlock victim, a serialization
    /// failure, or a caller-classified conflict. The **same** range is
    /// re-probed unsplit, and the probe is refunded to the budget.
    Transient(E),
}

/// One input item's resolution. Positionally aligned with the input slice.
#[derive(Debug, PartialEq, Eq)]
pub enum ItemOutcome<E> {
    /// Resolved by a clean probe over a range containing it.
    Complete,
    /// Resolved by its own single-item probe, so the error is attributable to
    /// this item alone.
    Failed(E),
    /// Never resolved: the budget ran out, or the search was abandoned, while
    /// this item was still inside an unprobed range. See
    /// [`BisectOutcomes::last_error`] for what the search last saw.
    Unresolved,
}

/// The result of a completed search: one outcome per input item, plus what it
/// cost.
#[derive(Debug)]
pub struct BisectOutcomes<E> {
    /// One entry per input item, in input order.
    pub items: Vec<ItemOutcome<E>>,
    /// Probes actually spent, net of refunded transient re-probes.
    pub probes_used: usize,
    /// Transient re-probes taken (refunded, so not counted in `probes_used`).
    pub transient_retries: usize,
    /// The most recent error from a probe spanning more than one item — what a
    /// caller quotes when explaining an [`ItemOutcome::Unresolved`].
    ///
    /// `None` when every failure reached a single-item probe, since each of
    /// those hands its error to [`ItemOutcome::Failed`].
    pub last_error: Option<E>,
}

/// The transient allowance ran out: the same range kept failing on contention.
///
/// The search has nothing to attribute to any item, so the caller abandons it
/// and retries the whole batch later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransientLimitExceeded {
    /// Probes spent before abandoning.
    pub probes_used: usize,
    /// Transient re-probes taken before abandoning.
    pub transient_retries: usize,
}

impl std::fmt::Display for TransientLimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "bisect abandoned after {} probes and {} transient retries",
            self.probes_used, self.transient_retries
        )
    }
}

impl std::error::Error for TransientLimitExceeded {}

/// Drives a bisect over `n` items: which range to probe next, and what each
/// verdict means for the ranges still outstanding.
///
/// ```rust,ignore
/// let mut search = BisectSearch::new(items.len(), BisectBudget::Auto);
/// while let Some(range) = search.next_range() {
///     // The caller owns the operation — one shared transaction, or a fresh
///     // one committed per range.
///     let verdict = probe(&items[range.clone()]).await;
///     search.report(range, verdict)?;
/// }
/// let outcomes = search.into_outcomes();
/// ```
#[derive(Debug)]
pub struct BisectSearch<E> {
    pending: BinaryHeap<PendingRange>,
    resolved: Vec<Option<ItemOutcome<E>>>,
    probes_used: usize,
    cap: usize,
    transient_retries: usize,
    max_transient_retries: usize,
    last_error: Option<E>,
}

impl<E> BisectSearch<E> {
    /// Starts a search over `n` items with the whole range as the first probe.
    pub fn new(n: usize, budget: BisectBudget) -> Self {
        let mut pending = BinaryHeap::new();
        if n > 0 {
            pending.push(PendingRange { start: 0, end: n });
        }
        Self {
            pending,
            resolved: (0..n).map(|_| None).collect(),
            probes_used: 0,
            cap: budget.effective_cap(n),
            transient_retries: 0,
            max_transient_retries: DEFAULT_MAX_TRANSIENT_RETRIES,
            last_error: None,
        }
    }

    /// Overrides [`DEFAULT_MAX_TRANSIENT_RETRIES`] for this search.
    #[must_use]
    pub fn with_max_transient_retries(mut self, max: usize) -> Self {
        self.max_transient_retries = max;
        self
    }

    /// The next range to probe: the largest outstanding one, earliest start
    /// breaking ties.
    ///
    /// `None` when everything is resolved or the budget is spent — items still
    /// inside unprobed ranges resolve as [`ItemOutcome::Unresolved`].
    pub fn next_range(&mut self) -> Option<Range<usize>> {
        if self.probes_used >= self.cap {
            return None;
        }
        let range = self.pending.pop()?;
        self.probes_used += 1;
        Some(range.start..range.end)
    }

    /// Records what a probe found.
    ///
    /// Errors once the transient allowance is exhausted, at which point the
    /// search is over — see [`TransientLimitExceeded`].
    pub fn report(
        &mut self,
        range: Range<usize>,
        verdict: ProbeVerdict<E>,
    ) -> Result<(), TransientLimitExceeded> {
        let range = PendingRange {
            start: range.start,
            end: range.end,
        };

        match verdict {
            ProbeVerdict::Clean => {
                for slot in &mut self.resolved[range.start..range.end] {
                    *slot = Some(ItemOutcome::Complete);
                }
            }

            ProbeVerdict::Transient(error) => {
                self.last_error = Some(error);
                // Refunded: the budget pays for bisection, and this probe
                // produced none.
                self.probes_used = self.probes_used.saturating_sub(1);
                if self.transient_retries >= self.max_transient_retries {
                    return Err(TransientLimitExceeded {
                        probes_used: self.probes_used,
                        transient_retries: self.transient_retries,
                    });
                }
                self.transient_retries += 1;
                self.pending.push(range);
            }

            // A single item that failed on its own probe: the error is
            // attributable to it, so it owns it.
            ProbeVerdict::Failed(error) if range.len() == 1 => {
                self.resolved[range.start] = Some(ItemOutcome::Failed(error));
            }

            ProbeVerdict::Failed(error) => {
                self.last_error = Some(error);
                let mid = range.start + range.len() / 2;
                self.pending.push(PendingRange {
                    start: range.start,
                    end: mid,
                });
                self.pending.push(PendingRange {
                    start: mid,
                    end: range.end,
                });
            }
        }

        Ok(())
    }

    /// Probes spent so far, net of refunds.
    pub fn probes_used(&self) -> usize {
        self.probes_used
    }

    /// Transient re-probes taken so far.
    pub fn transient_retries(&self) -> usize {
        self.transient_retries
    }

    /// The most recent error from a probe spanning more than one item.
    pub fn last_error(&self) -> Option<&E> {
        self.last_error.as_ref()
    }

    /// Finishes the search, resolving anything still outstanding as
    /// [`ItemOutcome::Unresolved`] so every input gets exactly one outcome.
    pub fn into_outcomes(self) -> BisectOutcomes<E> {
        let items = self
            .resolved
            .into_iter()
            .map(|slot| slot.unwrap_or(ItemOutcome::Unresolved))
            .collect();

        BisectOutcomes {
            items,
            probes_used: self.probes_used,
            transient_retries: self.transient_retries,
            last_error: self.last_error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs a search where `culprits` are the item indices that fail, recording
    /// the probe sequence. No database, no operation — the algorithm alone.
    fn run(
        n: usize,
        budget: BisectBudget,
        culprits: &[usize],
    ) -> (Vec<Range<usize>>, BisectOutcomes<String>) {
        let mut search = BisectSearch::new(n, budget);
        let mut probes = Vec::new();
        while let Some(range) = search.next_range() {
            probes.push(range.clone());
            let verdict = if culprits.iter().any(|c| range.contains(c)) {
                ProbeVerdict::Failed(format!("bad {range:?}"))
            } else {
                ProbeVerdict::Clean
            };
            search.report(range, verdict).expect("no transients here");
        }
        (probes, search.into_outcomes())
    }

    #[test]
    fn auto_budget_matches_the_documented_formula() {
        assert_eq!(BisectBudget::Auto.effective_cap(1), 1);
        assert_eq!(BisectBudget::Auto.effective_cap(10), 9);
        assert_eq!(BisectBudget::Auto.effective_cap(25), 11);
        assert_eq!(BisectBudget::Auto.effective_cap(100), 15);
    }

    #[test]
    fn max_probes_zero_clamps_to_one() {
        assert_eq!(BisectBudget::MaxProbes(0).effective_cap(50), 1);
        assert_eq!(BisectBudget::MaxProbes(3).effective_cap(50), 3);
    }

    #[test]
    fn a_clean_batch_probes_exactly_once() {
        let (probes, outcomes) = run(5, BisectBudget::Auto, &[]);
        assert_eq!(probes, vec![0..5]);
        assert_eq!(outcomes.probes_used, 1);
        assert!(
            outcomes
                .items
                .iter()
                .all(|o| matches!(o, ItemOutcome::Complete))
        );
    }

    #[test]
    fn ranges_are_probed_largest_first_earliest_start_on_ties() {
        let (probes, _) = run(8, BisectBudget::FullResolution, &[0]);
        // Whole slice, then the split halves largest-first with the earlier
        // start winning the tie, narrowing onto index 0.
        assert_eq!(probes[0], 0..8);
        assert_eq!(probes[1], 0..4);
        assert_eq!(probes[2], 4..8);
        assert!(
            probes.windows(2).all(|w| {
                let (a, b) = (w[0].len(), w[1].len());
                a > b || (a == b && w[0].start < w[1].start) || a < b
            }),
            "probe order was {probes:?}"
        );
    }

    #[test]
    fn a_single_culprit_is_isolated_and_its_siblings_are_salvaged() {
        let (_, outcomes) = run(8, BisectBudget::Auto, &[3]);
        for (idx, outcome) in outcomes.items.iter().enumerate() {
            if idx == 3 {
                assert!(
                    matches!(outcome, ItemOutcome::Failed(_)),
                    "index 3: {outcome:?}"
                );
            } else {
                assert_eq!(outcome, &ItemOutcome::Complete, "index {idx}");
            }
        }
    }

    #[test]
    fn scattered_culprits_do_not_poison_their_clean_siblings() {
        let (_, outcomes) = run(16, BisectBudget::FullResolution, &[0, 8]);
        let completed = outcomes
            .items
            .iter()
            .filter(|o| matches!(o, ItemOutcome::Complete))
            .count();
        assert_eq!(completed, 14);
        assert!(matches!(outcomes.items[0], ItemOutcome::Failed(_)));
        assert!(matches!(outcomes.items[8], ItemOutcome::Failed(_)));
    }

    #[test]
    fn full_resolution_resolves_every_item_of_an_all_bad_batch() {
        let (_, outcomes) = run(6, BisectBudget::FullResolution, &[0, 1, 2, 3, 4, 5]);
        assert!(
            outcomes
                .items
                .iter()
                .all(|o| matches!(o, ItemOutcome::Failed(_)))
        );
    }

    #[test]
    fn max_probes_one_is_equivalent_to_resolving_nothing() {
        let (probes, outcomes) = run(5, BisectBudget::MaxProbes(1), &[2]);
        assert_eq!(probes, vec![0..5]);
        assert!(
            outcomes
                .items
                .iter()
                .all(|o| matches!(o, ItemOutcome::Unresolved))
        );
        assert!(
            outcomes.last_error.is_some(),
            "the batch-level error is kept for the caller"
        );
    }

    #[test]
    fn every_input_gets_exactly_one_outcome_even_under_budget_exhaustion() {
        let (_, outcomes) = run(10, BisectBudget::MaxProbes(3), &[0]);
        assert_eq!(outcomes.items.len(), 10);
    }

    #[test]
    fn a_transient_probe_is_re_run_whole_and_refunded() {
        let mut search: BisectSearch<String> = BisectSearch::new(8, BisectBudget::MaxProbes(1));

        let first = search.next_range().expect("a first probe");
        assert_eq!(first, 0..8);
        search
            .report(first, ProbeVerdict::Transient("40P01".into()))
            .expect("within the allowance");

        // Refunded, so the budget of 1 still admits a probe, and it covers the
        // same range.
        let retry = search.next_range().expect("the refunded re-probe");
        assert_eq!(retry, 0..8);
        assert_eq!(search.transient_retries(), 1);

        search.report(retry, ProbeVerdict::Clean).expect("clean");
        let outcomes = search.into_outcomes();
        assert_eq!(outcomes.probes_used, 1);
        assert_eq!(outcomes.transient_retries, 1);
        assert!(
            outcomes
                .items
                .iter()
                .all(|o| matches!(o, ItemOutcome::Complete))
        );
    }

    #[test]
    fn the_transient_allowance_is_bounded() {
        let mut search: BisectSearch<String> =
            BisectSearch::new(4, BisectBudget::FullResolution).with_max_transient_retries(2);

        for _ in 0..2 {
            let range = search.next_range().expect("a probe");
            search
                .report(range, ProbeVerdict::Transient("40001".into()))
                .expect("within the allowance");
        }

        let range = search.next_range().expect("a probe");
        let limit = search
            .report(range, ProbeVerdict::Transient("40001".into()))
            .expect_err("the allowance is spent");
        assert_eq!(limit.transient_retries, 2);
    }

    #[test]
    fn an_empty_batch_probes_nothing() {
        let (probes, outcomes) = run(0, BisectBudget::Auto, &[]);
        assert!(probes.is_empty());
        assert!(outcomes.items.is_empty());
    }
}

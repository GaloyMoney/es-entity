//! `ResultExt` — the one trait a consumer imports to move a `Result` from one
//! error signature to another. `widen`, the three `narrow_*`, `rejected`,
//! `classify`, and (under `tracing`) `record` are all the same act —
//! relocating a `Result`'s error — so they live on one blanket-impl'd trait
//! rather than one import per verb.

use crate::{
    classify::Classify,
    fail::{Fail, Failure, Fault, Laned, Rejection, WidenResult},
    lane::{Denied, Fatal},
    profile::{LaneProfile, NarrowDenied, WithoutDenied},
};

/// Sealed. The error-level engine for [`ResultExt::narrow_rejected`] — keyed
/// on the source shape the same way [`WidenResult`] is, since only `Fail`
/// carries a rejected lane to narrow away.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`{Self}` has no rejected lane to narrow",
    note = "`narrow_rejected` is for a `Fail<D, L>` whose rejected lane has no \
            caller left to correct it; a `Fault<L>` has no rejected lane at all"
)]
pub trait NarrowRejectedLane: crate::fail::sealed::Sealed {
    type Narrowed;
    fn narrow_rejected(self) -> Self::Narrowed;
}

impl<D: Rejection, L: LaneProfile<Fatal = Fatal>> NarrowRejectedLane for Fail<D, L> {
    type Narrowed = Fault<L>;
    fn narrow_rejected(self) -> Fault<L> {
        Fail::narrow_rejected(self)
    }
}

/// Sealed. The error-level engine for [`ResultExt::narrow_denied`], shared by
/// both carriers — `Fail` and `Fault` each still have a denied lane to
/// narrow.
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`{Self}` has no denied lane to narrow",
    note = "`narrow_denied` is only available on a profile whose `Denied` lane is enabled"
)]
pub trait NarrowDeniedLane: crate::fail::sealed::Sealed {
    type Narrowed;
    fn narrow_denied(self) -> Self::Narrowed;
}

// `L: LaneProfile<Denied = Denied>` (the concrete marker, not the generic
// `Slot<Denied>`) is load-bearing: `NarrowDenied<F>` also has a blanket impl
// for `Infallible` (a disabled slot), so bounding on `NarrowDenied` alone
// would make this engine — and so `ResultExt::narrow_denied` — callable even
// when the profile has no `Denied` lane at all, turning narrowing into a
// silent identity on an already-absent lane instead of the compile error
// decision 2 requires. The inherent `Fault::narrow_denied`/`Fail::narrow_denied`
// keep the wider bound; only this engine is deliberately narrower.
impl<D: Rejection, L> NarrowDeniedLane for Fail<D, L>
where
    L: LaneProfile<Denied = Denied>,
    L::Denied: NarrowDenied<L::Fatal>,
{
    type Narrowed = Fail<D, WithoutDenied<L>>;
    fn narrow_denied(self) -> Self::Narrowed {
        Fail::narrow_denied(self)
    }
}

impl<L> NarrowDeniedLane for Fault<L>
where
    L: LaneProfile<Denied = Denied>,
    L::Denied: NarrowDenied<L::Fatal>,
{
    type Narrowed = Fault<WithoutDenied<L>>;
    fn narrow_denied(self) -> Self::Narrowed {
        Fault::narrow_denied(self)
    }
}

/// The one trait a consumer imports to move a `Result` between error
/// signatures: widen it to a bigger profile, narrow away a lane that is no
/// longer live at this boundary, hand the rejection to the caller as a value,
/// classify a foreign error into a local wrapper, or (under `tracing`) record
/// it onto the current span.
pub trait ResultExt<T, E>: Sized {
    /// Target-inferred widening for `Fault` or `Fail` results.
    ///
    /// `Fault` results widen to `Fault`; `Fail` results widen to `Fail`,
    /// converting the rejection through [`crate::Lift`]. Success values and
    /// fault payloads are preserved. Widening can add lanes, but cannot
    /// silently discard an enabled lane:
    ///
    /// ```compile_fail
    /// use errlanes::{Fault, ResultExt, lanes};
    /// fn discard_denied(value: Result<(), Fault<lanes!(Denied, Fatal)>>)
    ///     -> Result<(), Fault<lanes!(Fatal)>>
    /// {
    ///     value.widen()
    /// }
    /// ```
    fn widen<E2>(self) -> Result<T, E2>
    where
        Self: WidenResult<T, E2>;

    /// Narrows away the `Rejected` lane: a rejection with no caller left to
    /// correct it becomes `Fatal(Invariant)`, carrying the rejection as its
    /// source. Only available where `E` is a `Fail` — a `Fault` has no
    /// rejected lane to narrow, and that is a compile error, not an
    /// identity:
    ///
    /// ```compile_fail
    /// use errlanes::{Fault, ResultExt, lanes};
    /// fn narrow(value: Result<(), Fault<lanes!(Fatal)>>)
    ///     -> Result<(), Fault<lanes!(Fatal)>>
    /// {
    ///     value.narrow_rejected()
    /// }
    /// ```
    fn narrow_rejected(self) -> Result<T, E::Narrowed>
    where
        E: NarrowRejectedLane;

    /// Narrows away the `Transient` lane: a caller-owned retry loop hands
    /// back the narrowed profile once it stops retrying, so it stops
    /// offering `Transient` to its own callers. `attempts` is what the loop
    /// counted; an exhausted transient becomes `Fatal(Exhausted)` with the
    /// last transient as its source.
    fn narrow_transient(self, attempts: u32) -> Result<T, E::WithoutTransient>
    where
        E: Laned;

    /// Narrows away the `Denied` lane: a denial at a boundary with no
    /// subject (code running as the system) becomes `Fatal(Denied)`. Only
    /// available where `E`'s profile enables `Denied`:
    ///
    /// ```compile_fail
    /// use errlanes::{Fault, ResultExt, lanes};
    /// fn narrow(value: Result<(), Fault<lanes!(Transient, Fatal)>>)
    ///     -> Result<(), Fault<lanes!(Transient, Fatal)>>
    /// {
    ///     value.narrow_denied()
    /// }
    /// ```
    fn narrow_denied(self) -> Result<T, E::Narrowed>
    where
        E: NarrowDeniedLane;

    /// Hands the rejection to the caller as a value and keeps the faults
    /// propagating: `Result<T, Fail<D, L>>` becomes
    /// `Result<Result<T, D>, Fault<L>>`, the `Result`-level form of
    /// [`Fail::rejected`]. The outer `?` carries the faults on into any
    /// enclosing carrier; the inner `Result` is the domain outcome, with the
    /// rejection as its `Err`, matched right where it occurred:
    ///
    /// ```
    /// use errlanes::{Fail, Fault, ResultExt, lanes};
    ///
    /// #[derive(Debug, errlanes::Rejection)]
    /// #[rejection(code = "TIMED_OUT")]
    /// struct TimedOut;
    ///
    /// fn await_completion() -> Result<u64, Fail<TimedOut, lanes!(Transient, Fatal)>> {
    ///     Err(Fail::Rejected(TimedOut))
    /// }
    ///
    /// fn poll_once() -> Result<Option<u64>, Fault<lanes!(Transient, Fatal)>> {
    ///     match await_completion().rejected()? {
    ///         Ok(outcome) => Ok(Some(outcome)),
    ///         Err(TimedOut) => Ok(None),
    ///     }
    /// }
    ///
    /// assert!(poll_once().unwrap().is_none());
    /// ```
    ///
    /// This is the dual of [`narrow_rejected`](Self::narrow_rejected): that
    /// one is for a boundary with no caller left to correct the rejection,
    /// this one for the call site that is going to. There is deliberately no
    /// `Option`-returning accessor on a `Result`: an `as_rejected()` that
    /// answered `None` for both `Ok` and a fault would be the one place a
    /// lane could be dropped without naming it. Only available where `E`
    /// carries a rejected lane; a `Fault` has none:
    ///
    /// ```compile_fail
    /// use errlanes::{Fault, ResultExt, lanes};
    /// fn split(value: Result<(), Fault<lanes!(Fatal)>>) {
    ///     let _ = value.rejected();
    /// }
    /// ```
    fn rejected(self) -> Result<Result<T, E::Rejection>, Fault<E::Lanes>>
    where
        E: Failure;

    /// Maps the rejection with a closure and leaves every other lane as it
    /// is: the `Result`-level form of [`Fail::map_rejected`]. For a
    /// type-level remap use [`widen`](Self::widen); this is for enriching a
    /// rejection with data only the call site has, such as the input that
    /// was attempted: `repo.create(new).await.map_rejected(|r| r.with_attempted(id))?`.
    fn map_rejected<D2>(self, f: impl FnOnce(E::Rejection) -> D2) -> Result<T, Fail<D2, E::Lanes>>
    where
        E: Failure;

    /// `.classify::<W>()` — the verb that turns a foreign error into a local
    /// [`Classify`] wrapper at a one-off call site, so a function that does
    /// not itself return `W` can still enter the lanes through it:
    /// `conn.query(..).await.classify::<DbWrite>()?`.
    fn classify<W: Classify + From<E>>(self) -> Result<T, W>;

    /// Records onto the current span, then hands the result straight back —
    /// for a call site that must record and keep going rather than
    /// propagate (`?`), such as a batch dispatcher writing its own
    /// `conclusion` after the fact.
    #[cfg(feature = "tracing")]
    fn record(self) -> Self
    where
        E: Laned;
}

impl<T, E> ResultExt<T, E> for Result<T, E> {
    fn widen<E2>(self) -> Result<T, E2>
    where
        Self: WidenResult<T, E2>,
    {
        WidenResult::widen(self)
    }

    fn narrow_rejected(self) -> Result<T, E::Narrowed>
    where
        E: NarrowRejectedLane,
    {
        self.map_err(NarrowRejectedLane::narrow_rejected)
    }

    fn narrow_transient(self, attempts: u32) -> Result<T, E::WithoutTransient>
    where
        E: Laned,
    {
        self.map_err(|e| e.narrow_transient(attempts))
    }

    fn narrow_denied(self) -> Result<T, E::Narrowed>
    where
        E: NarrowDeniedLane,
    {
        self.map_err(NarrowDeniedLane::narrow_denied)
    }

    fn rejected(self) -> Result<Result<T, E::Rejection>, Fault<E::Lanes>>
    where
        E: Failure,
    {
        match self {
            Ok(value) => Ok(Ok(value)),
            Err(e) => e.into_fail().rejected().map(Err),
        }
    }

    fn map_rejected<D2>(self, f: impl FnOnce(E::Rejection) -> D2) -> Result<T, Fail<D2, E::Lanes>>
    where
        E: Failure,
    {
        self.map_err(|e| e.into_fail().map_rejected(f))
    }

    fn classify<W: Classify + From<E>>(self) -> Result<T, W> {
        self.map_err(W::from)
    }

    #[cfg(feature = "tracing")]
    fn record(self) -> Self
    where
        E: Laned,
    {
        if let Err(e) = &self {
            e.record(&tracing::Span::current());
        }
        self
    }
}

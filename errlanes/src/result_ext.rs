//! `ResultExt` — the one trait a consumer imports to move a `Result` from one
//! error signature to another. `widen`, the three `narrow_*`, `rejected`,
//! `classify`, and (under `tracing`) `record` are all the same act —
//! relocating a `Result`'s error — so they live on one blanket-impl'd trait
//! rather than one import per verb.

use crate::{
    carrier::{Carrier, IntoLanes, kind},
    classify::Classify,
    fail::{Fail, Fault, Laned, Rejection, WidenResult},
    lane::{Denied, Fatal},
    profile::{LaneProfile, NarrowDenied, WithoutDenied},
};

/// Sealed. The error-level engine for [`ResultExt::narrow_rejected`] — keyed
/// on the source shape the same way [`WidenResult`] is: a `Fail` narrows to
/// the `Fault` of its own profile, a bare `Rejection` to the bare `Fatal`, a
/// carrier through its built-in. Both outputs are associated types, read off
/// the source, so `.narrow_rejected()?` never needs a destination named at the
/// call site.
///
/// One impl over `S: IntoLanes`, handing off to [`NarrowRejectedBy`] on the
/// concrete shape type. A separate `C: Carrier` impl would overlap the
/// `R: Rejection` one (generic against generic).
#[doc(hidden)]
#[diagnostic::on_unimplemented(
    message = "`{Self}` has no rejected lane to narrow",
    note = "`narrow_rejected` is for a `Fail<D, L>`, or a bare `Rejection`, with no \
            caller left to correct the rejection; a `Fault<L>` has no rejected lane at all"
)]
pub trait NarrowRejectedLane: narrow_sealed::Sealed {
    type Narrowed;
    fn narrow_rejected(self) -> Self::Narrowed;
}

mod narrow_sealed {
    use crate::carrier::IntoLanes;
    pub trait Sealed {}
    impl<S: IntoLanes> Sealed for S {}
}

impl<S: IntoLanes> NarrowRejectedLane for S
where
    S::Shape: NarrowRejectedBy<S>,
{
    type Narrowed = <S::Shape as NarrowRejectedBy<S>>::Narrowed;
    fn narrow_rejected(self) -> Self::Narrowed {
        <S::Shape as NarrowRejectedBy<S>>::narrow(self)
    }
}

#[doc(hidden)]
pub trait NarrowRejectedBy<S> {
    type Narrowed;
    fn narrow(s: S) -> Self::Narrowed;
}

impl<D: Rejection, L: LaneProfile<Fatal = Fatal>> NarrowRejectedBy<Fail<D, L>> for kind::Fail {
    type Narrowed = Fault<L>;
    fn narrow(s: Fail<D, L>) -> Fault<L> {
        Fail::narrow_rejected(s)
    }
}

/// A bare rejection has no profile to keep, so narrowing it yields the bare
/// payload: `Fatal(Invariant)` with the rejection as its (opaque) source. `?`
/// then carries that into any `Fault<L>` or `Fail<D, L>` whose `Fatal` lane
/// is enabled — the destination profile is read off the function signature,
/// never named at the call site.
impl<R: Rejection> NarrowRejectedBy<R> for kind::Source {
    type Narrowed = Fatal;
    fn narrow(s: R) -> Fatal {
        crate::fail::invariant_from_rejection(s)
    }
}

impl<C: Carrier> NarrowRejectedBy<C> for kind::CarrierShape
where
    C::Repr: NarrowRejectedLane,
{
    type Narrowed = <C::Repr as NarrowRejectedLane>::Narrowed;
    fn narrow(s: C) -> Self::Narrowed {
        s.into_repr().narrow_rejected()
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

/// A carrier narrows through its built-in, so its own lane set governs
/// availability: a carrier with no `Denied` lane has a `Repr` with a disabled
/// `Denied` slot, and `narrow_denied` is not callable on it. The result is
/// the narrowed *built-in*, which `?` carries on.
impl<C: Carrier> NarrowDeniedLane for C
where
    C::Repr: NarrowDeniedLane,
{
    type Narrowed = <C::Repr as NarrowDeniedLane>::Narrowed;
    fn narrow_denied(self) -> Self::Narrowed {
        self.into_repr().narrow_denied()
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
    ///
    /// A [`Classify`] wrapper widens too — into a `Fail` as usual, or
    /// straight into a `Fault` when it never rejects. That last form is how a
    /// wrapper enters the lanes at a call site with no signature to infer the
    /// destination from, above all on the way into a `Box<dyn Error>`, where
    /// `?` on the bare wrapper would box it unlaned and lose its
    /// classification (see `classify.rs` and the README's box-boundary
    /// section).
    fn widen<E2>(self) -> Result<T, E2>
    where
        Self: WidenResult<T, E2>;

    /// Narrows away the `Rejected` lane: a rejection with no caller left to
    /// correct it becomes `Fatal(Invariant)`, carrying the rejection as its
    /// source. On a `Fail<D, L>` the result is `Fault<L>`. On a *bare*
    /// `Rejection` — a public method that returns just `R` because its
    /// caller can act on it, consumed by an internal frame that already
    /// proved the precondition — the result is the bare `Fatal`, and `?`
    /// carries it into whatever `Fault`/`Fail` the function returns, so the
    /// destination profile is never named at the call site:
    ///
    /// ```
    /// use errlanes::{Fault, ResultExt, lanes};
    ///
    /// #[derive(Debug, errlanes::Rejection)]
    /// #[rejection(code = "LANE_DISABLED")]
    /// struct LaneDisabled;
    ///
    /// fn listen() -> Result<(), LaneDisabled> {
    ///     Err(LaneDisabled)
    /// }
    ///
    /// // The lane was required at registration: by now, off is an invariant.
    /// fn run() -> Result<(), Fault<lanes!(Transient, Fatal)>> {
    ///     listen().narrow_rejected()?;
    ///     Ok(())
    /// }
    ///
    /// assert!(matches!(run(), Err(Fault::Fatal(_))));
    /// ```
    ///
    /// A `Fault` has no rejected lane to narrow, and that is a compile
    /// error, not an identity:
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
    fn rejected(self) -> Result<Result<T, E::Rejected>, Fault<E::Lanes>>
    where
        E: IntoLanes,
        E::Rejected: Rejection;

    /// Maps the rejection with a closure and leaves every other lane as it
    /// is: the `Result`-level form of [`Fail::map_rejected`]. For a
    /// type-level remap use [`widen`](Self::widen); this is for enriching a
    /// rejection with data only the call site has, such as the input that
    /// was attempted: `repo.create(new).await.map_rejected(|r| r.with_attempted(id))?`.
    fn map_rejected<D2>(self, f: impl FnOnce(E::Rejected) -> D2) -> Result<T, Fail<D2, E::Lanes>>
    where
        E: IntoLanes,
        E::Rejected: Rejection;

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

    fn rejected(self) -> Result<Result<T, E::Rejected>, Fault<E::Lanes>>
    where
        E: IntoLanes,
        E::Rejected: Rejection,
    {
        match self {
            Ok(value) => Ok(Ok(value)),
            Err(e) => e.into_lanes().rejected().map(Err),
        }
    }

    fn map_rejected<D2>(self, f: impl FnOnce(E::Rejected) -> D2) -> Result<T, Fail<D2, E::Lanes>>
    where
        E: IntoLanes,
        E::Rejected: Rejection,
    {
        self.map_err(|e| e.into_lanes().map_rejected(f))
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

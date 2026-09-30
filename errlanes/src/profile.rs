//! Sealed compile-time subsets of the three fault lanes.
use std::{convert::Infallible, error::Error, fmt::Debug};

use crate::{Denied, Exhausted, Fatal, Transient};

mod sealed {
    pub trait Sealed {}
}

/// A standard lane marker, or an uninhabited disabled slot.
#[doc(hidden)]
pub trait Slot<M>: From<Infallible> + Error + Clone + Send + Sync + 'static {
    fn marker(&self) -> &M;
}
impl<M: From<Infallible> + Error + Clone + Send + Sync + 'static> Slot<M> for M {
    fn marker(&self) -> &M {
        self
    }
}
macro_rules! disabled {
    ($($marker:ty),*) => {$ (
        impl Slot<$marker> for Infallible { fn marker(&self) -> &$marker { match *self {} } }
        impl From<Infallible> for $marker { fn from(value: Infallible) -> Self { match value {} } }
    )*};
}
disabled!(Denied, Transient, Fatal, Exhausted);

/// How a transient slot is consumed when retries stop: an enabled `Transient`
/// becomes `Fatal(Exhausted)` carrying the attempt count and the last transient
/// as its source; a disabled slot has no value to consume.
///
/// There is deliberately no `Settling<Infallible> for Transient`: a profile
/// that admits `Transient` but not `Fatal` claims an operation can be retried
/// but can never fail permanently, which is not true of anything worth
/// retrying. Such a profile is therefore not settleable, and — through the
/// [`crate::Laned`] impls — not retryable either.
#[doc(hidden)]
pub trait Settling<F> {
    fn settling(self, attempts: u32) -> F;
}
impl<F> Settling<F> for Infallible {
    fn settling(self, _: u32) -> F {
        match self {}
    }
}
impl Settling<Fatal> for Transient {
    fn settling(self, attempts: u32) -> Fatal {
        Fatal::from_error(
            crate::FatalKind::Exhausted,
            Exhausted {
                attempts,
                last: self,
            },
        )
    }
}

/// Sealed profile: disabled slots are `Infallible`, enabled slots are the
/// established markers. Use [`crate::lanes!`] to select a subset.
pub trait LaneProfile: sealed::Sealed + Debug + Clone + Send + Sync + 'static {
    type Denied: Slot<Denied>;
    type Transient: Slot<Transient>;
    type Fatal: Slot<Fatal>;

    /// This profile with the transient lane consumed. `Fail<D, L::Settled>` is
    /// what `settle`/`retry` hand back, and is what [`crate::Settled`] aliases.
    type Settled: LaneProfile<Denied = Self::Denied, Transient = Infallible, Fatal = Self::Fatal>;
}

#[derive(Debug, Clone, Copy)]
pub struct Profile<const D: bool, const T: bool, const F: bool>;
macro_rules! profile {
    ($d:literal, $t:literal, $f:literal; $denied:ty, $transient:ty, $fatal:ty) => {
        impl sealed::Sealed for Profile<$d, $t, $f> {}
        impl LaneProfile for Profile<$d, $t, $f> {
            type Denied = $denied;
            type Transient = $transient;
            type Fatal = $fatal;
            type Settled = Profile<$d, false, $f>;
        }
    };
}
profile!(false, false, false; Infallible, Infallible, Infallible);
profile!(false, false, true; Infallible, Infallible, Fatal);
profile!(false, true, false; Infallible, Transient, Infallible);
profile!(false, true, true; Infallible, Transient, Fatal);
profile!(true, false, false; Denied, Infallible, Infallible);
profile!(true, false, true; Denied, Infallible, Fatal);
profile!(true, true, false; Denied, Transient, Infallible);
profile!(true, true, true; Denied, Transient, Fatal);

pub type AllLanes = Profile<true, true, true>;

/// `L` with its transient lane consumed — an adjective on the *profile*, not a
/// second carrier enum. `Fail<R, Settled<L>>` and `Fault<Settled<L>>` are what
/// `settle`/`retry` return: ordinary `Fail`/`Fault` values whose `Transient`
/// slot is uninhabited, so a by-value match names only the lanes that remain
/// and an exhausted retry arrives as `Fatal(Exhausted)`.
pub type Settled<L> = <L as LaneProfile>::Settled;

/// Select a subset of `Denied`, `Transient`, and `Fatal`, in any order.
#[macro_export]
macro_rules! lanes {
    ($($lane:ident),* $(,)?) => { $crate::lanes!(@acc [false, false, false] $($lane,)*) };

    (@acc [$d:expr, $t:expr, $f:expr]) => { $crate::profile::Profile<{ $d }, { $t }, { $f }> };
    (@acc [$d:expr, $t:expr, $f:expr] Denied, $($rest:ident,)*) => {
        $crate::lanes!(@acc [true, $t, $f] $($rest,)*)
    };
    (@acc [$d:expr, $t:expr, $f:expr] Transient, $($rest:ident,)*) => {
        $crate::lanes!(@acc [$d, true, $f] $($rest,)*)
    };
    (@acc [$d:expr, $t:expr, $f:expr] Fatal, $($rest:ident,)*) => {
        $crate::lanes!(@acc [$d, $t, true] $($rest,)*)
    };
    // Both diagnostics expand to a well-formed `Profile`, so the only error
    // reported is the message itself -- no follow-on `(): LaneProfile`.
    (@acc [$d:expr, $t:expr, $f:expr] Rejected, $($rest:ident,)*) => {
        $crate::profile::Profile<{{
            compile_error!(
                "`Rejected` is not a fault lane: it is selected by the carrier, not the \
                 profile. Use `Fail<YourRejection, lanes!(..)>` instead of \
                 `Fault<lanes!(Rejected, ..)>`."
            );
            $d
        }}, { $t }, { $f }>
    };
    (@acc [$d:expr, $t:expr, $f:expr] $other:ident, $($rest:ident,)*) => {
        $crate::profile::Profile<{{
            compile_error!(concat!(
                "unknown lane `",
                stringify!($other),
                "`: expected `Denied`, `Transient`, or `Fatal`"
            ));
            $d
        }}, { $t }, { $f }>
    };
}

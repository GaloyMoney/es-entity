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

/// How a transient slot is narrowed away when retries stop: an enabled
/// `Transient` becomes `Fatal(Exhausted)` carrying the attempt count and the
/// last transient as its source; a disabled slot has no value to consume.
///
/// There is deliberately no `NarrowTransient<Infallible> for Transient`: a
/// profile that admits `Transient` but not `Fatal` claims an operation can be
/// retried but can never fail permanently, which is not true of anything
/// worth retrying. Such a profile is therefore not narrowable, and — through
/// the [`crate::Laned`] impls — not retryable either.
#[doc(hidden)]
pub trait NarrowTransient<F> {
    fn narrow(self, attempts: u32) -> F;
}
impl<F> NarrowTransient<F> for Infallible {
    fn narrow(self, _: u32) -> F {
        match self {}
    }
}
impl NarrowTransient<Fatal> for Transient {
    fn narrow(self, attempts: u32) -> Fatal {
        Fatal::from_error(
            crate::FatalKind::Exhausted,
            Exhausted {
                attempts,
                last: self,
            },
        )
    }
}

/// How a denied slot is narrowed away at a boundary with no subject: an
/// enabled `Denied` becomes `Fatal(Denied)` carrying itself as the source; a
/// disabled slot has no value to consume. As with `NarrowTransient`, there is
/// no `NarrowDenied<Infallible> for Denied`: a profile that can deny but can
/// never fail permanently has nowhere to put the narrowing.
#[doc(hidden)]
pub trait NarrowDenied<F> {
    fn narrow(self) -> F;
}
impl<F> NarrowDenied<F> for Infallible {
    fn narrow(self) -> F {
        match self {}
    }
}
impl NarrowDenied<Fatal> for Denied {
    fn narrow(self) -> Fatal {
        self.into_fatal()
    }
}

/// Sealed profile: disabled slots are `Infallible`, enabled slots are the
/// established markers. Use [`crate::lanes!`] to select a subset.
pub trait LaneProfile: sealed::Sealed + Debug + Clone + Send + Sync + 'static {
    type Denied: Slot<Denied>;
    type Transient: Slot<Transient>;
    type Fatal: Slot<Fatal>;

    /// This profile with the transient lane narrowed away: the projection
    /// `narrow_transient` and `retry` return through, spelled
    /// [`crate::profile::WithoutTransient`].
    type WithoutTransient: LaneProfile<Denied = Self::Denied, Transient = Infallible, Fatal = Self::Fatal>;

    /// This profile with the denied lane narrowed away: the projection
    /// `narrow_denied` returns through, spelled
    /// [`crate::profile::WithoutDenied`].
    type WithoutDenied: LaneProfile<Denied = Infallible, Transient = Self::Transient, Fatal = Self::Fatal>;
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
            type WithoutTransient = Profile<$d, false, $f>;
            type WithoutDenied = Profile<false, $t, $f>;
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

/// No lanes at all — the profile of a pure [`crate::Rejection`]: everything
/// about it is rejected, nothing is a fault.
pub type NoLanes = Profile<false, false, false>;

/// The lane-wise union of two profiles — `derive(Classify)` infers a
/// `delegate` variant's contribution to the enclosing `Lanes` this way, and
/// a `delegate` enum's full `Lanes` as the union of every variant's. Sealed,
/// closed over the eight profiles (no `generic_const_exprs`): each pair is
/// written out by [`union_impl`](self) below.
#[doc(hidden)]
pub trait Union<Other: LaneProfile>: LaneProfile {
    type Out: LaneProfile;
}
macro_rules! union_impl {
    ($d1:literal,$t1:literal,$f1:literal; $d2:literal,$t2:literal,$f2:literal) => {
        impl Union<Profile<$d2, $t2, $f2>> for Profile<$d1, $t1, $f1> {
            type Out = Profile<{ $d1 || $d2 }, { $t1 || $t2 }, { $f1 || $f2 }>;
        }
    };
}
union_impl!(false,false,false; false,false,false);
union_impl!(false,false,false; false,false,true);
union_impl!(false,false,false; false,true,false);
union_impl!(false,false,false; false,true,true);
union_impl!(false,false,false; true,false,false);
union_impl!(false,false,false; true,false,true);
union_impl!(false,false,false; true,true,false);
union_impl!(false,false,false; true,true,true);
union_impl!(false,false,true; false,false,false);
union_impl!(false,false,true; false,false,true);
union_impl!(false,false,true; false,true,false);
union_impl!(false,false,true; false,true,true);
union_impl!(false,false,true; true,false,false);
union_impl!(false,false,true; true,false,true);
union_impl!(false,false,true; true,true,false);
union_impl!(false,false,true; true,true,true);
union_impl!(false,true,false; false,false,false);
union_impl!(false,true,false; false,false,true);
union_impl!(false,true,false; false,true,false);
union_impl!(false,true,false; false,true,true);
union_impl!(false,true,false; true,false,false);
union_impl!(false,true,false; true,false,true);
union_impl!(false,true,false; true,true,false);
union_impl!(false,true,false; true,true,true);
union_impl!(false,true,true; false,false,false);
union_impl!(false,true,true; false,false,true);
union_impl!(false,true,true; false,true,false);
union_impl!(false,true,true; false,true,true);
union_impl!(false,true,true; true,false,false);
union_impl!(false,true,true; true,false,true);
union_impl!(false,true,true; true,true,false);
union_impl!(false,true,true; true,true,true);
union_impl!(true,false,false; false,false,false);
union_impl!(true,false,false; false,false,true);
union_impl!(true,false,false; false,true,false);
union_impl!(true,false,false; false,true,true);
union_impl!(true,false,false; true,false,false);
union_impl!(true,false,false; true,false,true);
union_impl!(true,false,false; true,true,false);
union_impl!(true,false,false; true,true,true);
union_impl!(true,false,true; false,false,false);
union_impl!(true,false,true; false,false,true);
union_impl!(true,false,true; false,true,false);
union_impl!(true,false,true; false,true,true);
union_impl!(true,false,true; true,false,false);
union_impl!(true,false,true; true,false,true);
union_impl!(true,false,true; true,true,false);
union_impl!(true,false,true; true,true,true);
union_impl!(true,true,false; false,false,false);
union_impl!(true,true,false; false,false,true);
union_impl!(true,true,false; false,true,false);
union_impl!(true,true,false; false,true,true);
union_impl!(true,true,false; true,false,false);
union_impl!(true,true,false; true,false,true);
union_impl!(true,true,false; true,true,false);
union_impl!(true,true,false; true,true,true);
union_impl!(true,true,true; false,false,false);
union_impl!(true,true,true; false,false,true);
union_impl!(true,true,true; false,true,false);
union_impl!(true,true,true; false,true,true);
union_impl!(true,true,true; true,false,false);
union_impl!(true,true,true; true,false,true);
union_impl!(true,true,true; true,true,false);
union_impl!(true,true,true; true,true,true);

/// `L` with its transient lane narrowed away — sugar for
/// [`LaneProfile::WithoutTransient`], which is what `narrow_transient` and
/// `retry` return through. Narrowing is a projection on the profile, not a
/// second family of carriers: `Fault<WithoutTransient<L>>` is an ordinary
/// `Fault` whose `Transient` slot is uninhabited, and an exhausted retry
/// arrives as `Fatal(Exhausted)`.
///
/// Prefer naming the resulting profile directly wherever you can — narrowing
/// `lanes!(Transient, Fatal)` yields `lanes!(Fatal)`, and writing that keeps a
/// lane the caller can never see out of the signature. This alias is for code
/// generic over `L`, where there is no concrete name to reach for.
pub type WithoutTransient<L> = <L as LaneProfile>::WithoutTransient;

/// `L` with its denied lane narrowed away — sugar for
/// [`LaneProfile::WithoutDenied`], which is what `narrow_denied` returns
/// through. Same caveat as [`WithoutTransient`]: prefer the concrete profile
/// name wherever one is available.
pub type WithoutDenied<L> = <L as LaneProfile>::WithoutDenied;

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

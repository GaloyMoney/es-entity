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

#[doc(hidden)]
pub trait TransientSlot: Slot<Transient> {
    type Exhausted: Slot<Exhausted>;
    fn settle(self, attempts: u32) -> Self::Exhausted;
}
impl TransientSlot for Transient {
    type Exhausted = Exhausted;
    fn settle(self, attempts: u32) -> Exhausted {
        Exhausted {
            attempts,
            last: self,
        }
    }
}
impl TransientSlot for Infallible {
    type Exhausted = Infallible;
    fn settle(self, _: u32) -> Infallible {
        match self {}
    }
}

/// Sealed profile: disabled slots are `Infallible`, enabled slots are the
/// established markers. Use [`crate::lanes!`] to select a subset.
pub trait LaneProfile: sealed::Sealed + Debug + Clone + Send + Sync + 'static {
    type Denied: Slot<Denied>;
    type Transient: TransientSlot;
    type Fatal: Slot<Fatal>;
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
/// Ordinary repositories can fail transiently or fatally, but cannot deny.
pub type RepoLanes = Profile<false, true, true>;

/// Select a subset of `Denied`, `Transient`, and `Fatal`, in any order.
#[macro_export]
macro_rules! lanes {
    ($($lane:ident),* $(,)?) => {
        $crate::profile::Profile<
            { false $(|| $crate::lanes!(@denied $lane))* },
            { false $(|| $crate::lanes!(@transient $lane))* },
            { false $(|| $crate::lanes!(@fatal $lane))* }
        >
    };
    (@denied Denied) => { true };
    (@denied Transient) => { false };
    (@denied Fatal) => { false };
    (@transient Denied) => { false };
    (@transient Transient) => { true };
    (@transient Fatal) => { false };
    (@fatal Denied) => { false };
    (@fatal Transient) => { false };
    (@fatal Fatal) => { true };
}

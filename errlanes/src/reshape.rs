//! Type-level lane edits: `WithDenied<RepoFault>`.
//!
//! The aliases here take a lane profile (`lanes!(..)`), a [`Fault`], a
//! [`Fail`], or a [`Carrier`], and enable or disable one lane on it. A
//! carrier reshapes to its built-in (`C::Repr`), never to a new enum.
//!
//! ```
//! use errlanes::{Fault, WithDenied, lanes};
//!
//! type MyFault = Fault<lanes!(Transient, Fatal)>;
//!
//! fn gate() -> Result<(), WithDenied<MyFault>> {
//!     Err(Fault::Denied(errlanes::Denied::new()))
//! }
//!
//! let _: Result<(), Fault<lanes!(Denied, Transient, Fatal)>> = gate();
//! ```
use crate::{Carrier, Fail, Fault, LaneProfile};

/// One lane edit per associated type; see the module docs. Implemented for
/// profiles, [`Fault`], [`Fail`], and every [`Carrier`].
pub trait Reshape {
    type WithDenied;
    type WithoutDenied;
    type WithTransient;
    type WithoutTransient;
    type WithFatal;
}

impl<L: LaneProfile> Reshape for Fault<L> {
    type WithDenied = Fault<L::WithDenied>;
    type WithoutDenied = Fault<L::WithoutDenied>;
    type WithTransient = Fault<L::WithTransient>;
    type WithoutTransient = Fault<L::WithoutTransient>;
    type WithFatal = Fault<L::WithFatal>;
}

impl<D, L: LaneProfile> Reshape for Fail<D, L> {
    type WithDenied = Fail<D, L::WithDenied>;
    type WithoutDenied = Fail<D, L::WithoutDenied>;
    type WithTransient = Fail<D, L::WithTransient>;
    type WithoutTransient = Fail<D, L::WithoutTransient>;
    type WithFatal = Fail<D, L::WithFatal>;
}

/// A carrier reshapes through its built-in.
impl<C: Carrier> Reshape for C
where
    C::Repr: Reshape,
{
    type WithDenied = <C::Repr as Reshape>::WithDenied;
    type WithoutDenied = <C::Repr as Reshape>::WithoutDenied;
    type WithTransient = <C::Repr as Reshape>::WithTransient;
    type WithoutTransient = <C::Repr as Reshape>::WithoutTransient;
    type WithFatal = <C::Repr as Reshape>::WithFatal;
}

/// `E` with the denied lane enabled.
pub type WithDenied<E> = <E as Reshape>::WithDenied;
/// `E` with the denied lane disabled (what `narrow_denied` returns).
pub type WithoutDenied<E> = <E as Reshape>::WithoutDenied;
/// `E` with the transient lane enabled.
pub type WithTransient<E> = <E as Reshape>::WithTransient;
/// `E` with the transient lane disabled (what `narrow_transient` returns).
pub type WithoutTransient<E> = <E as Reshape>::WithoutTransient;
/// `E` with the fatal lane enabled.
pub type WithFatal<E> = <E as Reshape>::WithFatal;

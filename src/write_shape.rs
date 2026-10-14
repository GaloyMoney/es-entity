//! Whether a repo's generated write ops can reject, as a type.
//!
//! A repo's `create`/`update` return `RepoWriteError<CV>` when they can hit a
//! constraint and the plain `RepoFault` when they cannot. A parent repo's
//! write also runs its nested children's writes, so the parent can reject if
//! its own statement can *or* any child's can. The derive of the parent cannot
//! see the child's options, so each repo publishes its shapes as
//! [`WriteShapes`] on its generated constraint violation enum and the parent
//! folds them with [`WriteShape::Or`]. The enum carries them rather than the
//! repo type because it is always `pub`: a public parent whose signatures
//! mention a less visible child repo would not compile.

use errlanes::UnmappedInto;

use crate::{Fail, Fatal, Fault, Lift, Rejection, RepoFault, RepoWriteError};

/// [`Rejects`] or [`Faults`].
pub trait WriteShape {
    /// The error a write of this shape returns for the constraint violation
    /// `CV`.
    type Error<CV: Rejection>;
    /// The shape of a write that performs a write of this shape followed by
    /// one of shape `S`: it rejects when either does.
    type Or<S: WriteShape>: WriteShape;
}

/// What a repo publishes, on its generated constraint violation enum, about its
/// own writes (nested children included): `Create` for
/// `create`/`create_all`, `Update` for `update`/`update_all`/`forget`/
/// `delete`. A parent's update also creates its new children, so it folds in
/// their `Create` as well.
#[doc(hidden)]
pub trait WriteShapes {
    type Create: WriteShape;
    type Update: WriteShape;
}

/// The write can hit a constraint: `RepoWriteError<CV>`.
pub struct Rejects;

/// The write can reject on nothing: `RepoFault`.
pub struct Faults;

impl WriteShape for Rejects {
    type Error<CV: Rejection> = RepoWriteError<CV>;
    type Or<S: WriteShape> = Rejects;
}

impl WriteShape for Faults {
    type Error<CV: Rejection> = RepoFault;
    type Or<S: WriteShape> = S;
}

/// Carries the error of a nested child repo's write into the parent's
/// vocabulary: a rejection is lifted into the parent's constraint violation, a
/// fault passes through.
pub trait LiftNestedWrite<ParentCv>: Sized {
    type Out;
    fn lift_nested(self) -> Self::Out;
}

impl<ChildCv, ParentCv> LiftNestedWrite<ParentCv> for RepoWriteError<ChildCv>
where
    ChildCv: Rejection,
    ParentCv: Rejection + Lift<ChildCv>,
    <ParentCv as Lift<ChildCv>>::Unmapped: UnmappedInto<Fatal>,
{
    type Out = RepoWriteError<ParentCv>;

    fn lift_nested(self) -> RepoWriteError<ParentCv> {
        Fail::lift::<ParentCv, errlanes::lanes!(Transient, Fatal)>(self)
    }
}

impl<ParentCv> LiftNestedWrite<ParentCv> for RepoFault {
    type Out = RepoFault;

    fn lift_nested(self) -> RepoFault {
        let fault: Fault<errlanes::lanes!(Transient, Fatal)> = self;
        fault
    }
}

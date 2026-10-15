//! Where a repo's entities get their id from, and the one public route into
//! an [`EntityEvents`] container.
//!
//! A repo resolves the id of every entity it creates according to its
//! [`IdSource`]: [`FromRepo`] (the default, the repo generates it) or
//! [`FromCaller`] (the caller puts it in the `New*` value). The
//! [`IntoEvents`] implementation of a `New*` type receives an
//! [`EntityEventsBuilder`] whose second parameter is that source, so
//! [`EntityEventsBuilder::id`] exists only when the repo resolved one, and a
//! [`FromRepo`] `New*` has no API through which a caller-chosen id can reach
//! the insert.
//!
//! [`IntoEvents`]: crate::IntoEvents

use crate::{events::EntityEvents, traits::EsEvent};

mod sealed {
    pub trait Sealed {}
}

/// Where the id of a newly created entity comes from. Sealed: [`FromRepo`]
/// and [`FromCaller`] are the only sources.
pub trait IdSource: sealed::Sealed {
    /// What the [`EntityEventsBuilder`] carries: the resolved id for
    /// [`FromRepo`], nothing for [`FromCaller`].
    type Slot<Id>;
}

/// The repo resolved the id before `into_events` runs; the `New*` type has no
/// id field and reads it with [`EntityEventsBuilder::id`].
pub struct FromRepo;
impl sealed::Sealed for FromRepo {}
impl IdSource for FromRepo {
    type Slot<Id> = Id;
}

/// The caller puts the id (a deterministic one) in the `New*` value, which
/// hands it to [`EntityEventsBuilder::initial_events`]. A caller's id is a
/// uniqueness claim, so the primary key stays among the repo's constraint
/// violations.
pub struct FromCaller;
impl sealed::Sealed for FromCaller {}
impl IdSource for FromCaller {
    type Slot<Id> = ();
}

/// The id source applied, no events yet. Handed to
/// [`IntoEvents::into_events`](crate::IntoEvents::into_events) by the
/// generated repo code and consumed by `initial_events`, the only public way
/// to build an [`EntityEvents`] container.
pub struct EntityEventsBuilder<E: EsEvent, S: IdSource> {
    id: S::Slot<E::EntityId>,
}

impl<E: EsEvent> EntityEventsBuilder<E, FromRepo> {
    #[doc(hidden)]
    pub fn __from_repo(id: E::EntityId) -> Self {
        Self { id }
    }

    /// The id the repo resolved for this entity.
    pub fn id(&self) -> E::EntityId {
        self.id.clone()
    }

    /// The entity's initial events, under the resolved id.
    pub fn initial_events(self, events: impl IntoIterator<Item = E>) -> EntityEvents<E> {
        EntityEvents::init(self.id, events)
    }
}

impl<E: EsEvent> EntityEventsBuilder<E, FromCaller> {
    #[doc(hidden)]
    pub fn __from_caller() -> Self {
        Self { id: () }
    }

    /// The entity's initial events under the id the caller put in the `New*`
    /// value.
    pub fn initial_events(
        self,
        id: E::EntityId,
        events: impl IntoIterator<Item = E>,
    ) -> EntityEvents<E> {
        EntityEvents::init(id, events)
    }
}

/// What `create_or_find_by_id_in_op` did for a repo with deterministic ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ensured<E> {
    /// This call inserted the entity.
    Created(E),
    /// The id already existed (a concurrent or earlier create won); this is
    /// the stored entity.
    Found(E),
}

impl<E> Ensured<E> {
    /// The entity, however it came about.
    pub fn into_inner(self) -> E {
        match self {
            Self::Created(entity) | Self::Found(entity) => entity,
        }
    }

    pub fn is_created(&self) -> bool {
        matches!(self, Self::Created(_))
    }
}

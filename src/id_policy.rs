//! How a repo's entities obtain their id, and the one public route into an
//! [`EntityEvents`] container.
//!
//! A repo resolves the id of every entity it creates according to its
//! [`IdPolicy`]: [`Generated`] (the default, the repo mints it) or
//! [`Supplied`] (the `New*` type brings its own). The [`IntoEvents`]
//! implementation of a `New*` type receives an [`EntityEventsBuilder`] whose
//! second parameter is that policy, so [`EntityEventsBuilder::id`] exists
//! only when the repo resolved one, and a [`Generated`] `New*` has no API
//! through which a caller-chosen id can reach the insert.
//!
//! [`IntoEvents`]: crate::IntoEvents

use crate::{events::EntityEvents, traits::EsEvent};

mod sealed {
    pub trait Sealed {}
}

/// Who decides the id of a newly created entity. Sealed: [`Generated`] and
/// [`Supplied`] are the only policies.
pub trait IdPolicy: sealed::Sealed {
    /// What the [`EntityEventsBuilder`] carries: the resolved id for
    /// [`Generated`], nothing for [`Supplied`].
    type Slot<Id>;
}

/// The repo resolved the id before `into_events` runs; the `New*` type has no
/// id field and reads it with [`EntityEventsBuilder::id`].
pub struct Generated;
impl sealed::Sealed for Generated {}
impl IdPolicy for Generated {
    type Slot<Id> = Id;
}

/// The `New*` type brings its own id (a deterministic one), which it hands to
/// [`EntityEventsBuilder::initial_events`]. A supplied id is a uniqueness
/// claim made by the caller, so the primary key stays among the repo's
/// constraint violations.
pub struct Supplied;
impl sealed::Sealed for Supplied {}
impl IdPolicy for Supplied {
    type Slot<Id> = ();
}

/// The identity policy applied, no events yet. Handed to
/// [`IntoEvents::into_events`](crate::IntoEvents::into_events) by the
/// generated repo code and consumed by `initial_events`, the only public way
/// to build an [`EntityEvents`] container.
pub struct EntityEventsBuilder<E: EsEvent, P: IdPolicy> {
    id: P::Slot<E::EntityId>,
}

impl<E: EsEvent> EntityEventsBuilder<E, Generated> {
    #[doc(hidden)]
    pub fn __generated(id: E::EntityId) -> Self {
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

impl<E: EsEvent> EntityEventsBuilder<E, Supplied> {
    #[doc(hidden)]
    pub fn __supplied() -> Self {
        Self { id: () }
    }

    /// The entity's initial events under the id the `New*` type brought.
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

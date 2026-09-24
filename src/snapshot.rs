//! Core types for entity snapshotting.
//!
//! A repo with `#[es_repo(snapshot)]` persists, in the same statement as the
//! events it appends, an author-defined state `S` (the fold of events
//! `1..=k`) into `<tbl>_snapshots`. Hydration then loads `S` plus only the
//! events after `k` in one round trip.

use chrono::{DateTime, Utc};

/// Matches no stored snapshot; binds on full-history loads and is
/// `NoSnapshot`'s fingerprint.
pub const NO_SNAPSHOT_FINGERPRINT: i64 = i64::MIN;

/// Implemented by `#[derive(EsSnapshot)]` for user state types and by hand for
/// [`NoSnapshot`].
pub trait EsSnapshot:
    serde::Serialize + serde::de::DeserializeOwned + std::fmt::Debug + PartialEq + Send + Sync + 'static
{
    /// `false` only for [`NoSnapshot`].
    const IS_SNAPSHOT: bool;
    const FINGERPRINT: i64;
    #[doc(hidden)]
    const HAS_FORGETTABLE_FIELDS: bool;
    /// JSON keys (field idents) of the top-level `Forgettable<T>` fields.
    #[doc(hidden)]
    const FORGETTABLE_JSON_FIELDS: &'static [&'static str];
    #[doc(hidden)]
    fn extract_forgettable_payloads(&self) -> Option<serde_json::Value>;
    #[doc(hidden)]
    fn forget_forgettable_payloads(&mut self);
}

/// The default `S` of `EntityEvents<E, S>`: this entity has no snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoSnapshot {}

impl serde::Serialize for NoSnapshot {
    fn serialize<Ser: serde::Serializer>(&self, _serializer: Ser) -> Result<Ser::Ok, Ser::Error> {
        match *self {}
    }
}

impl<'de> serde::Deserialize<'de> for NoSnapshot {
    fn deserialize<D: serde::Deserializer<'de>>(_deserializer: D) -> Result<Self, D::Error> {
        Err(serde::de::Error::custom(
            "NoSnapshot cannot be deserialized",
        ))
    }
}

impl EsSnapshot for NoSnapshot {
    const IS_SNAPSHOT: bool = false;
    const FINGERPRINT: i64 = NO_SNAPSHOT_FINGERPRINT;
    const HAS_FORGETTABLE_FIELDS: bool = false;
    const FORGETTABLE_JSON_FIELDS: &'static [&'static str] = &[];

    fn extract_forgettable_payloads(&self) -> Option<serde_json::Value> {
        match *self {}
    }

    fn forget_forgettable_payloads(&mut self) {
        match *self {}
    }
}

/// A persisted snapshot as loaded: the state plus the metadata of the events
/// it summarises.
pub struct SnapshotRecord<S> {
    /// Events `1..=sequence` are folded into `state`.
    pub sequence: usize,
    pub state: S,
    pub recorded_at: DateTime<Utc>,
    /// `recorded_at` of event 1 — keeps `entity_first_persisted_at()` O(1).
    pub first_recorded_at: DateTime<Utc>,
}

impl<S: std::fmt::Debug> std::fmt::Debug for SnapshotRecord<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnapshotRecord")
            .field("sequence", &self.sequence)
            .field("state", &self.state)
            .field("recorded_at", &self.recorded_at)
            .field("first_recorded_at", &self.first_recorded_at)
            .finish()
    }
}

impl<S: Clone> Clone for SnapshotRecord<S> {
    fn clone(&self) -> Self {
        Self {
            sequence: self.sequence,
            state: self.state.clone(),
            recorded_at: self.recorded_at,
            first_recorded_at: self.first_recorded_at,
        }
    }
}

impl<S: PartialEq> PartialEq for SnapshotRecord<S> {
    fn eq(&self, other: &Self) -> bool {
        self.sequence == other.sequence
            && self.state == other.state
            && self.recorded_at == other.recorded_at
            && self.first_recorded_at == other.first_recorded_at
    }
}

/// One item of an entity's replay. The snapshot, when present, is the FIRST
/// item in forward order and the LAST item in reverse order. Deliberately
/// exhaustive — do not add `#[non_exhaustive]`.
#[derive(Debug, Clone, Copy)]
pub enum Replay<'a, E, S> {
    Snapshot(&'a S),
    Event(&'a E),
}

/// Normalises `idempotency_guard!` input: a plain `&E` stream (today's call
/// sites) and a `replay()` stream both become [`Replay`]. Do not implement
/// for other types.
pub trait IntoReplay<'a, E, S> {
    fn into_replay(self) -> Replay<'a, E, S>;
}

impl<'a, E> IntoReplay<'a, E, NoSnapshot> for &'a E {
    fn into_replay(self) -> Replay<'a, E, NoSnapshot> {
        Replay::Event(self)
    }
}

impl<'a, E, S> IntoReplay<'a, E, S> for Replay<'a, E, S> {
    fn into_replay(self) -> Self {
        self
    }
}

/// Reached by `idempotency_guard!` when a stream yields `Replay::Snapshot`
/// but the guard has no `snapshot:` clause. Only [`NoSnapshot`] (and
/// `SnapshotRecord<NoSnapshot>`) implement it, so on a real snapshot this is
/// a compile error with the message below.
#[diagnostic::on_unimplemented(
    message = "`idempotency_guard!` over a snapshotted event stream needs a `snapshot: <pattern> [if <guard>]` clause",
    label = "this guard iterates `replay()` of an entity whose snapshot type is `{Self}`",
    note = "add `snapshot: s if <what the snapshot says about this operation>`; use `snapshot: _ if false` if the snapshot can never imply the operation was applied"
)]
pub trait GuardWithoutSnapshotClause {
    fn reached<R>(&self) -> R;
}

impl GuardWithoutSnapshotClause for NoSnapshot {
    fn reached<R>(&self) -> R {
        match *self {}
    }
}

impl GuardWithoutSnapshotClause for SnapshotRecord<NoSnapshot> {
    fn reached<R>(&self) -> R {
        match self.state {}
    }
}

/// Implemented by every entity whose repo enables `snapshot`.
///
/// The `#[derive(EsRepo)]` macro checks that the entity's own `Snapshot`
/// associated type and the repo's `snapshot` flag agree, so widening the
/// repo without also widening the entity's `EntityEvents<E, S>` (or the
/// reverse) does not compile:
///
/// ```ignore
/// impl HeadSnapshot for Meter {
///     fn capture(&self) -> Option<MeterSnapshot> {
///         (self.events.tail_len() >= 4).then(|| MeterSnapshot { .. })
///     }
/// }
/// ```
///
/// ```compile_fail
/// use es_entity::*;
/// use serde::{Serialize, Deserialize};
/// # fn main() {}
/// # es_entity::entity_id! { SnapGuardMeterId }
/// # #[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
/// # #[serde(tag = "type", rename_all = "snake_case")]
/// # #[es_event(id = "SnapGuardMeterId")]
/// # pub enum SnapGuardMeterEvent {
/// #     Initialized { id: SnapGuardMeterId },
/// # }
/// # pub struct NewSnapGuardMeter { id: SnapGuardMeterId }
/// # impl IntoEvents<SnapGuardMeterEvent> for NewSnapGuardMeter {
/// #     fn into_events(self) -> EntityEvents<SnapGuardMeterEvent> {
/// #         EntityEvents::init(self.id, [SnapGuardMeterEvent::Initialized { id: self.id }])
/// #     }
/// # }
/// // Missing: a real `Snapshot` — `events` stays
/// // `EntityEvents<SnapGuardMeterEvent>` (implicit `NoSnapshot`).
/// #[derive(EsEntity)]
/// pub struct SnapGuardMeter {
///     pub id: SnapGuardMeterId,
///     events: EntityEvents<SnapGuardMeterEvent>,
/// }
/// # impl TryFromEvents<SnapGuardMeterEvent> for SnapGuardMeter {
/// #     fn try_from_events(events: EntityEvents<SnapGuardMeterEvent>) -> Result<Self, EntityHydrationError> {
/// #         Ok(SnapGuardMeter { id: *events.id(), events })
/// #     }
/// # }
/// # impl HeadSnapshot for SnapGuardMeter {
/// #     fn capture(&self) -> Option<NoSnapshot> { None }
/// # }
/// // error: entity snapshot type and `#[es_repo(snapshot)]` disagree.
/// #[derive(EsRepo, Debug)]
/// #[es_repo(
///     entity = "SnapGuardMeter",
///     tbl = "meters",
///     events_tbl = "meter_events",
///     snapshot,
///     snapshot_tbl = "meter_snapshots"
/// )]
/// pub struct SnapGuardMeters {
///     pool: es_entity::db::Pool,
/// }
/// ```
///
/// A snapshot type with a `Forgettable<T>` field also requires the repo to
/// enable `forgettable` — otherwise the payload would never be scrubbed:
///
/// ```compile_fail
/// use es_entity::*;
/// use serde::{Serialize, Deserialize};
/// # fn main() {}
/// # es_entity::entity_id! { SnapGuardClientId }
/// # #[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
/// # #[serde(tag = "type", rename_all = "snake_case")]
/// # #[es_event(id = "SnapGuardClientId")]
/// # pub enum SnapGuardClientEvent {
/// #     Initialized { id: SnapGuardClientId, email: Forgettable<String> },
/// # }
/// #[derive(EsSnapshot, Debug, Clone, PartialEq, Serialize, Deserialize)]
/// pub struct SnapGuardClientSnapshot {
///     pub id: SnapGuardClientId,
///     pub email: Forgettable<String>,
/// }
/// # pub struct NewSnapGuardClient { id: SnapGuardClientId, email: String }
/// # impl IntoEvents<SnapGuardClientEvent> for NewSnapGuardClient {
/// #     fn into_events(self) -> EntityEvents<SnapGuardClientEvent> {
/// #         EntityEvents::init(
/// #             self.id,
/// #             [SnapGuardClientEvent::Initialized { id: self.id, email: Forgettable::new(self.email) }],
/// #         )
/// #     }
/// # }
/// # #[derive(EsEntity)]
/// # pub struct SnapGuardClient {
/// #     pub id: SnapGuardClientId,
/// #     events: EntityEvents<SnapGuardClientEvent, SnapGuardClientSnapshot>,
/// # }
/// # impl HeadSnapshot for SnapGuardClient {
/// #     fn capture(&self) -> Option<SnapGuardClientSnapshot> { None }
/// # }
/// # impl TryFromEvents<SnapGuardClientEvent, SnapGuardClientSnapshot> for SnapGuardClient {
/// #     fn try_from_events(events: EntityEvents<SnapGuardClientEvent, SnapGuardClientSnapshot>) -> Result<Self, EntityHydrationError> {
/// #         Ok(SnapGuardClient { id: *events.id(), events })
/// #     }
/// # }
/// // error: snapshot type has Forgettable fields but this repo does not
/// // enable `forgettable`.
/// #[derive(EsRepo, Debug)]
/// #[es_repo(
///     entity = "SnapGuardClient",
///     tbl = "clients",
///     events_tbl = "client_events",
///     snapshot,
///     snapshot_tbl = "client_snapshots"
/// )]
/// pub struct SnapGuardClients {
///     pool: es_entity::db::Pool,
/// }
/// ```
pub trait HeadSnapshot: crate::EsEntity {
    /// Called on every write the entity passes through — with staged events,
    /// or with none when the loaded state had no matching snapshot. `Some(s)`
    /// = persist `s` as the snapshot at the current head; `None` = keep
    /// whatever snapshot exists and let the tail grow.
    fn capture(&self) -> Option<Self::Snapshot>;
}

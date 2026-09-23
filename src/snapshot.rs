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
pub trait Snapshotting: crate::EsEntity {
    /// Called by the repo on every write, after commands have staged their
    /// events. `Some` = persist this as the fold of everything up to the new
    /// head; `None` = keep the existing one.
    fn snapshot(&self) -> Option<Self::Snapshot>;
}

/// `verify_snapshot` found the snapshotted fold differs from the
/// full-history fold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotMismatch {
    pub full_history: String,
    pub snapshotted: String,
}

impl std::fmt::Display for SnapshotMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "snapshot mismatch: full-history fold = {}, snapshotted fold = {}",
            self.full_history, self.snapshotted
        )
    }
}

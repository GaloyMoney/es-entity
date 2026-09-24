#![allow(dead_code)]

use derive_builder::Builder;
use es_entity::*;
use serde::{Deserialize, Serialize};

es_entity::entity_id! { MeterId, SiteId }

#[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "MeterId")]
pub enum MeterEvent {
    Initialized {
        id: MeterId,
        site_id: SiteId,
        label: String,
    },
    ReadingRecorded {
        value: i64,
    },
    Reset,
}

#[derive(EsSnapshot, Debug, Clone, Serialize, Deserialize)]
#[es_snapshot(version = 1)]
pub struct MeterSnapshot {
    pub id: MeterId,
    pub site_id: SiteId,
    pub label: String,
    pub count: u32,
    pub total: i64,
    pub last_value: Option<i64>,
    pub resets: u32,
}

/// The snapshotted entity: `capture()` fires once the tail reaches 4 events.
#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct Meter {
    pub id: MeterId,
    pub site_id: SiteId,
    pub label: String,
    events: EntityEvents<MeterEvent, MeterSnapshot>,
}

impl Meter {
    pub fn count(&self) -> u32 {
        self.events.replay().fold(0, |acc, r| match r {
            Replay::Snapshot(s) => s.count,
            Replay::Event(MeterEvent::ReadingRecorded { .. }) => acc + 1,
            Replay::Event(_) => acc,
        })
    }

    pub fn total(&self) -> i64 {
        self.events.replay().fold(0, |acc, r| match r {
            Replay::Snapshot(s) => s.total,
            Replay::Event(MeterEvent::ReadingRecorded { value }) => acc + value,
            Replay::Event(_) => acc,
        })
    }

    pub fn last_value(&self) -> Option<i64> {
        self.events.replay().rev().find_map(|r| match r {
            Replay::Event(MeterEvent::ReadingRecorded { value }) => Some(*value),
            Replay::Event(MeterEvent::Reset) => None,
            Replay::Snapshot(s) => s.last_value,
            Replay::Event(_) => None,
        })
    }

    pub fn resets(&self) -> u32 {
        self.events.replay().fold(0, |acc, r| match r {
            Replay::Snapshot(s) => s.resets,
            Replay::Event(MeterEvent::Reset) => acc + 1,
            Replay::Event(_) => acc,
        })
    }

    pub fn tail_len(&self) -> usize {
        self.events.tail_len()
    }

    pub fn has_snapshot(&self) -> bool {
        self.events.snapshot().is_some()
    }

    pub fn len_persisted(&self) -> usize {
        self.events.len_persisted()
    }

    pub fn record(&mut self, value: i64) -> Idempotent<()> {
        idempotency_guard!(
            self.events.replay().rev(),
            already_applied: MeterEvent::ReadingRecorded { value: v } if *v == value,
            resets_on: MeterEvent::ReadingRecorded { .. },
            snapshot: s if s.last_value == Some(value),
        );
        self.events.push(MeterEvent::ReadingRecorded { value });
        Idempotent::Executed(())
    }

    /// Deliberately simple forward guard: once ever reset, additional
    /// `reset()` calls are no-ops. Exercises a forward scan where the
    /// snapshot is the *first* replay item.
    pub fn reset(&mut self) -> Idempotent<()> {
        idempotency_guard!(
            self.events.replay(),
            already_applied: MeterEvent::Reset,
            snapshot: s if s.resets > 0,
        );
        self.events.push(MeterEvent::Reset);
        Idempotent::Executed(())
    }
}

impl HeadSnapshot for Meter {
    fn capture(&self) -> Option<MeterSnapshot> {
        (self.events.tail_len() >= 4).then(|| MeterSnapshot {
            id: self.id,
            site_id: self.site_id,
            label: self.label.clone(),
            count: self.count(),
            total: self.total(),
            last_value: self.last_value(),
            resets: self.resets(),
        })
    }
}

impl TryFromEvents<MeterEvent, MeterSnapshot> for Meter {
    fn try_from_events(
        events: EntityEvents<MeterEvent, MeterSnapshot>,
    ) -> Result<Self, EntityHydrationError> {
        let mut builder = MeterBuilder::default();
        for r in events.replay() {
            match r {
                Replay::Snapshot(s) => {
                    builder = builder.id(s.id).site_id(s.site_id).label(s.label.clone());
                }
                Replay::Event(MeterEvent::Initialized { id, site_id, label }) => {
                    builder = builder.id(*id).site_id(*site_id).label(label.clone());
                }
                Replay::Event(_) => {}
            }
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Builder)]
pub struct NewMeter {
    pub id: MeterId,
    pub site_id: SiteId,
    #[builder(setter(into))]
    pub label: String,
}

impl NewMeter {
    pub fn builder() -> NewMeterBuilder {
        NewMeterBuilder::default()
    }
}

impl IntoEvents<MeterEvent> for NewMeter {
    fn into_events(self) -> EntityEvents<MeterEvent> {
        EntityEvents::init(
            self.id,
            [MeterEvent::Initialized {
                id: self.id,
                site_id: self.site_id,
                label: self.label,
            }],
        )
    }
}

/// The non-snapshotted twin: same events, `iter_all()` still works. Reuses
/// `NewMeter` as its `New` type (both implement `IntoEvents<MeterEvent>`).
#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
#[es_entity(event = "MeterEvent", new = "NewMeter")]
pub struct PlainMeter {
    pub id: MeterId,
    pub site_id: SiteId,
    pub label: String,
    events: EntityEvents<MeterEvent>,
}

impl PlainMeter {
    pub fn count(&self) -> u32 {
        self.events
            .iter_all()
            .filter(|e| matches!(e, MeterEvent::ReadingRecorded { .. }))
            .count() as u32
    }

    pub fn total(&self) -> i64 {
        self.events.iter_all().fold(0, |acc, e| match e {
            MeterEvent::ReadingRecorded { value } => acc + value,
            _ => acc,
        })
    }

    pub fn last_value(&self) -> Option<i64> {
        self.events.iter_all().rev().find_map(|e| match e {
            MeterEvent::ReadingRecorded { value } => Some(*value),
            MeterEvent::Reset => None,
            _ => None,
        })
    }

    pub fn record(&mut self, value: i64) -> Idempotent<()> {
        idempotency_guard!(
            self.events.iter_all().rev(),
            already_applied: MeterEvent::ReadingRecorded { value: v } if *v == value,
            resets_on: MeterEvent::ReadingRecorded { .. },
        );
        self.events.push(MeterEvent::ReadingRecorded { value });
        Idempotent::Executed(())
    }
}

impl TryFromEvents<MeterEvent> for PlainMeter {
    fn try_from_events(events: EntityEvents<MeterEvent>) -> Result<Self, EntityHydrationError> {
        let mut builder = PlainMeterBuilder::default();
        for e in events.iter_all() {
            if let MeterEvent::Initialized { id, site_id, label } = e {
                builder = builder.id(*id).site_id(*site_id).label(label.clone());
            }
        }
        builder.events(events).build()
    }
}

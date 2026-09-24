#![allow(dead_code)]

use derive_builder::Builder;
use es_entity::*;
use serde::{Deserialize, Serialize};

use super::meter::{Meter, MeterId, NewMeter, PlainMeter, SiteId};

#[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "SiteId")]
pub enum SiteEvent {
    Initialized { id: SiteId },
}

#[derive(EsSnapshot, Debug, Clone, Serialize, Deserialize)]
#[es_snapshot(version = 1)]
pub struct SiteSnapshot {
    pub id: SiteId,
}

impl TryFromEvents<SiteEvent, SiteSnapshot> for Site {
    fn try_from_events(
        events: EntityEvents<SiteEvent, SiteSnapshot>,
    ) -> Result<Self, EntityHydrationError> {
        let mut builder = SiteBuilder::default();
        for r in events.replay() {
            match r {
                Replay::Snapshot(s) => builder = builder.id(s.id),
                Replay::Event(SiteEvent::Initialized { id }) => builder = builder.id(*id),
            }
        }
        builder.events(events).build()
    }
}

/// Both parent and child snapshot.
#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct Site {
    pub id: SiteId,
    events: EntityEvents<SiteEvent, SiteSnapshot>,

    #[es_entity(nested)]
    #[builder(default)]
    pub meters: Nested<Meter>,
}

impl Site {
    pub fn add_meter(&mut self, meter: NewMeter) {
        self.meters.add_new(meter);
    }
}

impl HeadSnapshot for Site {
    fn capture(&self) -> Option<SiteSnapshot> {
        Some(SiteSnapshot { id: self.id })
    }
}

/// Parent snapshots, child does not.
impl TryFromEvents<SiteEvent, SiteSnapshot> for SiteOverPlainMeters {
    fn try_from_events(
        events: EntityEvents<SiteEvent, SiteSnapshot>,
    ) -> Result<Self, EntityHydrationError> {
        let mut builder = SiteOverPlainMetersBuilder::default();
        for r in events.replay() {
            match r {
                Replay::Snapshot(s) => builder = builder.id(s.id),
                Replay::Event(SiteEvent::Initialized { id }) => builder = builder.id(*id),
            }
        }
        builder.events(events).build()
    }
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
#[es_entity(event = "SiteEvent", new = "NewSite")]
pub struct SiteOverPlainMeters {
    pub id: SiteId,
    events: EntityEvents<SiteEvent, SiteSnapshot>,

    #[es_entity(nested)]
    #[builder(default)]
    pub meters: Nested<PlainMeter>,
}

impl SiteOverPlainMeters {
    pub fn add_meter(&mut self, meter: NewMeter) {
        self.meters.add_new(meter);
    }
}

impl HeadSnapshot for SiteOverPlainMeters {
    fn capture(&self) -> Option<SiteSnapshot> {
        Some(SiteSnapshot { id: self.id })
    }
}

/// Child snapshots, parent does not.
impl TryFromEvents<SiteEvent> for PlainSiteOverMeters {
    fn try_from_events(events: EntityEvents<SiteEvent>) -> Result<Self, EntityHydrationError> {
        let mut builder = PlainSiteOverMetersBuilder::default();
        for e in events.iter_all() {
            let SiteEvent::Initialized { id } = e;
            builder = builder.id(*id);
        }
        builder.events(events).build()
    }
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
#[es_entity(event = "SiteEvent", new = "NewSite")]
pub struct PlainSiteOverMeters {
    pub id: SiteId,
    events: EntityEvents<SiteEvent>,

    #[es_entity(nested)]
    #[builder(default)]
    pub meters: Nested<Meter>,
}

impl PlainSiteOverMeters {
    pub fn add_meter(&mut self, meter: NewMeter) {
        self.meters.add_new(meter);
    }
}

#[derive(Debug, Builder)]
pub struct NewSite {
    pub id: SiteId,
}

impl NewSite {
    pub fn builder() -> NewSiteBuilder {
        NewSiteBuilder::default()
    }
}

impl IntoEvents<SiteEvent> for NewSite {
    fn into_events(self) -> EntityEvents<SiteEvent> {
        EntityEvents::init(self.id, [SiteEvent::Initialized { id: self.id }])
    }
}

pub fn new_meter(id: MeterId, site_id: SiteId, label: &str) -> NewMeter {
    NewMeter::builder()
        .id(id)
        .site_id(site_id)
        .label(label)
        .build()
        .unwrap()
}

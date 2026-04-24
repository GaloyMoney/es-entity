#![allow(dead_code)]

use derive_builder::Builder;
use serde::{Deserialize, Serialize};

use es_entity::*;

es_entity::entity_id! { TaskId }
es_entity::entity_id! { WorkspaceId }

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "TaskId")]
pub enum TaskEvent {
    Initialized {
        id: TaskId,
        workspace_id: Option<WorkspaceId>,
        status: String,
    },
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct Task {
    pub id: TaskId,
    #[builder(default)]
    pub workspace_id: Option<WorkspaceId>,
    pub status: String,

    events: EntityEvents<TaskEvent>,
}

impl TryFromEvents<TaskEvent> for Task {
    fn try_from_events(events: EntityEvents<TaskEvent>) -> Result<Self, EntityHydrationError> {
        let mut builder = TaskBuilder::default();
        for event in events.iter_all() {
            match event {
                TaskEvent::Initialized {
                    id,
                    workspace_id,
                    status,
                } => {
                    builder = builder
                        .id(*id)
                        .workspace_id(*workspace_id)
                        .status(status.clone());
                }
            }
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Builder)]
pub struct NewTask {
    #[builder(setter(into))]
    pub id: TaskId,
    #[builder(setter(into, strip_option), default)]
    pub workspace_id: Option<WorkspaceId>,
    #[builder(setter(into))]
    pub status: String,
}

impl NewTask {
    pub fn builder() -> NewTaskBuilder {
        NewTaskBuilder::default()
    }
}

impl IntoEvents<TaskEvent> for NewTask {
    fn into_events(self) -> EntityEvents<TaskEvent> {
        EntityEvents::init(
            self.id,
            [TaskEvent::Initialized {
                id: self.id,
                workspace_id: self.workspace_id,
                status: self.status,
            }],
        )
    }
}

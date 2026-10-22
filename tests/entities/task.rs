#![allow(dead_code)]

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

#[derive(EsEntity)]
pub struct Task {
    pub id: TaskId,
    pub workspace_id: Option<WorkspaceId>,
    pub status: String,

    events: EntityEvents<TaskEvent>,
}

impl TryFromEvents<TaskEvent> for Task {
    fn try_from_events(events: EntityEvents<TaskEvent>) -> Result<Self, EntityHydrationError> {
        let mut h = Task::hydrator();
        for event in events.iter_all() {
            match event {
                TaskEvent::Initialized {
                    id,
                    workspace_id,
                    status,
                } => {
                    h.id(*id)
                        .maybe_workspace_id(*workspace_id)
                        .status(status.clone());
                }
            }
        }
        h.build(events)
    }
}

#[derive(Debug, bon::Builder)]
pub struct NewTask {
    #[builder(into)]
    pub workspace_id: Option<WorkspaceId>,
    #[builder(into)]
    pub status: String,
}

impl IntoEvents<TaskEvent> for NewTask {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<TaskEvent, FromRepo>,
    ) -> EntityEvents<TaskEvent> {
        let id = builder.id();
        builder.initial_events([TaskEvent::Initialized {
            id,
            workspace_id: self.workspace_id,
            status: self.status,
        }])
    }
}

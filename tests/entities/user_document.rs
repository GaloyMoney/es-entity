#![allow(dead_code)]

use derive_builder::Builder;
use serde::{Deserialize, Serialize};

use es_entity::*;

es_entity::entity_id! { UserDocumentId }

use super::user::UserId;

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "UserDocumentId")]
pub enum UserDocumentEvent {
    Initialized {
        id: UserDocumentId,
        user_id: Option<UserId>,
    },
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct UserDocument {
    pub id: UserDocumentId,
    pub user_id: Option<UserId>,

    events: EntityEvents<UserDocumentEvent>,
}

impl TryFromEvents<UserDocumentEvent> for UserDocument {
    fn try_from_events(
        events: EntityEvents<UserDocumentEvent>,
    ) -> Result<Self, EntityHydrationError> {
        let mut builder = UserDocumentBuilder::default();
        for event in events.iter_all() {
            match event {
                UserDocumentEvent::Initialized { id, user_id } => {
                    builder = builder.id(*id).user_id(*user_id);
                }
            }
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Builder)]
pub struct NewUserDocument {
    #[builder(setter(into))]
    pub id: UserDocumentId,
    pub user_id: Option<UserId>,
}

impl NewUserDocument {
    pub fn builder() -> NewUserDocumentBuilder {
        NewUserDocumentBuilder::default()
    }
}

impl IntoEvents<UserDocumentEvent> for NewUserDocument {
    fn into_events(self) -> EntityEvents<UserDocumentEvent> {
        EntityEvents::init(
            self.id,
            [UserDocumentEvent::Initialized {
                id: self.id,
                user_id: self.user_id,
            }],
        )
    }
}

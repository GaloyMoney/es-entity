#![allow(dead_code)]

use derive_builder::Builder;
use serde::{Deserialize, Serialize};

use es_entity::*;

es_entity::entity_id! { UserId }

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "UserId", event_context)]
pub enum UserEvent {
    Initialized { id: UserId, name: String },
    NameUpdated { name: String },
}

pub struct ToyEncryptedUserEventPayloadCodec;

impl EventPayloadCodec<UserEvent> for ToyEncryptedUserEventPayloadCodec {
    fn encode(
        context: EventPayloadCodecContext<'_, UserId>,
        event: &UserEvent,
    ) -> Result<serde_json::Value, serde_json::Error> {
        let bytes = serde_json::to_vec(event)?;
        let key = toy_key(context.sequence);
        Ok(serde_json::json!({
            "codec": "toy-xor-hex-v1",
            "ciphertext": encode_hex(bytes.into_iter().map(|byte| byte ^ key)),
        }))
    }

    fn decode(
        context: EventPayloadCodecContext<'_, UserId>,
        payload: serde_json::Value,
    ) -> Result<UserEvent, serde_json::Error> {
        #[derive(Deserialize)]
        struct Envelope {
            ciphertext: String,
        }

        let envelope: Envelope = serde_json::from_value(payload)?;
        let key = toy_key(context.sequence);
        let plaintext = decode_hex(&envelope.ciphertext)?
            .into_iter()
            .map(|byte| byte ^ key)
            .collect::<Vec<_>>();
        serde_json::from_slice(&plaintext)
    }
}

fn toy_key(sequence: usize) -> u8 {
    0xa5 ^ (sequence as u8)
}

fn encode_hex(bytes: impl IntoIterator<Item = u8>) -> String {
    bytes
        .into_iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn decode_hex(hex: &str) -> Result<Vec<u8>, serde_json::Error> {
    use serde::de::Error as _;

    if hex.len() % 2 != 0 {
        return Err(serde_json::Error::custom("invalid hex length"));
    }

    hex.as_bytes()
        .chunks_exact(2)
        .map(|chunk| {
            std::str::from_utf8(chunk)
                .map_err(|e| serde_json::Error::custom(e.to_string()))
                .and_then(|digits| {
                    u8::from_str_radix(digits, 16)
                        .map_err(|e| serde_json::Error::custom(e.to_string()))
                })
        })
        .collect()
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct User {
    pub id: UserId,
    pub name: String,

    events: EntityEvents<UserEvent>,
}

impl User {
    pub fn update_name(&mut self, new_name: impl Into<String>) -> Idempotent<()> {
        let new_name = new_name.into();
        idempotency_guard!(
            self.events.iter_all().rev(),
            already_applied: UserEvent::NameUpdated { name } if name == &new_name,
            resets_on: UserEvent::NameUpdated { .. }
        );

        self.name = new_name.clone();
        self.events.push(UserEvent::NameUpdated { name: new_name });

        Idempotent::Executed(())
    }
}

impl TryFromEvents<UserEvent> for User {
    fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EntityHydrationError> {
        let mut builder = UserBuilder::default();
        for event in events.iter_all() {
            match event {
                UserEvent::Initialized { id, name } => {
                    builder = builder.id(*id).name(name.clone());
                }
                UserEvent::NameUpdated { name } => {
                    builder = builder.name(name.clone());
                }
            }
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Builder)]
pub struct NewUser {
    #[builder(setter(into))]
    pub id: UserId,
    #[builder(setter(into))]
    pub name: String,
}

impl NewUser {
    pub fn builder() -> NewUserBuilder {
        NewUserBuilder::default()
    }
}

impl IntoEvents<UserEvent> for NewUser {
    fn into_events(self) -> EntityEvents<UserEvent> {
        EntityEvents::init(
            self.id,
            [UserEvent::Initialized {
                id: self.id,
                name: self.name,
            }],
        )
    }
}

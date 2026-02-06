# Idempotency

Idempotency means that performing the same operation multiple times has the same effect as doing it once.
It's used to ensure that retrying a request doesn't cause unintended side effects, such as duplicated `Event`s being persisted.

This is particularly important in distributed systems where operations could be triggered from an asynchronous event queue (ie pub-sub).
Whenever you would like to have an `exactly-once` processing guarantee - you can easily achieve an `effectively-once` processing by ensuring your mutations are all idempotent.

Making your `Entity` mutations idempotent is very simple when doing Event Sourcing as you can easily check if the event you are about to append already exists in the history.

## Example

To see the issue in action - lets consider the `update_name` mutation without an idempotency check.

```rust
pub enum UserEvent {
    Initialized { id: u64, name: String },
    NameUpdated { name: String },
}

pub struct User {
    events: Vec<UserEvent>
}

impl User {
    pub fn update_name(&mut self, new_name: impl Into<String>) {
        let name = new_name.into();
        self.events.push(UserEvent::NameUpdated { name });
    }
}
```

In the above code we could easily record redundant events by calling the `update_name` mutation multiple times with the same input.
```rust
# pub enum UserEvent {
#     Initialized { id: u64, name: String },
#     NameUpdated { name: String },
# }
# pub struct User {
#     events: Vec<UserEvent>
# }
# impl User {
#     pub fn update_name(&mut self, new_name: impl Into<String>) {
#         let name = new_name.into();
#         self.events.push(UserEvent::NameUpdated { name });
#     }
# }

fn main() {
    let mut user = User { events: vec![] };
    user.update_name("Harrison");

    // Causes a redundant event to be appended
    user.update_name("Harrison");

    assert_eq!(user.events.len(), 2);
}
```

To prevent this we can iterate through the events to check if it has already been applied:

```rust
# pub enum UserEvent {
#     Initialized { id: u64, name: String },
#     NameUpdated { name: String },
# }
# pub struct User {
#     events: Vec<UserEvent>
# }
impl User {
    pub fn update_name(&mut self, new_name: impl Into<String>) {
        let name = new_name.into();
        for event in self.events.iter().rev() {
            match event {
                UserEvent::NameUpdated { name: existing_name } if existing_name == &name => {
                    return;
                }
                _ => ()
            }
        }
        self.events.push(UserEvent::NameUpdated { name });
    }
}

fn main() {
    let mut user = User { events: vec![] };

    user.update_name("Harrison");

    // This update will be ignored
    user.update_name("Harrison");

    assert_eq!(user.events.len(), 1);
}
```

But now we just silently ignore the operation.
Better would be to signal back to the caller whether or not the operation was applied.
For that we use the `Idempotent` type:
```rust
# extern crate es_entity;
# pub enum UserEvent {
#     Initialized { id: u64, name: String },
#     NameUpdated { name: String },
# }
# pub struct User {
#     events: Vec<UserEvent>
# }
use es_entity::Idempotent;
// #[must_use]
// pub enum Idempotent<T> {
//     Executed(T),
//     AlreadyApplied,
// }

impl User {
    pub fn update_name(&mut self, new_name: impl Into<String>) -> Idempotent<()>{
        let name = new_name.into();
        for event in self.events.iter().rev() {
            match event {
                UserEvent::NameUpdated { name: existing_name } if existing_name == &name => {
                    return Idempotent::AlreadyApplied;
                }
                _ => ()
            }
        }
        self.events.push(UserEvent::NameUpdated { name });
        Idempotent::Executed(())
    }
}

fn main() {
    let mut user = User { events: vec![] };
    assert!(user.update_name("Harrison").did_execute());
    assert!(user.update_name("Harrison").was_already_applied());
}
```

To cut down on boilerplate this pattern of iterating the events to check if an event was already applied has been encoded into the `idempotency_guard!` macro.

The macro expects an iterator over `&PersistedEvent<T>` items, which you get from `EntityEvents::iter_persisted()`:

```rust
# extern crate es_entity;
# extern crate serde;
# extern crate derive_builder;
# extern crate sqlx;
# use serde::{Deserialize, Serialize};
# use derive_builder::Builder;
use es_entity::{idempotency_guard, Idempotent, *};
# es_entity::entity_id! { UserId }
#
# #[derive(EsEvent, Debug, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "UserId")]
# pub enum UserEvent {
#     Initialized { id: UserId, name: String },
#     NameUpdated { name: String },
# }
#
# pub struct NewUser { id: UserId, name: String }
# impl IntoEvents<UserEvent> for NewUser {
#     fn into_events(self) -> EntityEvents<UserEvent> {
#         EntityEvents::init(self.id, [UserEvent::Initialized { id: self.id, name: self.name }])
#     }
# }
#
# #[derive(EsEntity, Builder)]
# #[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
# pub struct User {
#     pub id: UserId,
#     pub name: String,
#     events: EntityEvents<UserEvent>,
# }

impl User {
    pub fn update_name(&mut self, new_name: impl Into<String>) -> Idempotent<()> {
        let name = new_name.into();
        idempotency_guard!(
            // Iterator over persisted events (reversed for most-recent-first)
            self.events.iter_persisted().rev(),
            // Pattern match to check whether operation was already applied
            UserEvent::NameUpdated { name: existing_name } if existing_name == &name
        );
        self.events.push(UserEvent::NameUpdated { name });
        Idempotent::Executed(())
    }
}
#
# impl TryFromEvents<UserEvent> for User {
#     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EsEntityError> {
#         let mut builder = UserBuilder::default();
#         for event in events.iter_all() {
#             match event {
#                 UserEvent::Initialized { id, name } => {
#                     builder = builder.id(*id).name(name.clone());
#                 }
#                 UserEvent::NameUpdated { name } => {
#                     builder = builder.name(name.clone());
#                 }
#             }
#         }
#         builder.events(events).build()
#     }
# }
```

Finally there is the case where an operation was applied in the past - but it is still legal to re-apply it.
Like changing a name back to what it originally was:

```rust
# extern crate es_entity;
# extern crate serde;
# extern crate derive_builder;
# extern crate sqlx;
# use serde::{Deserialize, Serialize};
# use derive_builder::Builder;
use es_entity::{idempotency_guard, Idempotent, *};
# es_entity::entity_id! { UserId }
#
# #[derive(EsEvent, Debug, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "UserId")]
# pub enum UserEvent {
#     Initialized { id: UserId, name: String },
#     NameUpdated { name: String },
# }
#
# pub struct NewUser { id: UserId, name: String }
# impl IntoEvents<UserEvent> for NewUser {
#     fn into_events(self) -> EntityEvents<UserEvent> {
#         EntityEvents::init(self.id, [UserEvent::Initialized { id: self.id, name: self.name }])
#     }
# }
#
# #[derive(EsEntity, Builder)]
# #[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
# pub struct User {
#     pub id: UserId,
#     pub name: String,
#     events: EntityEvents<UserEvent>,
# }

impl User {
    pub fn update_name(&mut self, new_name: impl Into<String>) -> Idempotent<()> {
        let name = new_name.into();
        idempotency_guard!(
            self.events.iter_persisted().rev(),
            UserEvent::NameUpdated { name: existing_name } if existing_name == &name,
            // The `=>` signifies the pattern where to stop the iteration.
            => UserEvent::NameUpdated { .. }
        );
        self.events.push(UserEvent::NameUpdated { name });
        Idempotent::Executed(())
    }
}
#
# impl TryFromEvents<UserEvent> for User {
#     fn try_from_events(events: EntityEvents<UserEvent>) -> Result<Self, EsEntityError> {
#         let mut builder = UserBuilder::default();
#         for event in events.iter_all() {
#             match event {
#                 UserEvent::Initialized { id, name } => {
#                     builder = builder.id(*id).name(name.clone());
#                 }
#                 UserEvent::NameUpdated { name } => {
#                     builder = builder.name(name.clone());
#                 }
#             }
#         }
#         builder.events(events).build()
#     }
# }
```

Without the `=>` argument, updating a name back to a previous value would be rejected as `AlreadyApplied`.

## Idempotency Keys

Sometimes pattern matching against event data isn't sufficient for idempotency checks.

Consider an accounting system where a user withdraws $100. If the network times out and the client retries, you receive two withdrawal requests for $100. Was the second request a retry of the first (and should be ignored), or a legitimate new withdrawal (and should be processed)? Pattern matching on the amount alone can't distinguish between these cases—you need an external identifier to detect the duplicate.

The `idempotency-key` feature extends the `idempotency_guard!` macro to also check for matching idempotency keys stored in event contexts.

### Enabling the Feature

Add the feature to your `Cargo.toml`:

```toml
[dependencies]
es-entity = { version = "...", features = ["idempotency-key"] }
```

Note: This feature automatically enables `event-context-enabled`, which stores context data with each event.

### Setting an Idempotency Key

Before performing a mutation, set an idempotency key in the current event context:

```rust
# extern crate es_entity;
use es_entity::EventContext;

fn main() {
    let mut ctx = EventContext::current();
    ctx.set_idempotency_key("request-12345");
}
```

The idempotency key will be stored in the context of any events created while this context is active.

### Using with idempotency_guard!

When the `idempotency-key` feature is enabled, the `idempotency_guard!` macro checks both:
1. **Idempotency key matches** - If the current context has an idempotency key set, it checks if any persisted event has a matching key in its context
2. **Pattern matches** - The existing pattern matching behavior

```rust
# extern crate es_entity;
# extern crate serde;
# extern crate derive_builder;
# extern crate sqlx;
# use serde::{Deserialize, Serialize};
# use derive_builder::Builder;
use es_entity::{idempotency_guard, Idempotent, EventContext, *};
# es_entity::entity_id! { OrderId, PaymentId }
# type Money = f64;
#
# #[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "OrderId")]
# pub enum OrderEvent {
#     Initialized { id: OrderId },
#     PaymentApplied { payment_id: PaymentId, amount: Money },
# }
#
# pub struct NewOrder { id: OrderId }
# impl IntoEvents<OrderEvent> for NewOrder {
#     fn into_events(self) -> EntityEvents<OrderEvent> {
#         EntityEvents::init(self.id, [OrderEvent::Initialized { id: self.id }])
#     }
# }
#
# #[derive(EsEntity, Builder)]
# #[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
# pub struct Order {
#     pub id: OrderId,
#     events: EntityEvents<OrderEvent>,
# }

impl Order {
    pub fn apply_payment(&mut self, payment_id: PaymentId, amount: Money) -> Idempotent<()> {
        // Set idempotency key from external request ID
        EventContext::current().set_idempotency_key(format!("payment-{}", payment_id));

        // Guard checks BOTH:
        // 1. Any persisted event with same idempotency key?
        // 2. Pattern match for same payment_id?
        idempotency_guard!(
            self.events.iter_persisted().rev(),
            OrderEvent::PaymentApplied { payment_id: pid, .. } if pid == &payment_id
        );

        self.events.push(OrderEvent::PaymentApplied { payment_id, amount });
        Idempotent::Executed(())
    }
}
#
# impl TryFromEvents<OrderEvent> for Order {
#     fn try_from_events(events: EntityEvents<OrderEvent>) -> Result<Self, EsEntityError> {
#         let mut builder = OrderBuilder::default();
#         for event in events.iter_all() {
#             match event {
#                 OrderEvent::Initialized { id } => {
#                     builder = builder.id(*id);
#                 }
#                 OrderEvent::PaymentApplied { .. } => {}
#             }
#         }
#         builder.events(events).build()
#     }
# }
```

### Break Pattern Behavior

When using the break pattern (`=>`) with the `idempotency-key` feature, the macro continues scanning all events for idempotency key matches even after the break pattern matches. This ensures that duplicate requests are always detected regardless of where they appear in the event history:

```rust
# extern crate es_entity;
# extern crate serde;
# extern crate derive_builder;
# extern crate sqlx;
# use serde::{Deserialize, Serialize};
# use derive_builder::Builder;
# use es_entity::{idempotency_guard, Idempotent, EventContext, *};
# es_entity::entity_id! { OrderId, PaymentId }
# type Money = f64;
#
# #[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "OrderId")]
# pub enum OrderEvent {
#     Initialized { id: OrderId },
#     PaymentApplied { payment_id: PaymentId, amount: Money },
# }
#
# pub struct NewOrder { id: OrderId }
# impl IntoEvents<OrderEvent> for NewOrder {
#     fn into_events(self) -> EntityEvents<OrderEvent> {
#         EntityEvents::init(self.id, [OrderEvent::Initialized { id: self.id }])
#     }
# }
#
# #[derive(EsEntity, Builder)]
# #[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
# pub struct Order {
#     pub id: OrderId,
#     events: EntityEvents<OrderEvent>,
# }

impl Order {
    pub fn apply_payment(&mut self, payment_id: PaymentId, amount: Money) -> Idempotent<()> {
        EventContext::current().set_idempotency_key(format!("payment-{}", payment_id));
        idempotency_guard!(
            self.events.iter_persisted().rev(),
            OrderEvent::PaymentApplied { payment_id: pid, .. } if pid == &payment_id,
            // Break pattern stops pattern matching but idempotency key checking continues
            => OrderEvent::PaymentApplied { .. }
        );
        self.events.push(OrderEvent::PaymentApplied { payment_id, amount });
        Idempotent::Executed(())
    }
}
#
# impl TryFromEvents<OrderEvent> for Order {
#     fn try_from_events(events: EntityEvents<OrderEvent>) -> Result<Self, EsEntityError> {
#         let mut builder = OrderBuilder::default();
#         for event in events.iter_all() {
#             match event {
#                 OrderEvent::Initialized { id } => {
#                     builder = builder.id(*id);
#                 }
#                 OrderEvent::PaymentApplied { .. } => {}
#             }
#         }
#         builder.events(events).build()
#     }
# }
```

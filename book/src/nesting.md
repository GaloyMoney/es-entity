# Nesting

Building on the aggregate example from the previous chapter, let's implement the nested approach for our `Subscription` and `BillingPeriod` entities.
As discussed, this approach makes the aggregate relationship explicit in the type system and ensures all access to nested entities is moderated through the aggregate root.

## Setting up the Database Tables

First, we need to create the tables for both the parent (`Subscription`) and nested (`BillingPeriod`) entities:

```sql
-- The parent entity table
CREATE TABLE subscriptions (
  id UUID PRIMARY KEY,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE subscription_events (
  id UUID NOT NULL REFERENCES subscriptions(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

-- The nested entity table
CREATE TABLE billing_periods (
  id UUID PRIMARY KEY,
  subscription_id UUID NOT NULL REFERENCES subscriptions(id),
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE billing_period_events (
  id UUID NOT NULL REFERENCES billing_periods(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
```

Note how the nested `index` table (`billing_periods`) includes a foreign key to the parent.

## Defining the Nested Entity

Let's start by implementing the `BillingPeriod` entity that will be nested inside `Subscription`.
There are no special requirements on the child `entity` and it can be setup just like always:

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# extern crate derive_builder;
# extern crate tokio;
# extern crate anyhow;
use derive_builder::Builder;
use es_entity::*;
use serde::{Deserialize, Serialize};

es_entity::entity_id! {
    SubscriptionId,
    BillingPeriodId
}

#[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "BillingPeriodId")]
pub enum BillingPeriodEvent {
    Initialized {
        id: BillingPeriodId,
        subscription_id: SubscriptionId,
    },
    LineItemAdded {
        amount: f64,
        description: String,
    },
    Closed,
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
pub struct BillingPeriod {
    pub id: BillingPeriodId,
    pub subscription_id: SubscriptionId,
    pub is_current: bool,
    pub line_items: Vec<LineItem>,
    events: EntityEvents<BillingPeriodEvent>,
}

#[derive(Debug, Clone)]
pub struct LineItem {
    pub amount: f64,
    pub description: String,
}

impl BillingPeriod {
    pub fn add_line_item(&mut self, amount: f64, description: String) -> Idempotent<usize> {
        idempotency_guard!(
            self.events.iter_all().rev(),
            BillingPeriodEvent::LineItemAdded { amount: a, description: d, .. }
                if a == &amount && d == &description
        );

        self.line_items.push(LineItem {
            amount,
            description: description.clone(),
        });

        self.events.push(BillingPeriodEvent::LineItemAdded {
            amount,
            description,
        });

        Idempotent::Executed(self.line_items.len())
    }

    pub fn close(&mut self) -> Idempotent<()> {
        idempotency_guard!(
            self.events.iter_all().rev(),
            BillingPeriodEvent::Closed
        );

        self.is_current = false;
        self.events.push(BillingPeriodEvent::Closed);

        Idempotent::Executed(())
    }
}

impl TryFromEvents<BillingPeriodEvent> for BillingPeriod {
    fn try_from_events(events: EntityEvents<BillingPeriodEvent>) -> Result<Self, EsEntityError> {
        let mut builder = BillingPeriodBuilder::default().is_current(true);
        let mut line_items = Vec::new();

        for event in events.iter_all() {
            match event {
                BillingPeriodEvent::Initialized { id, subscription_id } => {
                    builder = builder.id(*id).subscription_id(*subscription_id);
                }
                BillingPeriodEvent::LineItemAdded { amount, description } => {
                    line_items.push(LineItem {
                        amount: *amount,
                        description: description.clone(),
                    });
                }
                BillingPeriodEvent::Closed => {
                    builder = builder.is_current(false)
                }
            }
        }

        builder
            .line_items(line_items)
            .events(events)
            .build()
    }
}

#[derive(Debug, Clone, Builder)]
pub struct NewBillingPeriod {
    pub id: BillingPeriodId,
    pub subscription_id: SubscriptionId,
}

impl IntoEvents<BillingPeriodEvent> for NewBillingPeriod {
    fn into_events(self) -> EntityEvents<BillingPeriodEvent> {
        EntityEvents::init(
            self.id,
            vec![BillingPeriodEvent::Initialized {
                id: self.id,
                subscription_id: self.subscription_id,
            }],
        )
    }
}
```

## Defining the Parent Entity with Nested Children

Now let's implement the `Subscription` entity that will contain the nested `BillingPeriod` entities:

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# extern crate derive_builder;
# extern crate tokio;
# extern crate anyhow;
# use derive_builder::Builder;
# use es_entity::*;
# use serde::{Deserialize, Serialize};
#
# es_entity::entity_id! {
#     SubscriptionId,
#     BillingPeriodId
# }
#
# #[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "BillingPeriodId")]
# pub enum BillingPeriodEvent {
#     Initialized {
#         id: BillingPeriodId,
#         subscription_id: SubscriptionId,
#     },
#     LineItemAdded {
#         amount: f64,
#         description: String,
#     },
#     Closed,
# }
#
# #[derive(EsEntity, Builder)]
# #[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
# pub struct BillingPeriod {
#     pub id: BillingPeriodId,
#     pub subscription_id: SubscriptionId,
#     pub is_current: bool,
#     pub line_items: Vec<LineItem>,
#     events: EntityEvents<BillingPeriodEvent>,
# }
#
# #[derive(Debug, Clone)]
# pub struct LineItem {
#     pub amount: f64,
#     pub description: String,
# }
#
# impl BillingPeriod {
#     pub fn add_line_item(&mut self, amount: f64, description: String) -> Idempotent<usize> {
#         idempotency_guard!(
#             self.events.iter_all().rev(),
#             BillingPeriodEvent::LineItemAdded { amount: a, description: d, .. }
#                 if a == &amount && d == &description
#         );
#
#         self.line_items.push(LineItem {
#             amount,
#             description: description.clone(),
#         });
#
#         self.events.push(BillingPeriodEvent::LineItemAdded {
#             amount,
#             description,
#         });
#
#         Idempotent::Executed(self.line_items.len())
#     }
#
#     pub fn close(&mut self) -> Idempotent<()> {
#         idempotency_guard!(
#             self.events.iter_all().rev(),
#             BillingPeriodEvent::Closed
#         );
#
#         self.is_current = false;
#         self.events.push(BillingPeriodEvent::Closed);
#
#         Idempotent::Executed(())
#     }
# }
#
# impl TryFromEvents<BillingPeriodEvent> for BillingPeriod {
#     fn try_from_events(events: EntityEvents<BillingPeriodEvent>) -> Result<Self, EsEntityError> {
#         let mut builder = BillingPeriodBuilder::default().is_current(true);
#         let mut line_items = Vec::new();
#
#         for event in events.iter_all() {
#             match event {
#                 BillingPeriodEvent::Initialized { id, subscription_id } => {
#                     builder = builder.id(*id).subscription_id(*subscription_id);
#                 }
#                 BillingPeriodEvent::LineItemAdded { amount, description } => {
#                     line_items.push(LineItem {
#                         amount: *amount,
#                         description: description.clone(),
#                     });
#                 }
#                 BillingPeriodEvent::Closed => {
#                     builder = builder.is_current(false)
#                 }
#             }
#         }
#
#         builder
#             .line_items(line_items)
#             .events(events)
#             .build()
#     }
# }
#
# #[derive(Debug, Clone, Builder)]
# pub struct NewBillingPeriod {
#     pub id: BillingPeriodId,
#     pub subscription_id: SubscriptionId,
# }
#
# impl IntoEvents<BillingPeriodEvent> for NewBillingPeriod {
#     fn into_events(self) -> EntityEvents<BillingPeriodEvent> {
#         EntityEvents::init(
#             self.id,
#             vec![BillingPeriodEvent::Initialized {
#                 id: self.id,
#                 subscription_id: self.subscription_id,
#             }],
#         )
#     }
# }
#[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "SubscriptionId")]
pub enum SubscriptionEvent {
    Initialized { id: SubscriptionId },
    BillingPeriodStarted { period_id: BillingPeriodId },
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
pub struct Subscription {
    pub id: SubscriptionId,
    current_period_id: Option<BillingPeriodId>,
    events: EntityEvents<SubscriptionEvent>,

    // The `#[es_entity(nested)]` attribute marks this field as containing nested entities.
    // It must be of type `Nested<T>`.
    // The #[builder(default)] will initialize it as empty.
    // The Repository will load the children after the parent has been hydrated.
    #[es_entity(nested)]
    #[builder(default)]
    billing_periods: Nested<BillingPeriod>,
}

impl Subscription {
    pub fn start_new_billing_period(&mut self) -> Idempotent<BillingPeriodId> {
        // Close the current billing period if there is one
        if let Some(current_id) = self.current_period_id {
            if let Some(current_period) = self.billing_periods.get_persisted_mut(&current_id) {
                current_period.close();
            }
        }

        // Create the new billing period
        let new_period = NewBillingPeriod {
            id: BillingPeriodId::new(),
            subscription_id: self.id,
        };

        let id = new_period.id;
        self.billing_periods.add_new(new_period);

        // Update the current period tracking
        self.current_period_id = Some(id);
        self.events.push(SubscriptionEvent::BillingPeriodStarted { period_id: id });

        Idempotent::Executed(id)
    }

    pub fn add_line_item_to_current_billing_period(&mut self, amount: f64, description: String) -> Idempotent<usize> {
        // Use the tracked current period ID to access the billing period directly
        if let Some(current_id) = self.current_period_id {
            if let Some(current_period) = self.billing_periods.get_persisted_mut(&current_id) {
                return current_period.add_line_item(amount, description);
            }
        }

        Idempotent::Ignored
    }
}

impl TryFromEvents<SubscriptionEvent> for Subscription {
    fn try_from_events(events: EntityEvents<SubscriptionEvent>) -> Result<Self, EsEntityError> {
        let mut builder = SubscriptionBuilder::default();

        for event in events.iter_all() {
            match event {
                SubscriptionEvent::Initialized { id } => {
                    builder = builder.id(*id);
                }
                SubscriptionEvent::BillingPeriodStarted { period_id } => {
                    builder = builder.current_period_id(Some(*period_id));
                }
            }
        }

        builder
            .events(events)
            .build()
    }
}

#[derive(Debug, Clone, Builder)]
pub struct NewSubscription {
    pub id: SubscriptionId,
}

impl IntoEvents<SubscriptionEvent> for NewSubscription {
    fn into_events(self) -> EntityEvents<SubscriptionEvent> {
        EntityEvents::init(
            self.id,
            vec![SubscriptionEvent::Initialized { id: self.id }],
        )
    }
}
```

The key points to note:
1. The `billing_periods` field is marked with `#[es_entity(nested)]`
2. The field type is `Nested<BillingPeriod>` which is a special container for nested entities
3. We use `add_new()` to add new nested entities
4. We mutate the children via `get_persisted_mut()`.

Under the hood the `EsEntity` macro will create an implementation of the `Parent` trait:
```rust,ignore
pub trait Parent<T: EsEntity> {
    fn new_children_mut(&mut self) -> &mut Vec<<T as EsEntity>::New>;
    fn iter_persisted_children_mut<'a>(&'a mut self) -> impl Iterator<Item = &'a mut T>
    where
        T: 'a;
    fn inject_children(&mut self, entities: impl IntoIterator<Item = T>);
}
```

for every field marked `#[es_entity(nested)]`.


## Setting up the Repositories

The repository setup is where the magic happens for nested entities.
We need to configure both the parent and child repositories with special attributes.
It is recommended to put both Repositories in the same file but only mark the parent one as `pub`.
This leverages the rust module system to enforce that the children cannot be accessed directly.

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# extern crate derive_builder;
# extern crate tokio;
# extern crate anyhow;
# use derive_builder::Builder;
# use es_entity::*;
# use serde::{Deserialize, Serialize};
#
# es_entity::entity_id! {
#     SubscriptionId,
#     BillingPeriodId
# }
#
# #[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "BillingPeriodId")]
# pub enum BillingPeriodEvent {
#     Initialized {
#         id: BillingPeriodId,
#         subscription_id: SubscriptionId,
#     },
#     LineItemAdded {
#         amount: f64,
#         description: String,
#     },
#     Closed,
# }
#
# #[derive(EsEntity, Builder)]
# #[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
# pub struct BillingPeriod {
#     pub id: BillingPeriodId,
#     pub subscription_id: SubscriptionId,
#     pub is_current: bool,
#     pub line_items: Vec<LineItem>,
#     events: EntityEvents<BillingPeriodEvent>,
# }
#
# #[derive(Debug, Clone)]
# pub struct LineItem {
#     pub amount: f64,
#     pub description: String,
# }
#
# impl BillingPeriod {
#     pub fn add_line_item(&mut self, amount: f64, description: String) -> Idempotent<usize> {
#         idempotency_guard!(
#             self.events.iter_all().rev(),
#             BillingPeriodEvent::LineItemAdded { amount: a, description: d, .. }
#                 if a == &amount && d == &description
#         );
#
#         self.line_items.push(LineItem {
#             amount,
#             description: description.clone(),
#         });
#
#         self.events.push(BillingPeriodEvent::LineItemAdded {
#             amount,
#             description,
#         });
#
#         Idempotent::Executed(self.line_items.len())
#     }
#
#     pub fn close(&mut self) -> Idempotent<()> {
#         idempotency_guard!(
#             self.events.iter_all().rev(),
#             BillingPeriodEvent::Closed
#         );
#
#         self.is_current = false;
#         self.events.push(BillingPeriodEvent::Closed);
#
#         Idempotent::Executed(())
#     }
# }
#
# impl TryFromEvents<BillingPeriodEvent> for BillingPeriod {
#     fn try_from_events(events: EntityEvents<BillingPeriodEvent>) -> Result<Self, EsEntityError> {
#         let mut builder = BillingPeriodBuilder::default().is_current(true);
#         let mut line_items = Vec::new();
#
#         for event in events.iter_all() {
#             match event {
#                 BillingPeriodEvent::Initialized { id, subscription_id } => {
#                     builder = builder.id(*id).subscription_id(*subscription_id);
#                 }
#                 BillingPeriodEvent::LineItemAdded { amount, description } => {
#                     line_items.push(LineItem {
#                         amount: *amount,
#                         description: description.clone(),
#                     });
#                 }
#                 BillingPeriodEvent::Closed => {
#                     builder = builder.is_current(false)
#                 }
#             }
#         }
#
#         builder
#             .line_items(line_items)
#             .events(events)
#             .build()
#     }
# }
#
# #[derive(Debug, Clone, Builder)]
# pub struct NewBillingPeriod {
#     pub id: BillingPeriodId,
#     pub subscription_id: SubscriptionId,
# }
#
# impl IntoEvents<BillingPeriodEvent> for NewBillingPeriod {
#     fn into_events(self) -> EntityEvents<BillingPeriodEvent> {
#         EntityEvents::init(
#             self.id,
#             vec![BillingPeriodEvent::Initialized {
#                 id: self.id,
#                 subscription_id: self.subscription_id,
#             }],
#         )
#     }
# }
# #[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "SubscriptionId")]
# pub enum SubscriptionEvent {
#     Initialized { id: SubscriptionId },
#     BillingPeriodStarted { period_id: BillingPeriodId },
# }
#
# #[derive(EsEntity, Builder)]
# #[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
# pub struct Subscription {
#     pub id: SubscriptionId,
#     current_period_id: Option<BillingPeriodId>,
#     events: EntityEvents<SubscriptionEvent>,
#
#     #[es_entity(nested)]
#     #[builder(default)]
#     billing_periods: Nested<BillingPeriod>,
# }
#
# impl TryFromEvents<SubscriptionEvent> for Subscription {
#     fn try_from_events(events: EntityEvents<SubscriptionEvent>) -> Result<Self, EsEntityError> {
#         let mut builder = SubscriptionBuilder::default();
#
#         for event in events.iter_all() {
#             match event {
#                 SubscriptionEvent::Initialized { id } => {
#                     builder = builder.id(*id);
#                 }
#                 SubscriptionEvent::BillingPeriodStarted { period_id } => {
#                     builder = builder.current_period_id(Some(*period_id));
#                 }
#             }
#         }
#
#         builder
#             .events(events)
#             .build()
#     }
# }
#
# #[derive(Debug, Clone, Builder)]
# pub struct NewSubscription {
#     pub id: SubscriptionId,
# }
#
# impl IntoEvents<SubscriptionEvent> for NewSubscription {
#     fn into_events(self) -> EntityEvents<SubscriptionEvent> {
#         EntityEvents::init(
#             self.id,
#             vec![SubscriptionEvent::Initialized { id: self.id }],
#         )
#     }
# }
# fn main() {}
#[derive(EsRepo)]
#[es_repo(
    entity = "BillingPeriod",
    columns(
        // The foreign key of the parent marked by `parent`.
        subscription_id(ty = "SubscriptionId", update(persist = false), parent)
    )
)]
// private struct
struct BillingPeriods {
    pool: sqlx::PgPool,
}

#[derive(EsRepo)]
#[es_repo(entity = "Subscription")]
pub struct Subscriptions {
    pool: sqlx::PgPool,

    // Mark this field as containing the nested repository
    #[es_repo(nested)]
    billing_periods: BillingPeriods,
}

impl Subscriptions {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self {
            pool: pool.clone(),
            billing_periods: BillingPeriods { pool },
        }
    }
}
```

The important configuration here:
1. The child repository (`BillingPeriods`) marks the foreign key column with `parent`.
2. The parent repository (`Subscriptions`) includes the child repository as a field marked with `#[es_repo(nested)]`

## Using Nested Entities

Now we can use our aggregate with full type safety and automatic loading of nested entities:

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# extern crate derive_builder;
# extern crate tokio;
# extern crate anyhow;
# use derive_builder::Builder;
# use es_entity::*;
# use serde::{Deserialize, Serialize};
# es_entity::entity_id! {
#     SubscriptionId,
#     BillingPeriodId
# }
# #[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "BillingPeriodId")]
# pub enum BillingPeriodEvent {
#     Initialized {
#         id: BillingPeriodId,
#         subscription_id: SubscriptionId,
#     },
#     LineItemAdded {
#         amount: f64,
#         description: String,
#     },
#     Closed,
# }
# #[derive(EsEntity, Builder)]
# #[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
# pub struct BillingPeriod {
#     pub id: BillingPeriodId,
#     pub subscription_id: SubscriptionId,
#     pub is_current: bool,
#     pub line_items: Vec<LineItem>,
#     events: EntityEvents<BillingPeriodEvent>,
# }
# #[derive(Debug, Clone)]
# pub struct LineItem {
#     pub amount: f64,
#     pub description: String,
# }
# impl BillingPeriod {
#     pub fn add_line_item(&mut self, amount: f64, description: String) -> Idempotent<usize> {
#         if !self.is_current {
#             unreachable!()
#         }
#         idempotency_guard!(
#             self.events.iter_all().rev(),
#             BillingPeriodEvent::LineItemAdded { amount: a, description: d, .. }
#                 if a == &amount && d == &description
#         );
#         self.line_items.push(LineItem {
#             amount,
#             description: description.clone(),
#         });
#         self.events.push(BillingPeriodEvent::LineItemAdded {
#             amount,
#             description,
#         });
#         Idempotent::Executed(self.line_items.len())
#     }
#     pub fn close(&mut self) -> Idempotent<()> {
#         idempotency_guard!(
#             self.events.iter_all().rev(),
#             BillingPeriodEvent::Closed
#         );
#         self.is_current = false;
#         self.events.push(BillingPeriodEvent::Closed);
#         Idempotent::Executed(())
#     }
# }
# impl TryFromEvents<BillingPeriodEvent> for BillingPeriod {
#     fn try_from_events(events: EntityEvents<BillingPeriodEvent>) -> Result<Self, EsEntityError> {
#         let mut builder = BillingPeriodBuilder::default();
#         let mut line_items = Vec::new();
#         let mut is_current = true;
#         for event in events.iter_all() {
#             match event {
#                 BillingPeriodEvent::Initialized { id, subscription_id } => {
#                     builder = builder.id(*id).subscription_id(*subscription_id);
#                 }
#                 BillingPeriodEvent::LineItemAdded { amount, description } => {
#                     line_items.push(LineItem {
#                         amount: *amount,
#                         description: description.clone(),
#                     });
#                 }
#                 BillingPeriodEvent::Closed => {
#                     is_current = false;
#                 }
#             }
#         }
#         builder
#             .is_current(is_current)
#             .line_items(line_items)
#             .events(events)
#             .build()
#     }
# }
# #[derive(Debug, Clone, Builder)]
# pub struct NewBillingPeriod {
#     pub id: BillingPeriodId,
#     pub subscription_id: SubscriptionId,
# }
# impl IntoEvents<BillingPeriodEvent> for NewBillingPeriod {
#     fn into_events(self) -> EntityEvents<BillingPeriodEvent> {
#         EntityEvents::init(
#             self.id,
#             vec![BillingPeriodEvent::Initialized {
#                 id: self.id,
#                 subscription_id: self.subscription_id,
#             }],
#         )
#     }
# }
# #[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "SubscriptionId")]
# pub enum SubscriptionEvent {
#     Initialized { id: SubscriptionId },
#     BillingPeriodStarted { period_id: BillingPeriodId },
# }
# #[derive(EsEntity, Builder)]
# #[builder(pattern = "owned", build_fn(error = "EsEntityError"))]
# pub struct Subscription {
#     pub id: SubscriptionId,
#     current_period_id: Option<BillingPeriodId>,
#     events: EntityEvents<SubscriptionEvent>,
#     #[es_entity(nested)]
#     #[builder(default)]
#     billing_periods: Nested<BillingPeriod>,
# }
# impl Subscription {
#     pub fn start_new_billing_period(&mut self) -> Idempotent<BillingPeriodId> {
#         if let Some(current_id) = self.current_period_id {
#             if let Some(current_period) = self.billing_periods.get_persisted_mut(&current_id) {
#                 current_period.close();
#             }
#         }
#         let new_period = NewBillingPeriod {
#             id: BillingPeriodId::new(),
#             subscription_id: self.id,
#         };
#         let id = new_period.id;
#         self.billing_periods.add_new(new_period);
#         self.current_period_id = Some(id);
#         self.events.push(SubscriptionEvent::BillingPeriodStarted { period_id: id });
#         Idempotent::Executed(id)
#     }
#     pub fn add_line_item_to_current_billing_period(&mut self, amount: f64, description: String) -> Idempotent<usize> {
#         if let Some(current_id) = self.current_period_id {
#             if let Some(current_period) = self.billing_periods.get_persisted_mut(&current_id) {
#                 return current_period.add_line_item(amount, description);
#             }
#         }
#         Idempotent::Ignored
#     }
#     pub fn current_billing_period(&self) -> Option<&BillingPeriod> {
#         self.current_period_id
#             .and_then(|id| self.billing_periods.get_persisted(&id))
#     }
# }
# impl TryFromEvents<SubscriptionEvent> for Subscription {
#     fn try_from_events(events: EntityEvents<SubscriptionEvent>) -> Result<Self, EsEntityError> {
#         let mut builder = SubscriptionBuilder::default();
#         let mut current_period_id = None;
#         for event in events.iter_all() {
#             match event {
#                 SubscriptionEvent::Initialized { id } => {
#                     builder = builder.id(*id);
#                 }
#                 SubscriptionEvent::BillingPeriodStarted { period_id } => {
#                     current_period_id = Some(*period_id);
#                 }
#             }
#         }
#         builder
#             .current_period_id(current_period_id)
#             .events(events)
#             .build()
#     }
# }
# #[derive(Debug, Clone, Builder)]
# pub struct NewSubscription {
#     pub id: SubscriptionId,
# }
# impl IntoEvents<SubscriptionEvent> for NewSubscription {
#     fn into_events(self) -> EntityEvents<SubscriptionEvent> {
#         EntityEvents::init(
#             self.id,
#             vec![SubscriptionEvent::Initialized { id: self.id }],
#         )
#     }
# }
# #[derive(EsRepo)]
# #[es_repo(
#     entity = "BillingPeriod",
#     columns(
#         subscription_id(ty = "SubscriptionId", update(persist = false), parent)
#     )
# )]
# pub struct BillingPeriods {
#     pool: sqlx::PgPool,
# }
# #[derive(EsRepo)]
# #[es_repo(entity = "Subscription")]
# pub struct Subscriptions {
#     pool: sqlx::PgPool,
#     #[es_repo(nested)]
#     billing_periods: BillingPeriods,
# }
# impl Subscriptions {
#     pub fn new(pool: sqlx::PgPool) -> Self {
#         Self {
#             pool: pool.clone(),
#             billing_periods: BillingPeriods { pool },
#         }
#     }
# }
# async fn init_pool() -> anyhow::Result<sqlx::PgPool> {
#     let pg_con = format!("postgres://user:password@localhost:5432/pg");
#     Ok(sqlx::PgPool::connect(&pg_con).await?)
# }
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let subscriptions = Subscriptions::new(init_pool().await?);

    // Create a new subscription
    let subscription_id = SubscriptionId::new();
    let new_subscription = NewSubscription { id: subscription_id };
    let mut subscription = subscriptions.create(new_subscription).await?;

    // Start a billing period
    subscription.start_new_billing_period();

    // Add some line items to the current period
    subscription.add_line_item_to_current_billing_period(
        100.0,
        "Monthly subscription fee".to_string()
    );
    subscription.add_line_item_to_current_billing_period(
        25.0,
        "Additional service charge".to_string()
    );

    // Persist all changes (both parent and nested entities)
    subscriptions.update(&mut subscription).await?;

    // Load the subscription - nested entities are automatically loaded
    let loaded = subscriptions.find_by_id(subscription_id).await?;

    // Access the current billing period
    if let Some(current_period) = loaded.current_billing_period() {
        println!("Current period has {} line items", current_period.line_items.len());
        for item in &current_period.line_items {
            println!("  - {}: ${}", item.description, item.amount);
        }
    }

    Ok(())
}
```

One thing to note is that  the `_in_op` functions of the parent repository now require an `AtomicOperation` argument since we must load all the entities in a consistent snapshot:
```rust,ignore
async fn find_by_id_in_op<OP>(op: OP, id: EntityId)
where
    OP: AtomicOperation;

// The version of the queries in Repositories without nested children
// cannot be used here as it would not load parent + children from a consistent snapshot.
// async fn find_by_id_in_op<'a, OP>(op: OP, id: EntityId)
// where
//     OP: IntoOneTimeExecutor<'a>;
```

## Benefits of the Nested Approach

This approach provides several key benefits:

1. **Type Safety**: The aggregate boundary is enforced at compile time
2. **Atomic Updates**: All changes to the aggregate are persisted together
3. **Automatic Loading**: When you load the parent, all nested entities are loaded automatically
4. **Encapsulation**: All access to nested entities goes through the aggregate root
5. **Consistency**: The parent entity can enforce invariants across all its children

## Performance Considerations

While nesting provides strong consistency guarantees, there are some performance implications to consider:

1. **Loading**: All nested entities are loaded when the parent is loaded. For aggregates with many children, this could impact performance.
2. **Updates**: All nested entities are checked for changes during updates, even if only one was modified.
3. **Memory**: The entire aggregate is held in memory, which could be significant for large aggregates.

For these reasons, it's important to keep aggregates small and focused on a specific consistency boundary.

## When to Use Nesting

Use the nested approach when:
- You have a true invariant that spans multiple entities
- The child entities have no meaning without the parent
- You need to enforce consistency rules across the relationship
- The number of child entities is reasonably bounded

Avoid nesting when:
- The relationship is merely associative
- Child entities can exist independently
- You expect unbounded growth in the number of children
- Performance requirements dictate more granular loading/updating

Remember, as discussed in the aggregates chapter, there are often alternative designs that can avoid the need for nesting while still maintaining consistency.

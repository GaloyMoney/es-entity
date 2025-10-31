# sim-time

The `sim-time` feature enables time simulation capabilities in es-entity, allowing you to accelerate time for testing and development purposes. This is particularly useful for testing time-dependent logic without having to wait for real time to pass.

## Enabling sim-time

Add the `sim-time` feature to your es-entity dependency:

```toml
[dependencies]
es-entity = { version = "0.7", features = ["sim-time"] }
```

## Configuration

The sim-time crate is configured through the `TimeConfig` struct. By default, sim-time operates in real-time mode. To enable simulation, you need to initialize it with a configuration:

```rust
# extern crate es_entity;
# extern crate tokio;
# use es_entity::prelude::{sim_time, chrono};
use std::time::Duration;

#[tokio::main]
async fn main() {
    let config = sim_time::TimeConfig {
        realtime: false,
        simulation: sim_time::SimulationConfig {
            // Start the simulation at a specific time
            start_at: chrono::Utc::now(),
            
            // Real milliseconds between simulation ticks
            tick_interval_ms: 10,
            
            // Simulated duration per tick
            tick_duration_secs: Duration::from_secs(86400), // 1 day per 10ms
            
            // Whether to switch to real-time when catching up to present
            transform_to_realtime: false,
        },
    };

    // Initialize sim-time with the configuration
    sim_time::init(config);
}
```

## Configuration Parameters

### TimeConfig

- `realtime: bool` - When `true`, sim-time is deactivated and all time operations use real time. When `false`, simulation is enabled.
- `simulation: Option<SimulationConfig>` - The simulation configuration. Required when `realtime` is `false`.

### SimulationConfig

- `start_at: DateTime<Utc>` - The starting time for the simulation. Defaults to the current time.
- `tick_interval_ms: u64` - The real-world milliseconds between simulation ticks.
- `tick_duration_secs: Duration` - How much simulated time passes per tick.
- `transform_to_realtime: bool` - If `true`, the simulation will automatically switch to real-time mode once it catches up to the current time.

## Usage

Once configured, sim-time provides several functions that work with simulated time:

```rust
# extern crate es_entity;
# extern crate tokio;
# use es_entity::prelude::{sim_time, chrono};
use std::time::Duration;

# #[tokio::main]
# async fn main() {
#     // Initialize sim-time with the example configuration
#     let config = sim_time::TimeConfig {
#         realtime: false,
#         simulation: sim_time::SimulationConfig {
#             start_at: chrono::Utc::now(),
#             tick_interval_ms: 10,
#             tick_duration_secs: Duration::from_secs(86400), // 1 day per 10ms
#             transform_to_realtime: false,
#         },
#     };
#     sim_time::init(config);
#
// Get the current simulated time
let current_time = sim_time::now();

// Sleep for a simulated duration
// With the example config (1 day = 10ms), this sleeps for ~0.04 real seconds
sim_time::sleep(Duration::from_secs(3600)).await; // Sleep for 1 simulated hour

// Set a timeout on an operation
# async fn async_operation() -> Result<(), std::io::Error> { Ok(()) }
sim_time::timeout(Duration::from_secs(60), async_operation()).await;

// Wait until simulation catches up to real time
// (only relevant if transform_to_realtime is true)
sim_time::wait_until_realtime().await;
# }
```

## Effect on es-entity

When sim-time is enabled, it affects how es-entity handles timestamps:

1. **Database Operations**: The `DbOp` struct automatically caches the simulated time when the feature is enabled. This cached time is used instead of database `NOW()` for all write operations.

2. **Event Timestamps**: All events created during a transaction will use the same simulated timestamp, ensuring consistency.

3. **Time-based Queries**: Operations that depend on the current time will use the simulated time instead of real time.

## Example: Testing Time-Dependent Logic

```rust
# extern crate es_entity;
# extern crate tokio;
# extern crate sqlx;
# extern crate serde;
# extern crate anyhow;
# use es_entity::prelude::*;
# use std::time::Duration;
# use es_entity::{EsEntity, EsEvent, EsRepo, TryFromEvents, IntoEvents, EsEntityError, EntityEvents};
# use chrono::Datelike;
# 
# es_entity::entity_id! { SubscriptionId }
# 
# #[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
# #[derive(EsEvent)]
# #[serde(tag = "type")]
# #[es_event(id = "SubscriptionId")]
# enum SubscriptionEvent {
#     Initialized { 
#         id: SubscriptionId,
#         expires_at: chrono::DateTime<chrono::Utc> 
#     },
# }
# 
# #[derive(Clone, EsEntity)]
# struct Subscription {
#     pub id: SubscriptionId,
#     pub expires_at: chrono::DateTime<chrono::Utc>,
#     pub events: EntityEvents<SubscriptionEvent>,
# }
# 
# impl Subscription {
#     pub fn is_expired(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
#         self.expires_at <= now
#     }
#
#     pub fn created_at(&self) -> chrono::DateTime<chrono::Utc> {
#         // Get the timestamp from when this entity was first persisted
#         self.events.entity_first_persisted_at().unwrap_or_else(|| sim_time::now())
#     }
# }
# 
# impl TryFromEvents<SubscriptionEvent> for Subscription {
#     fn try_from_events(events: EntityEvents<SubscriptionEvent>) -> Result<Self, es_entity::EsEntityError> {
#         let mut expires_at = chrono::Utc::now();
#         for event in events.iter_all() {
#             match event {
#                 SubscriptionEvent::Initialized { expires_at: exp, .. } => {
#                     expires_at = *exp;
#                 }
#             }
#         }
#         Ok(Self { id: events.id().clone(), expires_at, events })
#     }
# }
# 
# #[derive(Debug)]
# struct NewSubscription {
#     id: SubscriptionId,
#     duration_days: i64,
# }
# 
# impl IntoEvents<SubscriptionEvent> for NewSubscription {
#     fn into_events(self) -> EntityEvents<SubscriptionEvent> {
#         EntityEvents::init(
#             self.id,
#             [SubscriptionEvent::Initialized {
#                 id: self.id,
#                 expires_at: sim_time::now() + chrono::Duration::days(self.duration_days),
#             }])
#     }
# }
# 
# #[derive(Clone, EsRepo)]
# #[es_repo(entity = "Subscription", event = "SubscriptionEvent")]
# struct SubscriptionRepo {
#     pool: sqlx::PgPool,
# }
# 
#[tokio::main]
async fn main() -> anyhow::Result<()> {
#     // Setup database connection
#     let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| "postgres://localhost/test".to_string());
#     let pool = sqlx::PgPool::connect(&db_url).await.unwrap();
#     let repo = SubscriptionRepo { pool };
#     
    // Start simulation at a fixed date in the past (middle of month to avoid boundary issue for the month/year, ie if test is run last day of the month/year)
    let start_time = chrono::DateTime::parse_from_rfc3339("2023-06-15T12:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    
    // Configure time to run 30 days per second
    let config = sim_time::TimeConfig {
        realtime: false,
        simulation: sim_time::SimulationConfig {
            start_at: start_time,
            tick_interval_ms: 33,  // ~30 ticks per second
            tick_duration_secs: Duration::from_secs(86400), // 1 day per tick
            transform_to_realtime: false,
        },
    };

    sim_time::init(config);

    // Create a subscription that expires in 30 days
    let subscription = repo.create(NewSubscription {
        id: SubscriptionId::new(),
        duration_days: 30,
    }).await?;

    // Verify that sim-time is working
    let created_at = subscription.created_at();
    
    // Verify sim-time is working by checking the entity was created in the simulated year/month
    assert_eq!(created_at.year(), start_time.year(), "Entity should be created in the simulated year");
    assert_eq!(created_at.month(), start_time.month(), "Entity should be created in the simulated month");
    
    // Verify that we're actually in the past (compared to real time)
    let real_now = chrono::Utc::now();
    assert!(created_at < real_now - chrono::Duration::days(300), "Entity creation time should be in the past");
    
    // The subscription should NOT be expired yet (30 days haven't passed in sim time)
    assert!(!subscription.is_expired(sim_time::now()));

    // Sleep for 30 simulated days (which takes ~1 real second with this config)
    sim_time::sleep(Duration::from_secs(30 * 86400)).await;

    // Check that the subscription is now expired
    let subscription = repo.find_by_id(subscription.id).await?;
    assert!(subscription.is_expired(sim_time::now()));

    Ok(())
}
```

## Best Practices

1. **Initialize Early**: Call `sim_time::init()` before any other es-entity operations to ensure consistent time handling.

2. **Use in Tests**: The sim-time feature is primarily designed for testing. Consider using conditional compilation to only enable it in test builds:
   ```toml
   [dev-dependencies]
   es-entity = { version = "0.7", features = ["sim-time"] }
   ```

3. **Consistent Time**: All operations within a single database transaction will use the same timestamp, ensuring consistency in your event store.

4. **Real-time Transformation**: Use `transform_to_realtime: true` when you want to start a simulation in the past and have it automatically switch to real-time when it catches up. This is useful for replaying historical scenarios.

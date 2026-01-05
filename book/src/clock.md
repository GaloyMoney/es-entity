# Clock Module

The `es_entity::clock` module provides a time abstraction that works identically whether using real time or artificial time for testing. This enables deterministic testing of time-dependent logic without waiting for real time to pass.

## Overview

The clock module provides:

- **`ClockHandle`** - A cheap-to-clone handle for time operations
- **`Clock`** - Global clock access (like `Utc::now()` but testable)
- **`ClockController`** - Control artificial time advancement
- **`SimulationConfig`** - Configure artificial clock behavior

## Clock Types

### Realtime Clock

Uses the system clock and tokio timers. This is the default behavior.

```rust,ignore
use es_entity::clock::ClockHandle;

let clock = ClockHandle::realtime();
let now = clock.now(); // Returns Utc::now()
```

### Artificial Clock

Time only advances when explicitly controlled. Perfect for deterministic testing.

```rust,ignore
use es_entity::clock::{ClockHandle, SimulationConfig};

// Create artificial clock with manual advancement
let (clock, ctrl) = ClockHandle::artificial(SimulationConfig::manual());

let t0 = clock.now();

// Time doesn't advance on its own
assert_eq!(clock.now(), t0);

// Advance time by 1 hour
ctrl.advance(Duration::from_secs(3600)).await;

assert_eq!(clock.now(), t0 + chrono::Duration::hours(1));
```

## Global Clock API

The `Clock` struct provides static methods for global clock access, similar to `Utc::now()`:

```rust,ignore
use es_entity::clock::{Clock, SimulationConfig};

// For testing: install artificial clock (returns controller)
let ctrl = Clock::install_artificial(SimulationConfig::manual());

// Get current time (works with both artificial and real time)
let now = Clock::now();

// Check if artificial clock is installed
if Clock::is_artificial() {
    // We're in test mode with controlled time
}

// Sleep and timeout also use the global clock
Clock::sleep(Duration::from_secs(60)).await;
Clock::timeout(Duration::from_secs(5), some_future).await;
```

### Lazy Initialization

If you call `Clock::now()` without installing an artificial clock, it lazily initializes to realtime mode. This means production code can use `Clock::now()` without any setup.

## SimulationConfig

Configure how the artificial clock behaves:

```rust,ignore
use es_entity::clock::{SimulationConfig, SimulationMode};
use chrono::Utc;

// Manual mode - time only advances via controller.advance()
let config = SimulationConfig::manual();

// Auto mode - time advances automatically at 1000x speed
let config = SimulationConfig::auto(1000.0);

// Start at a specific time
let config = SimulationConfig {
    start_at: Utc::now() - chrono::Duration::days(30),
    mode: SimulationMode::Manual,
};
```

## ClockController

The controller is returned when creating an artificial clock and provides:

```rust,ignore
// Advance time by duration (wakes sleeping tasks in order)
ctrl.advance(Duration::from_secs(3600)).await;

// Advance to next pending wake event
let wake_time = ctrl.advance_to_next_wake().await;

// Set time directly (doesn't process intermediate wakes)
ctrl.set_time(some_datetime);

// Get current time
let now = ctrl.now();

// Check pending sleep count
let count = ctrl.pending_wake_count();

// Transition to realtime mode
ctrl.transition_to_realtime();
```

## Integration with DbOp

When a global artificial clock is installed, database operations automatically use it:

```rust,ignore
use es_entity::clock::{Clock, SimulationConfig};

// Install artificial clock for testing
let ctrl = Clock::install_artificial(SimulationConfig::manual());

// DbOp::init() now caches the artificial time
let op = DbOp::init(&pool).await?;

// with_system_time() uses Clock::now()
let op_with_time = op.with_system_time();

// with_db_time() uses artificial time instead of SELECT NOW()
let op_with_time = op.with_db_time().await?;
```

This ensures all operations within a transaction use consistent, controlled time.

## Example: Testing Time-Dependent Logic

```rust,ignore
use es_entity::clock::{Clock, SimulationConfig};
use std::time::Duration;

#[tokio::test]
async fn test_subscription_expiry() {
    // Install artificial clock starting 30 days ago
    let start = Utc::now() - chrono::Duration::days(30);
    let ctrl = Clock::install_artificial(SimulationConfig {
        start_at: start,
        mode: SimulationMode::Manual,
    });

    // Create subscription that expires in 7 days
    let subscription = create_subscription_expiring_in(7).await;

    // Not expired yet
    assert!(!subscription.is_expired(Clock::now()));

    // Advance 8 days
    ctrl.advance(Duration::from_secs(8 * 86400)).await;

    // Now expired
    assert!(subscription.is_expired(Clock::now()));
}
```

## Best Practices

1. **Use `Clock::now()` instead of `Utc::now()`** - This makes your code testable with artificial time.

2. **Install artificial clock early in tests** - Call `Clock::install_artificial()` before any code that uses time.

3. **Use manual mode for deterministic tests** - Auto mode is useful for simulations but manual mode gives you full control.

4. **Advance time explicitly** - In tests, use `ctrl.advance()` to move time forward in a controlled way.

5. **Check `is_artificial()` sparingly** - Most code shouldn't need to know if time is artificial; it should just use `Clock::now()`.

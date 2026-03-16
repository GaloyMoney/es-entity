# Clock Module

The `es_entity::clock` module provides a time abstraction that works identically whether using real time or manual time for testing. This enables deterministic testing of time-dependent logic without waiting for real time to pass.

## Overview

The clock module provides:

- **`ClockHandle`** - A cheap-to-clone handle for time operations
- **`Clock`** - Global clock access (like `Utc::now()` but testable)
- **`ClockController`** - Control manual time advancement

## Clock Types

### Realtime Clock

Uses the system clock and tokio timers. This is the default behavior.

```rust,ignore
use es_entity::clock::ClockHandle;

let clock = ClockHandle::realtime();
let now = clock.now(); // Returns Utc::now()
```

### Manual Clock

Time only advances when explicitly controlled. Perfect for deterministic testing.

```rust,ignore
use es_entity::clock::ClockHandle;

// Create manual clock
let (clock, ctrl) = ClockHandle::manual();

let t0 = clock.now();

// Time doesn't advance on its own
assert_eq!(clock.now(), t0);

// Advance time by 1 hour
ctrl.advance(Duration::from_secs(3600)).await;

assert_eq!(clock.now(), t0 + chrono::Duration::hours(1));
```

You can also start at a specific time:

```rust,ignore
use es_entity::clock::ClockHandle;
use chrono::Utc;

let (clock, ctrl) = ClockHandle::manual_at(Utc::now() - chrono::Duration::days(30));
```

## Global Clock API

The `Clock` struct provides static methods for global clock access, similar to `Utc::now()`:

```rust,ignore
use es_entity::clock::Clock;

// For testing: install manual clock (returns controller)
let ctrl = Clock::install_manual();

// Get current time (works with both manual and real time)
let now = Clock::now();

// Check if manual clock is installed
if Clock::is_manual() {
    // We're in test mode with controlled time
}

// Sleep and timeout also use the global clock
Clock::sleep(Duration::from_secs(60)).await;
Clock::timeout(Duration::from_secs(5), some_future).await;
```

### Lazy Initialization

If you call `Clock::now()` without installing a manual clock, it lazily initializes to realtime mode. This means production code can use `Clock::now()` without any setup.

## ClockController

The controller is returned when creating a manual clock and provides:

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
```

## Integration with DbOp

When a global manual clock is installed, database operations automatically use it:

```rust,ignore
use es_entity::clock::Clock;

// Install manual clock for testing
let ctrl = Clock::install_manual();

// DbOp::init() now caches the manual time
let op = DbOp::init(&pool).await?;

// with_clock_time() uses the operation's clock
let op_with_time = op.with_clock_time();

// with_db_time() uses manual time instead of SELECT NOW()
let op_with_time = op.with_db_time().await?;
```

This ensures all operations within a transaction use consistent, controlled time.

### Explicit Clock Injection

For more control, you can inject a specific clock into database operations without modifying global state. This is useful when you want isolated clocks per test or need different clocks for different operations:

```rust,ignore
use es_entity::clock::ClockHandle;

// Create a manual clock (not installed globally)
let (clock, ctrl) = ClockHandle::manual();

// Pass the clock explicitly to DbOp
let op = DbOp::init_with_clock(&pool, &clock).await?;

// The operation uses this clock for time operations
let op_with_time = op.with_clock_time();
```

Repositories generated with `#[derive(EsRepo)]` also support this pattern:

```rust,ignore
// Using the repo's begin_op_with_clock method
let mut op = users.begin_op_with_clock(&clock).await?;

// Create entity - recorded_at will use the manual clock's time
let user = users.create_in_op(&mut op, new_user).await?;
op.commit().await?;
```

This approach avoids global state and allows each test to have its own independent clock, preventing test interference.

### Clock Field in Repository

For an even cleaner API, you can add a `clock` field to your repository struct. The macro supports two patterns:

#### Optional Clock Field

Use `Option<ClockHandle>` when you want the same repo type to work both with and without a custom clock:

```rust,ignore
use es_entity::{clock::ClockHandle, EsRepo};
use sqlx::PgPool;

#[derive(EsRepo)]
#[es_repo(entity = "User")]
pub struct Users {
    pool: PgPool,
    clock: Option<ClockHandle>,  // Optional: use if Some, fallback to global
}

impl Users {
    // Production: no clock, uses global
    pub fn new(pool: PgPool) -> Self {
        Self { pool, clock: None }
    }

    // Testing: with manual clock
    pub fn with_clock(pool: PgPool, clock: ClockHandle) -> Self {
        Self { pool, clock: Some(clock) }
    }
}
```

Usage:

```rust,ignore
// Production code - uses global clock
let users = Users::new(pool);
let user = users.create(new_user).await?;

// Test code - uses manual clock
let (clock, ctrl) = ClockHandle::manual();
let users = Users::with_clock(pool, clock);
let user = users.create(new_user).await?;  // Uses manual clock!
```

#### Required Clock Field

Use `ClockHandle` (non-optional) when you always want to inject a clock:

```rust,ignore
#[derive(EsRepo)]
#[es_repo(entity = "User")]
pub struct Users {
    pool: PgPool,
    clock: ClockHandle,  // Required: always use this clock
}

impl Users {
    pub fn new(pool: PgPool, clock: ClockHandle) -> Self {
        Self { pool, clock }
    }
}
```

This is useful when you want to enforce clock injection at construction time, making the dependency explicit.

#### Field Detection

The macro detects a field named `clock` (or marked with `#[es_repo(clock)]`) and generates the appropriate `begin_op()` implementation:

- `Option<ClockHandle>`: Uses the clock if `Some`, falls back to global clock if `None`
- `ClockHandle`: Always uses the injected clock
- No clock field: Always uses the global clock

## Example: Testing Time-Dependent Logic

```rust,ignore
use es_entity::clock::Clock;
use std::time::Duration;

#[tokio::test]
async fn test_subscription_expiry() {
    // Install manual clock starting 30 days ago
    let start = Utc::now() - chrono::Duration::days(30);
    let ctrl = Clock::install_manual_at(start);

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

1. **Use `Clock::now()` instead of `Utc::now()`** - This makes your code testable with manual time.

2. **Install manual clock early in tests** - Call `Clock::install_manual()` before any code that uses time.

3. **Advance time explicitly** - In tests, use `ctrl.advance()` to move time forward in a controlled way.

4. **Check `is_manual()` sparingly** - Most code shouldn't need to know if time is manual; it should just use `Clock::now()`.

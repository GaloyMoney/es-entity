use es_entity_time::{ClockHandle, SimulationConfig, SimulationMode};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

#[tokio::test]
async fn test_realtime_now() {
    let clock = ClockHandle::realtime();
    let before = chrono::Utc::now();
    let clock_now = clock.now();
    let after = chrono::Utc::now();

    assert!(clock_now >= before);
    assert!(clock_now <= after);
}

#[tokio::test]
async fn test_realtime_sleep() {
    let clock = ClockHandle::realtime();
    let start = std::time::Instant::now();
    clock.sleep(Duration::from_millis(50)).await;
    let elapsed = start.elapsed();

    assert!(elapsed >= Duration::from_millis(40));
    assert!(elapsed < Duration::from_millis(150));
}

#[tokio::test]
async fn test_simulated_manual_time_stands_still() {
    let clock = ClockHandle::simulated(SimulationConfig::manual());
    let t0 = clock.now();

    // Time doesn't advance on its own
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(clock.now(), t0);
}

#[tokio::test]
async fn test_simulated_manual_advance() {
    let clock = ClockHandle::simulated(SimulationConfig::manual());
    let t0 = clock.now();

    clock.advance(Duration::from_secs(3600)).await;

    assert_eq!(clock.now(), t0 + chrono::Duration::hours(1));
}

#[tokio::test]
async fn test_simulated_auto_advance() {
    // 10000x speed: 1ms real = 10 seconds simulated
    let clock = ClockHandle::simulated(SimulationConfig::auto(10000.0));

    let start = clock.now();
    tokio::time::sleep(Duration::from_millis(10)).await;
    let end = clock.now();

    let elapsed = end - start;
    // Should have advanced ~100 seconds (10ms * 10000x)
    assert!(elapsed.num_seconds() >= 50);
    assert!(elapsed.num_seconds() <= 200);
}

#[tokio::test]
async fn test_manual_sleep_wakes_on_advance() {
    let clock = ClockHandle::simulated(SimulationConfig::manual());
    let t0 = clock.now();

    let woke = Arc::new(AtomicUsize::new(0));
    let woke_clone = woke.clone();
    let clock_clone = clock.clone();

    let handle = tokio::spawn(async move {
        clock_clone.sleep(Duration::from_secs(60)).await;
        woke_clone.fetch_add(1, Ordering::SeqCst);
        clock_clone.now()
    });

    // Let task register its sleep
    tokio::task::yield_now().await;
    assert_eq!(clock.pending_wake_count(), 1);
    assert_eq!(woke.load(Ordering::SeqCst), 0);

    // Advance past sleep time
    clock.advance(Duration::from_secs(120)).await;

    let wake_time = handle.await.unwrap();
    assert_eq!(woke.load(Ordering::SeqCst), 1);
    // Task woke at exactly 60 seconds, not 120
    assert_eq!(wake_time, t0 + chrono::Duration::seconds(60));
}

#[tokio::test]
async fn test_multiple_sleeps_wake_in_order() {
    let clock = ClockHandle::simulated(SimulationConfig::manual());
    let t0 = clock.now();

    let wake_order = Arc::new(parking_lot::Mutex::new(Vec::new()));

    // Task A: 30 seconds
    let wo = wake_order.clone();
    let c = clock.clone();
    let handle_a = tokio::spawn(async move {
        c.sleep(Duration::from_secs(30)).await;
        wo.lock().push(('A', c.now()));
    });

    // Task B: 10 seconds
    let wo = wake_order.clone();
    let c = clock.clone();
    let handle_b = tokio::spawn(async move {
        c.sleep(Duration::from_secs(10)).await;
        wo.lock().push(('B', c.now()));
    });

    // Task C: 20 seconds
    let wo = wake_order.clone();
    let c = clock.clone();
    let handle_c = tokio::spawn(async move {
        c.sleep(Duration::from_secs(20)).await;
        wo.lock().push(('C', c.now()));
    });

    // Let tasks register
    tokio::task::yield_now().await;
    assert_eq!(clock.pending_wake_count(), 3);

    // Advance 1 minute - all should wake in order
    clock.advance(Duration::from_secs(60)).await;

    let _ = tokio::join!(handle_a, handle_b, handle_c);

    let order = wake_order.lock();
    assert_eq!(order.len(), 3);

    // Woke in chronological order
    assert_eq!(order[0].0, 'B'); // 10s
    assert_eq!(order[1].0, 'C'); // 20s
    assert_eq!(order[2].0, 'A'); // 30s

    // Each saw correct time
    assert_eq!(order[0].1, t0 + chrono::Duration::seconds(10));
    assert_eq!(order[1].1, t0 + chrono::Duration::seconds(20));
    assert_eq!(order[2].1, t0 + chrono::Duration::seconds(30));
}

#[tokio::test]
async fn test_advance_to_next_wake() {
    let clock = ClockHandle::simulated(SimulationConfig::manual());
    let t0 = clock.now();

    let c = clock.clone();
    let handle = tokio::spawn(async move {
        c.sleep(Duration::from_secs(100)).await;
    });

    tokio::task::yield_now().await;

    // Advance to next wake
    let wake_time = clock.advance_to_next_wake().await;
    assert_eq!(wake_time, Some(t0 + chrono::Duration::seconds(100)));
    assert_eq!(clock.now(), t0 + chrono::Duration::seconds(100));

    // No more pending wakes
    let next = clock.advance_to_next_wake().await;
    assert_eq!(next, None);

    let _ = handle.await;
}

#[tokio::test]
async fn test_timeout_success() {
    let clock = ClockHandle::simulated(SimulationConfig::manual());

    let c = clock.clone();
    let result =
        tokio::spawn(async move { c.timeout(Duration::from_secs(10), async { 42 }).await });

    tokio::task::yield_now().await;
    clock.advance(Duration::from_secs(1)).await;

    let value = result.await.unwrap();
    assert_eq!(value, Ok(42));
}

#[tokio::test]
async fn test_timeout_elapsed() {
    let clock = ClockHandle::simulated(SimulationConfig::manual());

    let c = clock.clone();
    let result_handle = tokio::spawn(async move {
        c.timeout(Duration::from_secs(5), async {
            // This will never complete on its own
            std::future::pending::<()>().await
        })
        .await
    });

    tokio::task::yield_now().await;

    // Advance past timeout
    clock.advance(Duration::from_secs(10)).await;

    let result = result_handle.await.unwrap();
    assert!(result.is_err());
}

#[tokio::test]
async fn test_cloned_handles_share_time() {
    let clock1 = ClockHandle::simulated(SimulationConfig::manual());
    let clock2 = clock1.clone();
    let clock3 = clock1.clone();

    let t0 = clock1.now();

    // Advance via one handle
    clock2.advance(Duration::from_secs(100)).await;

    // All see the same time
    assert_eq!(clock1.now(), t0 + chrono::Duration::seconds(100));
    assert_eq!(clock2.now(), t0 + chrono::Duration::seconds(100));
    assert_eq!(clock3.now(), t0 + chrono::Duration::seconds(100));
}

#[tokio::test]
async fn test_set_time_jumps_directly() {
    let clock = ClockHandle::simulated(SimulationConfig::manual());
    let t0 = clock.now();

    let target = t0 + chrono::Duration::days(30);
    clock.set_time(target);

    assert_eq!(clock.now(), target);
}

#[tokio::test]
async fn test_cancelled_sleep_cleanup() {
    let clock = ClockHandle::simulated(SimulationConfig::manual());

    // Start a sleep
    let c = clock.clone();
    let handle = tokio::spawn(async move {
        c.sleep(Duration::from_secs(100)).await;
    });

    tokio::task::yield_now().await;
    assert_eq!(clock.pending_wake_count(), 1);

    // Cancel the task
    handle.abort();
    let _ = handle.await;

    // Wake should be cleaned up
    // Note: might need a yield for cleanup to propagate
    tokio::task::yield_now().await;
    assert_eq!(clock.pending_wake_count(), 0);
}

#[tokio::test]
async fn test_concurrent_system_coordination() {
    // Simulate multiple systems that need coordinated time
    let clock = ClockHandle::simulated(SimulationConfig::manual());
    let t0 = clock.now();

    // System A: Job scheduler - runs job every hour
    let job_runs = Arc::new(AtomicUsize::new(0));
    let jr = job_runs.clone();
    let c = clock.clone();
    let _job_system = tokio::spawn(async move {
        loop {
            c.sleep(Duration::from_secs(3600)).await;
            jr.fetch_add(1, Ordering::SeqCst);
        }
    });

    // System B: Cache with 30-minute TTL
    let cache_refreshes = Arc::new(AtomicUsize::new(0));
    let cr = cache_refreshes.clone();
    let c = clock.clone();
    let _cache_system = tokio::spawn(async move {
        loop {
            c.sleep(Duration::from_secs(1800)).await;
            cr.fetch_add(1, Ordering::SeqCst);
        }
    });

    tokio::task::yield_now().await;

    // Advance 2 hours
    clock.advance(Duration::from_secs(7200)).await;

    // Job should have run 2 times (at 1h and 2h)
    assert_eq!(job_runs.load(Ordering::SeqCst), 2);

    // Cache should have refreshed 4 times (at 30m, 1h, 1h30, 2h)
    assert_eq!(cache_refreshes.load(Ordering::SeqCst), 4);

    // Time is exactly at 2 hours
    assert_eq!(clock.now(), t0 + chrono::Duration::hours(2));
}

#[tokio::test]
async fn test_same_time_wakes() {
    let clock = ClockHandle::simulated(SimulationConfig::manual());
    let t0 = clock.now();

    let wake_count = Arc::new(AtomicUsize::new(0));

    // Multiple tasks all sleeping for exactly 60 seconds
    for _ in 0..5 {
        let wc = wake_count.clone();
        let c = clock.clone();
        tokio::spawn(async move {
            c.sleep(Duration::from_secs(60)).await;
            wc.fetch_add(1, Ordering::SeqCst);
        });
    }

    tokio::task::yield_now().await;
    assert_eq!(clock.pending_wake_count(), 5);

    // Advance to wake time
    clock.advance(Duration::from_secs(60)).await;

    // All should have woken
    assert_eq!(wake_count.load(Ordering::SeqCst), 5);
    assert_eq!(clock.now(), t0 + chrono::Duration::seconds(60));
}

#[tokio::test]
async fn test_debug_output() {
    let clock = ClockHandle::realtime();
    let debug = format!("{:?}", clock);
    assert!(debug.contains("Realtime"));

    let clock = ClockHandle::simulated(SimulationConfig::manual());
    let debug = format!("{:?}", clock);
    assert!(debug.contains("Simulated"));
}

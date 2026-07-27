//! Micro-benchmarks for the event-context machinery.
//!
//! Structured after cala's `cala-perf` criterion setup. These benches only use
//! public API that exists both before and after PR #163 (`im::HashMap` vs
//! copy-on-write `Arc<Vec>` inside `ContextData`), so the identical bench
//! commit can be cherry-picked onto either implementation for an A/B run.
//!
//! # Running
//!
//! ```text
//! cargo bench --bench context                              # core paths
//! cargo bench --bench context --features tracing-context   # + per-persist tracing insert
//! ```
//!
//! # A/B protocol against the `im`-based implementation
//!
//! ```text
//! git switch perf/event-context-benches-im    # parent of PR #163 + this bench commit
//! cargo bench --bench context -- --save-baseline im
//! git switch perf/event-context-benches       # PR #163 + this bench commit
//! cargo bench --bench context -- --baseline im
//! ```
//!
//! # What each group isolates
//!
//! 1. `per_poll_overhead` — the PR's headline claim. A future that returns
//!    `Pending` N times is driven to completion by a raw no-op-waker loop,
//!    bare vs wrapped in `with_event_context`. (wrapped - bare) / (N + 1)
//!    is the per-poll cost of the wrapper (seed + inner poll + write-back +
//!    drop).
//! 2. `tokio_yield` — same comparison on a real tokio runtime, so the wrapper
//!    cost can be read as a fraction of realistic scheduler overhead.
//! 3. `context_data_clone` — the clone that `EventContextFuture::poll` pays
//!    on every poll (`im` map clone vs `Arc` refcount bump).
//! 4. `context_lifecycle` — `seed`+`drop` (TLS push/pop + `Rc` alloc, a cost
//!    the PR does NOT remove) and `seed`+`insert`+`drop` (the copy-on-write /
//!    HAMT path-copy mutation cost).
//! 5. `persist_path` — `EntityEvents::push` on an `event_context` event
//!    (the real `data_for_storing()` call, one per persisted event; with
//!    `tracing-context` enabled this includes the `"tracing"` insert) and
//!    `serde_json::to_value` (the sqlx `Encode` serialization shape).

use criterion::{BatchSize, BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use serde::{Deserialize, Serialize};

use std::{
    future::Future,
    hint::black_box,
    pin::{Pin, pin},
    task::{Context, Poll, Waker},
    time::Duration,
};

use es_entity::{ContextData, EntityEvents, EventContext, WithEventContext, *};

es_entity::entity_id! { BenchEntityId }

#[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "BenchEntityId", event_context)]
enum BenchEvent {
    Happened { value: u64 },
}

const KEYS: [&str; 8] = [
    "key_0", "key_1", "key_2", "key_3", "key_4", "key_5", "key_6", "key_7",
];

/// Realistic context payloads: what `#[es_event_context]` inserts at request
/// setup (ids / small strings).
fn context_data(entries: usize) -> ContextData {
    let mut ctx = EventContext::fork();
    for key in KEYS.iter().take(entries) {
        ctx.insert(key, &"01996a2b-3c4d-5e6f-7a8b-9c0d1e2f3a4b")
            .unwrap();
    }
    ctx.data()
}

/// Number of `Poll::Pending` returns before completion; a request that
/// suspends 64 times pays the wrapper 65 polls.
const POLLS: u32 = 64;

struct YieldN(u32);

impl Future for YieldN {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if self.0 == 0 {
            Poll::Ready(())
        } else {
            self.0 -= 1;
            Poll::Pending
        }
    }
}

/// Drive a future to completion with a no-op waker — isolates poll cost from
/// any scheduler.
fn drive<F: Future>(fut: F) -> F::Output {
    let mut fut = pin!(fut);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(out) = fut.as_mut().poll(&mut cx) {
            return out;
        }
    }
}

fn per_poll_overhead(c: &mut Criterion) {
    let mut g = c.benchmark_group("1. per_poll_overhead");
    g.throughput(Throughput::Elements(POLLS as u64 + 1));

    g.bench_function("bare_future", |b| {
        b.iter(|| drive(black_box(YieldN(POLLS))))
    });

    for entries in [0usize, 3] {
        let data = context_data(entries);
        g.bench_function(
            BenchmarkId::new("with_event_context", format!("{entries}_entries")),
            |b| b.iter(|| drive(black_box(YieldN(POLLS)).with_event_context(data.clone()))),
        );
    }
    g.finish();
}

async fn yield_many() {
    for _ in 0..POLLS {
        tokio::task::yield_now().await;
    }
}

fn tokio_yield(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();

    let mut g = c.benchmark_group("2. tokio_yield");
    g.throughput(Throughput::Elements(POLLS as u64 + 1));

    g.bench_function("bare_future", |b| b.to_async(&rt).iter(yield_many));

    let data = context_data(3);
    g.bench_function("with_event_context/3_entries", |b| {
        b.to_async(&rt)
            .iter(|| yield_many().with_event_context(data.clone()))
    });
    g.finish();
}

fn context_data_clone(c: &mut Criterion) {
    let mut g = c.benchmark_group("3. context_data_clone");
    for entries in [0usize, 3, 8] {
        let data = context_data(entries);
        g.bench_function(
            BenchmarkId::from_parameter(format!("{entries}_entries")),
            |b| b.iter(|| black_box(data.clone())),
        );
    }
    g.finish();
}

fn context_lifecycle(c: &mut Criterion) {
    let mut g = c.benchmark_group("4. context_lifecycle");

    // TLS push + Rc alloc + reverse-scan removal — per-poll cost the PR keeps.
    let data = context_data(3);
    g.bench_function("seed_drop/3_entries", |b| {
        b.iter(|| {
            let ctx = EventContext::seed(black_box(data.clone()));
            drop(ctx);
        })
    });

    // Adds one insert into shared data: Arc::make_mut copy-on-write on the PR
    // branch, HAMT path-copy (chunk clone) on the im branch.
    g.bench_function("seed_insert_drop/3_entries", |b| {
        b.iter(|| {
            let mut ctx = EventContext::seed(black_box(data.clone()));
            ctx.insert("inserted_key", &"inserted-value").unwrap();
            drop(ctx);
        })
    });
    g.finish();
}

fn persist_path(c: &mut Criterion) {
    let mut g = c.benchmark_group("5. persist_path");

    // Keep an ambient context (as a request handler would) alive across the
    // whole group so `data_for_storing()` clones realistic data.
    let mut ambient = EventContext::current();
    for key in KEYS.iter().take(3) {
        ambient
            .insert(key, &"01996a2b-3c4d-5e6f-7a8b-9c0d1e2f3a4b")
            .unwrap();
    }

    // One `data_for_storing()` per pushed event — ~10M calls in the stress run.
    g.bench_function("entity_events_push/3_ambient_entries", |b| {
        b.iter_batched(
            || EntityEvents::<BenchEvent>::init(BenchEntityId::new(), std::iter::empty()),
            |mut events| {
                events.push(BenchEvent::Happened { value: 42 });
                events
            },
            BatchSize::SmallInput,
        )
    });

    // The serialization the sqlx `Encode` path performs per stored context.
    let data = context_data(3);
    g.bench_function("serde_to_value/3_entries", |b| {
        b.iter(|| serde_json::to_value(black_box(&data)).unwrap())
    });
    g.finish();

    drop(ambient);
}

criterion_group!(
    name = benches;
    config = Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(2));
    targets = per_poll_overhead, tokio_yield, context_data_clone, context_lifecycle, persist_path
);
criterion_main!(benches);

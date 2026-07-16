//! Property-based tests for the `idempotency_guard!` contract.
//!
//! The contract under test: for any sequence of commands whose guards use only
//! `already_applied` (no `resets_on`), applying the same sequence twice produces
//! the same final event stream as applying it once. This is the retry-safety
//! property that callers rely on.
//!
//! Also covers `resets_on` semantics: a "reset" event re-enables the guarded
//! command, so apply/reset/apply produces three events, not two.

use es_entity::*;
use proptest::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------- toy entity ----------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ToyEvent {
    Initialized { id: Uuid },
    Renamed { name: String },
    Tagged { tag: String },
    Activated,
    Deactivated,
}

impl EsEvent for ToyEvent {
    type EntityId = Uuid;
    fn event_context() -> bool {
        false
    }
    fn event_type(&self) -> &'static str {
        match self {
            ToyEvent::Initialized { .. } => "initialized",
            ToyEvent::Renamed { .. } => "renamed",
            ToyEvent::Tagged { .. } => "tagged",
            ToyEvent::Activated => "activated",
            ToyEvent::Deactivated => "deactivated",
        }
    }
}

struct Toy {
    events: EntityEvents<ToyEvent>,
}

impl Toy {
    fn new(id: Uuid) -> Self {
        Self {
            events: EntityEvents::init(id, [ToyEvent::Initialized { id }]),
        }
    }

    /// `already_applied` only — once renamed to N, repeat is a no-op forever.
    fn set_name(&mut self, name: String) -> Idempotent<()> {
        idempotency_guard!(
            self.events.iter_all(),
            already_applied: ToyEvent::Renamed { name: existing } if existing == &name
        );
        self.events.push(ToyEvent::Renamed { name });
        Idempotent::Executed(())
    }

    /// `already_applied` only — additive, set semantics.
    fn add_tag(&mut self, tag: String) -> Idempotent<()> {
        idempotency_guard!(
            self.events.iter_all(),
            already_applied: ToyEvent::Tagged { tag: existing } if existing == &tag
        );
        self.events.push(ToyEvent::Tagged { tag });
        Idempotent::Executed(())
    }

    /// `already_applied + resets_on` — Activate is no-op until a Deactivate
    /// resets the scan window.
    fn activate(&mut self) -> Idempotent<()> {
        idempotency_guard!(
            self.events.iter_all().rev(),
            already_applied: ToyEvent::Activated,
            resets_on: ToyEvent::Deactivated
        );
        self.events.push(ToyEvent::Activated);
        Idempotent::Executed(())
    }

    fn deactivate(&mut self) -> Idempotent<()> {
        idempotency_guard!(
            self.events.iter_all().rev(),
            already_applied: ToyEvent::Deactivated,
            resets_on: ToyEvent::Activated
        );
        self.events.push(ToyEvent::Deactivated);
        Idempotent::Executed(())
    }

    fn event_stream(&self) -> Vec<ToyEvent> {
        self.events.iter_all().cloned().collect()
    }
}

// ---------- command DSL ----------

#[derive(Debug, Clone)]
enum Cmd {
    SetName(String),
    AddTag(String),
    Activate,
    Deactivate,
}

impl Cmd {
    fn apply(&self, toy: &mut Toy) -> bool {
        let r = match self {
            Cmd::SetName(n) => toy.set_name(n.clone()),
            Cmd::AddTag(t) => toy.add_tag(t.clone()),
            Cmd::Activate => toy.activate(),
            Cmd::Deactivate => toy.deactivate(),
        };
        r.did_execute()
    }
}

// Small string pool keeps the search space narrow enough that collisions
// (and therefore guard hits) actually happen.
fn arb_name() -> impl Strategy<Value = String> {
    prop_oneof![Just("a".into()), Just("b".into()), Just("c".into())]
}
fn arb_tag() -> impl Strategy<Value = String> {
    prop_oneof![Just("x".into()), Just("y".into()), Just("z".into())]
}

/// Commands restricted to `already_applied`-only guards. These are full-retry safe.
fn arb_pure_cmd() -> impl Strategy<Value = Cmd> {
    prop_oneof![
        arb_name().prop_map(Cmd::SetName),
        arb_tag().prop_map(Cmd::AddTag),
    ]
}

/// All commands, including the `resets_on` ones.
fn arb_any_cmd() -> impl Strategy<Value = Cmd> {
    prop_oneof![
        arb_name().prop_map(Cmd::SetName),
        arb_tag().prop_map(Cmd::AddTag),
        Just(Cmd::Activate),
        Just(Cmd::Deactivate),
    ]
}

// ---------- properties ----------

proptest! {
    #![proptest_config(ProptestConfig { cases: 4096, ..ProptestConfig::default() })]

    /// Stutter: applying any single command twice in a row equals applying it once.
    /// Holds for both `already_applied`-only and `resets_on` guards.
    #[test]
    fn stutter_is_idempotent(cmd in arb_any_cmd()) {
        let id = Uuid::nil();
        let mut a = Toy::new(id);
        let mut b = Toy::new(id);

        cmd.apply(&mut a);
        cmd.apply(&mut b);
        let executed_again = cmd.apply(&mut b);

        prop_assert!(!executed_again, "second apply should be AlreadyApplied");
        prop_assert_eq!(a.event_stream(), b.event_stream());
    }

    /// Full retry safety for `already_applied`-only commands:
    /// `apply(cs ++ cs)` produces the same event stream as `apply(cs)`.
    /// This is the contract callers depend on — re-driving a command sequence
    /// after a crash/retry must not produce duplicate events.
    #[test]
    fn full_retry_is_noop_for_pure_commands(cmds in proptest::collection::vec(arb_pure_cmd(), 0..20)) {
        let id = Uuid::nil();
        let mut once = Toy::new(id);
        let mut twice = Toy::new(id);

        for c in &cmds {
            c.apply(&mut once);
        }
        for c in &cmds {
            c.apply(&mut twice);
        }
        // Replay
        for c in &cmds {
            let executed = c.apply(&mut twice);
            prop_assert!(!executed, "replay of {:?} pushed a new event", c);
        }

        prop_assert_eq!(once.event_stream(), twice.event_stream());
    }

    /// Extensional equality: the resulting event stream of an `already_applied`-only
    /// sequence equals the deduplicated first-occurrence subsequence of pushed events.
    /// (Not strictly needed for correctness, but a stronger statement that pins down
    /// what `already_applied` actually does.)
    #[test]
    fn pure_commands_dedup_by_first_occurrence(cmds in proptest::collection::vec(arb_pure_cmd(), 0..20)) {
        let id = Uuid::nil();
        let mut toy = Toy::new(id);
        for c in &cmds {
            c.apply(&mut toy);
        }

        // Build the expected stream: Initialized, then each unique command in
        // first-occurrence order mapped to its event.
        let mut seen_names: Vec<String> = Vec::new();
        let mut seen_tags: Vec<String> = Vec::new();
        let mut expected: Vec<ToyEvent> = vec![ToyEvent::Initialized { id }];
        for c in &cmds {
            match c {
                Cmd::SetName(n) if !seen_names.contains(n) => {
                    seen_names.push(n.clone());
                    expected.push(ToyEvent::Renamed { name: n.clone() });
                }
                Cmd::AddTag(t) if !seen_tags.contains(t) => {
                    seen_tags.push(t.clone());
                    expected.push(ToyEvent::Tagged { tag: t.clone() });
                }
                _ => {}
            }
        }

        prop_assert_eq!(toy.event_stream(), expected);
    }

    /// `resets_on` semantics: between two Activates, a Deactivate must allow
    /// the second Activate to fire. activate; deactivate; activate produces 3 events
    /// past the initial Initialized (any redundant intermediate calls dedup).
    /// Generalized: any non-empty alternation of Activate/Deactivate stays non-empty
    /// and never produces two consecutive identical states.
    #[test]
    fn resets_on_allows_alternation(toggles in proptest::collection::vec(prop::bool::ANY, 1..15)) {
        let id = Uuid::nil();
        let mut toy = Toy::new(id);
        for &active in &toggles {
            let _ = if active { toy.activate() } else { toy.deactivate() };
        }
        let stream = toy.event_stream();

        // No two consecutive Activated or two consecutive Deactivated events.
        for w in stream.windows(2) {
            match (&w[0], &w[1]) {
                (ToyEvent::Activated, ToyEvent::Activated) => {
                    prop_assert!(false, "consecutive Activated events: {:?}", stream);
                }
                (ToyEvent::Deactivated, ToyEvent::Deactivated) => {
                    prop_assert!(false, "consecutive Deactivated events: {:?}", stream);
                }
                _ => {}
            }
        }
    }
}

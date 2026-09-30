# errlanes

"Something went wrong" is rarely enough. A caller supplied an invalid amount, a
user lacks permission, a transaction hit a deadlock, stored state is corrupt —
each needs a different response, often from a different layer. The usual flat
error enum makes every layer re-derive which kind it is holding, and that
judgement gets re-made, inconsistently, at every hop.

errlanes sorts failures into four **lanes** and carries the choice in the type.
The layer that understands a failure assigns its lane once; every layer above
propagates it without interpreting it again.

| Lane | Means | Response |
|---|---|---|
| `Rejected(R)` | A domain outcome the caller can act on | Handle it, or tell the end user |
| `Denied` | The caller is not authorized | Audit and refuse |
| `Transient` | Retrying may succeed | Retry, within a budget |
| `Fatal` | A bug, misconfiguration, or corrupt state | Stop, surface, page |

Two carrier enums hold them. Setting lane selection aside for a moment:

```rust
use errlanes::{Denied, Fatal, Transient};

enum Fault {
    Denied(Denied),
    Transient(Transient),
    Fatal(Fatal),
}

enum Fail<R> {
    Rejected(R),
    Denied(Denied),
    Transient(Transient),
    Fatal(Fatal),
}
```

`Fault` covers the three outcomes whose handling does not depend on a domain
type. `Fail` adds `Rejected`, carrying an application-defined `R` so callers can
tell individual domain cases apart. Both keep the underlying source for
diagnosis.

Which carrier a function returns is itself information: a `Fault` says *nothing
about this call is the caller's to correct*.

## Selecting lanes

Most operations produce only some of the lanes. Storage might fail transiently
or fatally but has no business denying access; `lanes!` says so in the
signature:

```rust
use errlanes::{Fail, Fault, lanes};
# #[derive(Debug, thiserror::Error, errlanes::Rejection)]
# enum Validation {
#     #[error("amount must be positive")]
#     InvalidAmount,
# }

type ReadError = Fault<lanes!(Transient, Fatal)>;
type PayError = Fail<Validation, lanes!(Transient, Fatal)>;
```

A disabled lane is uninhabited, so no value can ever occupy it. A **by-value
match therefore names exactly the lanes you selected** — nothing to fill in for
the ones you left out:

```rust
use errlanes::{Fault, lanes};

fn describe(failure: Fault<lanes!(Transient, Fatal)>) -> &'static str {
    match failure {
        Fault::Transient(_) => "worth another attempt",
        Fault::Fatal(_) => "needs a human",
    }
}

// With a single lane left, there is nothing to match at all.
fn unwrap_fatal(failure: Fault<lanes!(Fatal)>) -> errlanes::Fatal {
    let Fault::Fatal(fatal) = failure;
    fatal
}
# let _ = describe(errlanes::Transient::new(errlanes::TransientKind::Deadlock).into());
# let _ = unwrap_fatal(errlanes::Fatal::invariant("x").into());
```

Two rules round this out. Lane order does not matter, and `lanes!()` selects no
fault lanes at all. And `lanes!` names only the three fault lanes: the
`Rejected` lane is selected by *which carrier* you use, `Fail` or `Fault`.

Matching a **borrowed** failure is the one place Rust still asks about
uninhabited variants, so reach for an accessor instead of a `match`. Each is
available only when the profile enables that lane:

```rust
use errlanes::{Fault, Lane, lanes};

fn log(failure: &Fault<lanes!(Transient, Fatal)>) -> Lane {
    if let Some(transient) = failure.as_transient() {
        assert!(transient.retry_after.is_none());
    }
    failure.lane()
}
# assert_eq!(log(&errlanes::Fatal::invariant("x").into()), Lane::Fatal);
```

`Fail` adds `as_rejected` to `as_denied` / `as_transient` / `as_fatal`, and
`lane()` works on any profile.

## Moving between signatures

A failure changes shape as it travels: the lane set grows when a caller can
produce outcomes the callee could not, and the rejection type changes when a
failure crosses into a domain with its own vocabulary. Three operations cover
every case:

| From | To | Use |
|---|---|---|
| the same carrier | itself | `?` |
| a lane marker (`Transient`, `Fatal`, `Denied`) | any carrier enabling it | `?` |
| `Fault<S>` | `Fail<R, D>` | `?` |
| a bare rejection `C` | `Fail<R, D>`, given `R: From<C>` | `?` |
| `Fail<C, S>` | `Fail<R, D>` | `.widen()?` |
| `Fault<S>` | `Fault<D>` | `.widen()?` |

`?` handles anything that needs no decision. `.widen()` is for the one case that
does — changing the rejection type — and its destination is inferred from the
return type. Widening only ever *adds* lanes: dropping one that the source can
still produce is a compile error, because somebody has to handle it.

### Authorization adds a lane

```rust
use errlanes::{Denied, Fault, WidenResult, lanes};

fn authorize(subject: &str) -> Result<(), Denied> {
    if subject == "admin" { Ok(()) } else { Err(Denied::default()) }
}

fn load() -> Result<u64, Fault<lanes!(Fatal)>> {
    Ok(42)
}

fn load_as(subject: &str) -> Result<u64, Fault<lanes!(Denied, Fatal)>> {
    authorize(subject)?;
    Ok(load().widen()?)
}

assert_eq!(load_as("admin").unwrap(), 42);
assert!(matches!(load_as("guest"), Err(Fault::Denied(_))));
```

The denial enters through `?` because `Denied` is a lane marker. The inner
`Fatal` survives `.widen()` with its kind, context and source intact.

### Retrying removes one

A transient failure is *born* where the meaning is known — usually at the edge
that talks to storage or an upstream service. That is the only place with the
evidence, so it is where classification belongs:

```rust
use errlanes::{Fault, Transient, TransientKind, lanes};
use std::time::Duration;

/// The storage edge: it knows a serialization failure is worth repeating, and
/// roughly when.
fn classify(sqlstate: &str) -> Fault<lanes!(Transient, Fatal)> {
    match sqlstate {
        "40001" | "40P01" => Transient::new(TransientKind::SerializationFailure)
            .with_context("commit lost to a concurrent writer")
            .with_retry_after(Duration::from_millis(10))
            .into(),
        other => errlanes::Fatal::from_error(
            errlanes::FatalKind::Dependency,
            std::io::Error::other(format!("sqlstate {other}")),
        )
        .into(),
    }
}
```

The caller that owns a *safe* repetition then handles the lane, and stops
offering it to its own callers. `settle` is what turns the attempt count into
an outcome: a transient that never succeeded becomes `Fatal(Exhausted)`,
keeping the last transient as its source.

```rust
use errlanes::{Fault, Transient, TransientKind, lanes};

const BUDGET: u32 = 3;

fn execute(
    mut attempt_once: impl FnMut() -> Result<u64, Fault<lanes!(Transient, Fatal)>>,
) -> Result<u64, Fault<lanes!(Fatal)>> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        match attempt_once() {
            Ok(value) => return Ok(value),
            Err(failure) if failure.is_transient() && attempts < BUDGET => continue,
            Err(failure) => return Err(failure.settle(attempts)),
        }
    }
}

// A transient that later succeeds never reaches the caller.
let mut attempts = 0;
let value = execute(|| {
    attempts += 1;
    if attempts == 1 {
        Err(Transient::new(TransientKind::Deadlock).into())
    } else {
        Ok(42)
    }
});
assert_eq!(value.unwrap(), 42);
assert_eq!(attempts, 2);

// One that never succeeds is spent, and says so.
let spent = execute(|| Err(Transient::new(TransientKind::Deadlock).into())).unwrap_err();
let Fault::Fatal(fatal) = spent;
assert_eq!(fatal.kind, errlanes::FatalKind::Exhausted);
let exhausted = std::error::Error::source(&fatal)
    .and_then(|source| source.downcast_ref::<errlanes::Exhausted>())
    .expect("the exhaustion is the fatal's source");
assert_eq!(exhausted.attempts, BUDGET);
assert_eq!(exhausted.last.kind, TransientKind::Deadlock);
```

Look at what `execute` returns. Settling consumed the transient lane, so
`lanes!(Transient, Fatal)` went in and `lanes!(Fatal)` came out: there is no
second family of settled carriers, just the same `Fault` over a smaller lane
set. A caller of `execute` has one lane to handle, and it destructures with a
single irrefutable `let`:

```rust
# use errlanes::{Fault, lanes};
fn only_fatal(failure: Fault<lanes!(Fatal)>) -> errlanes::FatalKind {
    let Fault::Fatal(fatal) = failure;
    fatal.kind
}
# assert_eq!(only_fatal(errlanes::Fatal::invariant("x").into()), errlanes::FatalKind::Invariant);
```

The `tokio` feature does all of this for you: `retry` and `retry_with` apply a
`RetryPolicy` (attempt budget, exponential backoff, jitter), honour a
transient's `retry_after`, and return the settled profile. Retry at the boundary
that owns a safe repetition — a lane says a failure *may* succeed on retry, not
that repeating an ambiguous commit is safe.

Because a settled profile has nowhere to put an exhaustion unless it admits
`Fatal`, `lanes!(Transient)` on its own is neither settleable nor retryable.
Anything worth retrying can fail permanently.

## Domain rejections

Some outcomes are neither retryable nor operator business: an amount is
invalid, an email is already registered. The caller may need to handle the case,
or tell an end user who can correct the request. Those belong in `Rejected`,
described by an ordinary `thiserror` enum:

```rust
use errlanes::{Level, Rejection};

#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Validation {
    #[error("amount must be positive")]
    #[rejection(code = "INVALID_AMOUNT")]
    InvalidAmount,
}

let rejection = Validation::InvalidAmount;
let public_code: &'static str = rejection.code().into();
assert_eq!(public_code, "INVALID_AMOUNT");
assert_eq!(rejection.level(), Level::Info);
```

The variant lets Rust callers match the case. The **code** is a stable identity
for an API boundary and for telemetry, and stays typed inside the application —
it becomes a string only at the wire, so a typo is a compile error and a
reworded message never breaks a consumer. Never build either from `Display`: a
rejection's message may embed caller-supplied input.

The **level** tells the recording boundary how loudly to log it. Rejections
default to `Info` because they are expected outcomes; `#[rejection(code = "…",
level = "warn")]` raises one without changing its lane.

### Rejections alongside faults

An operation can reject a request *and* trip over a fault while carrying it
out. `Fail<R, L>` expresses both:

```rust
use errlanes::{Fail, Fault, lanes};

#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Validation {
    #[error("amount must be positive")]
    InvalidAmount,
}

fn save() -> Result<(), Fault<lanes!(Fatal)>> { Ok(()) }

fn pay(amount: u64) -> Result<(), Fail<Validation, lanes!(Fatal)>> {
    if amount == 0 {
        return Err(Validation::InvalidAmount.into());
    }
    save()?;
    Ok(())
}

assert!(matches!(pay(0), Err(Fail::Rejected(Validation::InvalidAmount))));
assert!(pay(1).is_ok());
```

A function that only validates its input needs no carrier at all — return
`Result<T, Validation>` and let the caller's `?` place it in the `Rejected`
lane.

## Crossing a domain boundary

Moving up the stack, a failure may reach a domain with its own rejection enum.
The fault lanes already mean the same thing everywhere; the two rejection types
need a conversion, and `#[derive(Lift)]` writes it. The enum-level
`#[lift(Source)]` names the source; each destination variant says which source
case it stands for:

```rust
use errlanes::{Fail, Rejection, WidenResult, lanes};

#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Validation {
    #[error("amount must be positive")]
    #[rejection(code = "INVALID_AMOUNT")]
    InvalidAmount,
}

#[derive(Debug, thiserror::Error, errlanes::Rejection, errlanes::Lift)]
#[lift(Validation)]
enum Payment {
    #[error("payment amount must be positive")]
    #[lift(Validation::InvalidAmount)]
    AmountNotPositive,
}

fn inner() -> Result<(), Fail<Validation, lanes!(Fatal)>> {
    Err(Validation::InvalidAmount.into())
}

fn outer() -> Result<(), Fail<Payment, lanes!(Denied, Fatal)>> {
    inner().widen()?;
    Ok(())
}

let rejection = outer().unwrap_err().rejected().unwrap();
assert!(matches!(rejection, Payment::AmountNotPositive));
assert_eq!(Into::<&'static str>::into(rejection.code()), "INVALID_AMOUNT");
```

A mapping accounts for every source case by default: the derive generates an
exhaustive match plus `From`, so a new `Validation` case fails to compile until
this boundary decides what it means. Unit, tuple and named payloads forward
automatically; `#[lift(Source::Variant, with = mapper)]` handles a genuine
payload transformation.

Conversion and identity stay separate. `Lift` generates the conversion,
`Rejection` the code and level, and either derive works alone. When both are
present a simple mapping also forwards the source's metadata — the renamed
`AmountNotPositive` above keeps the code `INVALID_AMOUNT` and its level, because
a prefix or an intermediate Rust name is not a change of meaning. Declare
`#[rejection(code = "…")]` on the destination when the mapping genuinely
reinterprets the outcome.

### When only some cases belong

Sometimes only a subset of the source cases makes sense in the destination
domain, and the rest would mean a violated invariant.
`#[lift(Source, unhandled = fatal)]` declares that: mapped cases become
rejections, and anything unmapped becomes `Fatal(Invariant)` with the original
rejection as its source.

That is a property of the destination enum, declared once — so the call site is
the same `.widen()?` either way. Partial mappings need the destination to admit
`Fatal`, which the compiler checks, and they generate no `From`: there is no
infallible conversion to be had.

## Composing a whole family

An outer domain sometimes wants to expose *every* case of an inner family,
including ones added later. There is no per-case decision to make, so
`#[errlanes::compose]` states the relationship instead:

```rust
use errlanes::Rejection;

#[derive(Debug, thiserror::Error, errlanes::Rejection)]
pub enum Validation {
    #[error("amount must be positive")]
    #[rejection(code = "INVALID_AMOUNT")]
    InvalidAmount,
}

#[errlanes::compose]
#[derive(Debug, thiserror::Error)]
pub enum Payment {
    #[compose(flatten)]
    Amount(Validation),
    #[error("payment window is closed")]
    WindowClosed,
}

let payment = Payment::from(Validation::InvalidAmount);
assert!(matches!(payment, Payment::AmountInvalidAmount));
assert_eq!(Into::<&'static str>::into(payment.code()), "INVALID_AMOUNT");
```

The placeholder `Amount(Validation)` expands into real variants —
`AmountInvalidAmount` — lending its name as the prefix. Imported cases keep
their payloads, codes, levels, formatting and sources. `compose` supplies both
`Rejection` and `Lift`, so the result widens like any other family, and it can
sit alongside explicit lifts from other sources.

Composition picks up future source cases on purpose. Prefer an explicit mapping
when each addition deserves review or a case needs its own name; prefer reusing
the inner type outright when a layer adds no meaning of its own.

## Recording, at one boundary

The `tracing` feature turns a failure into span fields, once, at the boundary
that disposes of it — not at every layer it passed through. `record_fail` takes
a `Fail`, `record_fault` a `Fault`; both accept settled profiles.

```rust,ignore
let span = tracing::info_span!("http.request", /* errlanes::FIELDS as Empty */);
if let Err(ref failure) = outcome {
    errlanes::record_fail(&span, failure);
}
```

Declare `errlanes::FIELDS` on the span as `tracing::field::Empty`. `error.code`
and `error.level` come from `Rejection` for a rejection, and from the lane
otherwise: `FORBIDDEN`/`WARN` for denied, the transient or fatal kind
otherwise. Only `Fatal` writes `exception.message` and `exception.type`, where
the text is operator-facing by construction.

`lane_of` and `transient_of` recover a lane from an arbitrary `dyn Error` by
walking its source chain, including through boxed or newtype wrappers — the
interop point for code that has not adopted these carriers, alongside
`#[derive(Classify)]` for classifying a legacy enum. A wrapper must expose its
source for either to work.

## Features

| Feature | Default | Provides |
|---|---|---|
| `derive` | yes | `Rejection`, `Lift`, `Failure`, `Classify`, `compose` |
| `tokio` | no | `retry`, `retry_with`, `RetryPolicy` |
| `tracing` | no | `record_fail`, `record_fault`, `FIELDS` |
| `sqlx` | no | `classify_sqlx`, `lane_of_sqlx`, `From<sqlx::Error>` |

The `sqlx` classifier maps serialization failures, deadlocks, pool timeouts and
connection loss to `Transient`, and everything else — including `Protocol`
errors, which are not retryable connection loss — to `Fatal`. A repository
should recognise its own constraints before falling back to it; an unknown
constraint is an invariant, not a domain outcome.

---

Every Rust snippet above is a doctest:

```sh
nix develop -c cargo test --profile mdbook-test -p errlanes --doc
```

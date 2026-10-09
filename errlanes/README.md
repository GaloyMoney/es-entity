# errlanes

Errors communicate that something went wrong up the call stack. What went wrong
can vary widely: a caller supplied an invalid amount, a user lacks permission,
a database transaction hit a deadlock, or the application encountered corrupt
state. Each calls for a different response, often from a different part of the
application. Representing that information so callers can decide what to do
becomes harder as errors pass through several layers.

This crate takes an opinionated approach: categorize errors into four *lanes*,
each with a distinct meaning for the caller. The layer that understands an error
assigns its lane; callers can then handle or propagate it without having to
interpret the original error again.

The lanes are represented by two carrier enums. Their structure looks like this:

```rust
use errlanes::{Denied, Fatal, Transient};

enum Fault {
    Denied(Denied),       // The caller is not authorized.
    Transient(Transient), // Retrying the operation may succeed.
    Fatal(Fatal),         // Operator attention is needed.
}

enum Fail<R> {
    Rejected(R),          // A domain outcome the caller can act on.
    Denied(Denied),
    Transient(Transient),
    Fatal(Fatal),
}
```

`Fault` carries the three outcomes whose handling does not depend on a domain
error type. `Fail` adds Rejected, carrying an application-defined type `R` so
callers can distinguish individual domain cases. Both preserve the underlying
source for diagnosis.

Which carrier a function returns is itself information: a `Fault` says that
nothing about this call is the caller's to correct.

## Faults and lane selection

A particular operation will usually produce only some of these outcomes.
For example, a storage function might return Transient or Fatal but have no
reason to deny access. Its signature can express that with
`Fault<lanes!(Transient, Fatal)>`.

The `lanes!` macro selects which fault lanes a type permits. That selection
tells callers what they need to handle. It can change as an error travels up
the call stack: a caller may introduce an additional outcome or take
responsibility for handling one.

A disabled lane is uninhabited, so no value can ever occupy it. A by-value
match therefore names exactly the lanes that were selected:

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

### Example: Authorization adds a lane

Consider an inner operation that can only fail fatally. An outer function
checks whether the subject is allowed to perform it before calling it. The
outer function can therefore return Denied as well as Fatal:

```rust
use errlanes::{Denied, Fault, ResultExt, lanes};

fn authorize(subject: &str) -> Result<(), Denied> {
    if subject == "admin" { Ok(()) } else { Err(Denied::default()) }
}

fn inner() -> Result<u64, Fault<lanes!(Fatal)>> {
    Ok(42)
}

fn outer(subject: &str) -> Result<u64, Fault<lanes!(Denied, Fatal)>> {
    authorize(subject)?;
    let value = inner()?;
    Ok(value)
}

assert_eq!(outer("admin").unwrap(), 42);
assert!(matches!(outer("guest"), Err(Fault::Denied(_))));
```

The authorization failure enters the Denied lane through `?`. The inner
operation's result uses `?` to fit the outer function's larger set of
lanes. An inner Fatal remains Fatal, with its kind, context and
source intact.

### Example: Retrying handles a lane

The reverse situation occurs when an inner operation can return Transient,
but its caller owns retrying the operation. After handling Transient, that
caller only needs to expose Fatal to its own callers. `narrow_transient` turns
the attempt count into an outcome: a transient that never succeeded becomes
`Fatal(Exhausted)`, keeping the last transient as its source.

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
            Err(failure) => return Err(failure.narrow_transient(attempts)),
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
```

Here, `execute` takes the inner operation as a closure so it can call it again.
Its return type omits Transient because the loop handles that case: narrowing
consumed the lane, so `lanes!(Transient, Fatal)` went in and `lanes!(Fatal)`
came out.

## Narrowing a lane

As in the retry example, any lane can be narrowed once its owner has handled
it. `narrow_<lane>` removes exactly one lane and turns its value into a
`Fatal`, the only lane left once nobody can act on the removed one. A
narrowing is always a method call, never a `From`, so `?` cannot drop a lane
by accident. The same methods are available on a `Result` through `ResultExt`.

| narrowed lane | what the value is afterwards | method | `FatalKind` |
|---|---|---|---|
| `Transient` | a transient that ran out of retries | `narrow_transient(attempts)` | `Exhausted` |
| `Denied` | still a denial, fatal only because nobody can be told | `narrow_denied()` | `Denied` |
| `Rejected(D)` | a caller-correctable outcome with no caller, which is a bug | `narrow_rejected()` | `Invariant` |

## Handling the Rejected lane

A rejection is a domain outcome the caller can act on, so it is usually
handled rather than narrowed. `rejected()` hands the caller the rejection as
a value: `Result<T, Fail<D, L>>` becomes `Result<Result<T, D>, Fault<L>>`.
The outer `?` keeps the faults propagating; the inner `Result` is the domain
outcome, matched on the spot.

```rust
use errlanes::{Fail, Fault, ResultExt, lanes};

#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "TIMED_OUT")]
struct TimedOut;

fn await_completion() -> Result<u64, Fail<TimedOut, lanes!(Transient, Fatal)>> {
    Err(Fail::Rejected(TimedOut))
}

fn poll_once() -> Result<Option<u64>, Fault<lanes!(Transient, Fatal)>> {
    match await_completion().rejected()? {
        Ok(outcome) => Ok(Some(outcome)),
        Err(TimedOut) => Ok(None),
    }
}

assert!(poll_once().unwrap().is_none());
```

`narrow_rejected()` is for the opposite situation: a frame that has already
proved the precondition has no caller left to correct the rejection, so the
rejection is an invariant there.

### Rejections across domain boundaries

As an error moves up the call stack, it may cross into a domain with its own
rejection enum. The fault lanes already have shared meanings, but two domain
enums need an explicit conversion. `derive(Lift)` generates it: the enum-level
`#[lift(Validation)]` names the source, and an annotation on each destination
variant says which source case it represents:

```rust
use errlanes::{Fail, Rejection, ResultExt, lanes};

#[derive(Debug, errlanes::Rejection)]
enum Validation {
    #[rejection(code = "INVALID_AMOUNT")]
    InvalidAmount,
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Validation)]
enum Payment {
    #[lift(Validation::InvalidAmount)]
    AmountNotPositive,
}

fn validate(amount: u64) -> Result<(), Validation> {
    if amount == 0 { Err(Validation::InvalidAmount) } else { Ok(()) }
}

fn outer() -> Result<(), Fail<Payment, lanes!(Denied, Fatal)>> {
    validate(0)?;
    Ok(())
}

let rejection = outer().unwrap_err().rejected().unwrap();
assert!(matches!(rejection, Payment::AmountNotPositive));
assert_eq!(Into::<&'static str>::into(rejection.code()), "INVALID_AMOUNT");
```

By default a lift is exhaustive. The derive emits `From<Validation>`, so `?`
places the rejection in the outer Rejected lane, and if `Validation` gains a
case the mapping must be updated before it compiles. Unit, tuple and named
payloads forward automatically; `#[lift(Source::Variant, with = mapper)]`
transforms a payload, `into` converts it through `Into`, and `field = name`
keeps one named field of it. A plain lift arm also forwards the source case's
code and level, which is why `AmountNotPositive` above still reports
`INVALID_AMOUNT`; a `with`, `into` or `field` arm declares its own
`#[rejection(code = "...")]` instead, or `#[rejection(delegate)]` to take
them from the converted payload.

Sometimes only a subset of the source cases makes sense in the destination
domain. `#[lift(Source, unhandled = fatal)]` allows a partial mapping: an
unmapped rejection becomes `Fatal(Invariant)` with the original as its source,
so the destination must enable Fatal and the call site is `.lift()?` rather
than `?`. A named-field struct can select one source variant this way with
`variant = Case`, renaming fields with `#[lift(from = source_field)]`:

```rust
use errlanes::{Fail, ResultExt, lanes};
use std::time::Duration;

#[derive(Debug, errlanes::Rejection)]
enum SubscriptionRejection {
    CaughtUpTimeout { checkpoint: u64, target: u64, waited: Duration },
    NoSuchJob { key: String },
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[rejection(code = "EC_CAUGHT_UP_TIMEOUT")]
#[error("checkpoint {applied} had not reached {frontier} after {waited:?}")]
#[lift(SubscriptionRejection, variant = CaughtUpTimeout, unhandled = fatal)]
struct EcCaughtUpTimeout {
    #[lift(from = checkpoint)]
    applied: u64,
    #[lift(from = target)]
    frontier: u64,
    waited: Duration,
}

let result: Result<(), Fail<EcCaughtUpTimeout, lanes!(Fatal)>> =
    Err(SubscriptionRejection::CaughtUpTimeout {
        checkpoint: 3,
        target: 8,
        waited: Duration::from_secs(2),
    }).lift();
let Fail::Rejected(timeout) = result.unwrap_err() else { panic!("expected timeout") };
assert_eq!((timeout.applied, timeout.frontier), (3, 8));
```

A struct source lifts as a whole value: `#[lift(Payload)]` with no variant
suffix, on both the enum and the variant that receives it.

## At a `Box<dyn Error>` boundary

A `Classify` wrapper (see "Local errors" below) carries its lane in its
`impl Classify`, not in the value, so it must reach a carrier *before* it is
boxed; a boundary reading the box afterwards would only see the foreign error
underneath it. `.into_fault()?` lands a never-rejecting wrapper in its own
`Fault<W::Lanes>`, and `.into_fail()?` lands a rejecting one in its own
`Fail<W::Rejected, W::Lanes>`, so nothing has to be named even though `?`
into a box leaves the destination unconstrained. Raw foreign errors and
carriers need no such step. On the way back out, `Fault::classify(&*boxed)`
borrows the error and returns the first lane payload or blessed foreign error
in its `source()` chain, or `Fatal(Dependency)` if it finds neither. A
rejection cannot cross a box as a rejection: handle or narrow it first.

```rust
use errlanes::{Fault, ResultExt};

#[derive(Debug, errlanes::Classify)]
#[classify(fatal(CorruptState), from)]
#[error("could not decode a stored row")]
struct StoredRow(#[source] std::io::Error);

fn decode() -> Result<u8, StoredRow> {
    Err(StoredRow(std::io::Error::other("bad bytes")))
}

// Inside a boundary whose own trait returns a box:
fn run() -> Result<u8, Box<dyn std::error::Error + Send + Sync>> {
    Ok(decode().into_fault()?)
}

let fault = Fault::classify(&*run().unwrap_err());
assert!(matches!(fault, Fault::Fatal(f) if f.kind == errlanes::FatalKind::CorruptState));
```

## Local errors

Every local error type says, through one trait, how it enters the lanes:

```rust,ignore
pub trait Classify: Error + Send + Sync + 'static {
    type Rejected: RejectedSlot; // a `Rejection`, or `Infallible` if nothing is ever rejected
    type Lanes: LaneProfile;     // the narrowest profile the fault part can occupy
    fn classify(self) -> Fail<Self::Rejected, Self::Lanes>;
}
```

A type is one of three shapes, never more than one:

| type | `Rejected` | `Lanes` | how it is written |
|---|---|---|---|
| a domain outcome | itself | none | `#[derive(errlanes::Rejection)]` |
| a fault wrapper | `Infallible` | the lane(s) it names | `#[derive(errlanes::Classify)] #[classify(fatal(Kind))]` |
| a mixed wrapper | a domain outcome | the lane(s) its faulty variants name | `#[derive(errlanes::Classify)]`, variant by variant |

**A type is a `Rejection` or it implements `Classify` directly, never both** —
the blanket `impl<R: Rejection> Classify for R` makes a second, direct impl
conflict (`E0119`). The split is principled: a rejection's `Display` may embed
caller input and is never operator-facing; a fault's `Display` is exactly what
an operator-facing message must show.

A pure domain outcome — one that is never a fault, only ever rejected — is the
common case and the one most code writes. `derive(Rejection)` gives each
outcome a stable `code()` and a recording `level()`; `Classify` then comes
from the blanket above, not from a second derive:

```rust
use errlanes::{Level, Rejection};

#[derive(Debug, errlanes::Rejection)]
enum Validation {
    #[rejection(code = "INVALID_AMOUNT")]
    InvalidAmount,
}

let rejection = Validation::InvalidAmount;
let public_code: &'static str = rejection.code().into();
assert_eq!(public_code, "INVALID_AMOUNT");
assert_eq!(rejection.level(), Level::Warn);
```

The enum variant lets Rust callers match the case. The code lets an API expose
a stable identity and lets telemetry group occurrences of the same outcome,
even if its human-readable message changes. Codes remain typed inside the
application and can be converted to strings at the API boundary. Never build
either from `Display`: a rejection's message may embed caller-supplied input.

The level tells the recording boundary how severely to log the rejection.
Warn is the default: a rejection is a refused request an operator should be
able to see. A case that needs different operational visibility can override
it with, for example, `#[rejection(code = "INVALID_AMOUNT", level = "info")]`. Its lane is still Rejected.

## Errors from other crates

`sqlx::Error`, `serde_json::Error`, and anything else outside this crate never
implements `Classify` directly — errlanes cannot know what a `404` from one
caller's upstream means versus another's. The fix is the same one always: wrap
it in a local type, and give *that* a lane. A one-off call site does this with
`.classify::<W>()`, which turns a `Result<T, Foreign>` into a `Result<T, W>`:

```rust
use errlanes::{FatalKind, ResultExt};

#[derive(Debug, errlanes::Classify)]
#[classify(fatal(CorruptState), from)]
struct Stored(std::io::Error);

fn decode() -> Result<u8, std::io::Error> { Err(std::io::Error::other("x")) }

let wrapped: Result<u8, Stored> = decode().classify::<Stored>();
assert_eq!(wrapped.unwrap_err().0.to_string(), "x");
```

A function whose own error type already implements `Classify` gets this for
free through `From`, with bare `?` — no `.classify()` needed at the call site:

```rust,ignore
mod price_feed {
    // foreign → wrapper by `?` (the `from` flag's `From` impl)
    pub fn spot() -> Result<Quote, PriceFeed> {
        let response = client.get(url).send()?;
        Ok(response.json()?)
    }
}
```

**Same foreign error, two crates, two meanings.** A third-party price feed's
`404` on a known route is a domain outcome; its timeouts and 5xx retry; its
`401`/`403` are *our* credentials, not the caller's, so they are narrowed.
A different crate wrapping the exact same `reqwest::Error` from an in-house
service can decide every failure there is a deployment problem instead:

```rust,ignore
#[derive(Debug, errlanes::Rejection)]
pub enum PriceFeedRejection {
    #[rejection(code = "PRICE_FEED_UNKNOWN_SYMBOL")]
    UnknownSymbol(String),
}

#[derive(Debug, errlanes::Classify)]
pub enum PriceFeed {
    #[classify(delegate)]                 Rejected(PriceFeedRejection),
    #[classify(delegate, narrow(Denied))]  Http(reqwest::Error),
}
impl From<reqwest::Error> for PriceFeed { /* 404 on /symbols/{s} -> Rejected(UnknownSymbol(s)), else Http(e) */ }

#[derive(Debug, errlanes::Classify)]
#[classify(fatal(Config), from)]
pub struct FxRates(reqwest::Error); // our own service: any failure is ours

async fn quote() -> Result<Quote, Fail<QuoteRejection, lanes!(Transient, Fatal)>> {
    let spot = feed.get(url).send().await.classify::<PriceFeed>()?;  // lifts PriceFeedRejection
    let rate = rates.get(url).send().await.classify::<FxRates>()?;   // always Fatal(Config)
    Ok(spot * rate)
}
```

`delegate` forwards to the field's own `Classify`; `narrow(Denied)` then
removes just the denied lane (an upstream `401`/`403` becomes `Fatal(Denied)`
instead) before the arm's contribution is folded into the enclosing type's
`Lanes`. `Rejected` and `Lanes` are always inferred this way — never named on
the derive. The payload is the variant's only field, or — among several named
fields — the one marked `#[source]`, or named `source`. `from` additionally
needs the payload to be the only field, since `From` has nothing to fill
siblings with.

**`classify-sqlx`, `classify-serde-json`, and `classify-reqwest`** are
`impl Classify` for the three foreign types errlanes blesses on your behalf —
always `Rejected = Infallible`, the narrowest `Lanes` each warrants (see the
module docs for the exact table). With the feature enabled, bare `?` works on
the foreign type directly, and a wrapper's `delegate` arm can name it like any
other `Classify` payload. `classify-reqwest`'s `Denied` means *the subject of
this call* is unauthorized: an upstream `401`/`403` returned to a service
account is usually a credential or configuration fault at *our* layer, not the
caller's — narrow it with `#[classify(delegate, narrow(Denied))]` (→
`Fatal(Denied)`), or match the status in a hand-written wrapper to get
`Fatal(Config)` instead. A proxy forwarding the caller's own token to upstream
is the case that keeps `Denied` as-is.

## Moving between signatures

A failure changes shape as it travels: the lane set grows when a caller can
produce outcomes the callee could not, and the rejection type changes when a
failure crosses into a domain with its own vocabulary. The appropriate
conversion depends on what changes:

| From | To | Use |
|---|---|---|
| the same error type | itself | `?` |
| a lane payload (`Transient`, `Fatal`, `Denied`) | a built-in or carrier enabling it | `?` |
| `Fault<S>` | `Fault<D>` or `Fail<R, D>`, `S ⊆ D` | `?` |
| `Fail<R, S>` | `Fail<R, D>`, `S ⊆ D` | `?` |
| a bare rejection `C` | `Fail<R, D>`, given a total `R: From<C>` | `?` |
| `W: Classify` | `Fail<R, D>`, given a total `R: From<W::Rejected>` and `W::Lanes ⊆ D` | `?` |
| `W: Classify<Rejected = Infallible>` | `Fault<D>`, `W::Lanes ⊆ D` | `?` |
| a rejecting result (`Fail`, bare rejection, wrapper, carrier) | `Fail<P, D>` or a `Fail`-like carrier, with `Fatal` enabled and a total or partial `P: Lift<R>` | `.lift()?` |
| a never-rejecting result | its own `Fault<W::Lanes>` at a box or foreign carrier boundary | `.into_fault()?` |
| a rejecting result | its own `Fail<R, W::Lanes>` at a foreign carrier boundary | `.into_fail()?` |
| `Result<T, Foreign>` | `Result<T, W>` | `.classify::<W>()`, given `W: Classify + From<Foreign>` |
| `Result<T, Fail<R, L>>` | `Result<T, Fault<L>>` | `.narrow_rejected()` |
| `Result<T, R>`, a bare rejection | `Result<T, Fatal>` after the precondition is proven | `.narrow_rejected()?` |
| `Result<T, Fail<R, L>>` | `Result<Result<T, R>, Fault<L>>` | `.rejected()` |
| `Result<T, Fail<R, L>>` | `Result<T, Fail<P, L>>` | `.map_rejected(f)` for call-site data |
| a `Fault` or `Fail` result | lanes narrowed | `.narrow_transient(attempts)` or `.narrow_denied()` |

`?` expands lanes without dropping one. It also carries a *total* mapping from
a bare rejection or `Classify` wrapper into `Fail`; a partial mapping requires
`.lift()?`. A `Fail<R, S>` changing its rejection to `P` also uses `.lift()?`,
whether the lift is total or partial. `ResultExt::lift` enables `Fatal` before
the following `?`, so an unmapped case has a place to go. Consequently its
destination must enable `Fatal`. For a total `Fail` lift into a signature
without `Fatal`, use the value-level `Fail::lift::<P, D>` explicitly. Narrowing
removes a lane by name after its outcome has been handled.

## Carriers: your own `Fault` / `Fail`

`Fault<L>` and `Fail<R, L>` are one type per profile, so two crates that both
return `Fault<lanes!(Transient, Fatal)>` share it, and a `thiserror` enum that
wants `#[from]` for both collides. A **carrier** is a crate-local type that
stands in for one of them:

```rust
# use errlanes::Rejection;
# #[derive(Debug, errlanes::Rejection)]
# pub enum CustomerRejection { Closed }
#[derive(Debug, errlanes::Carrier)]
pub enum HostFault {
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

#[derive(Debug, errlanes::Carrier)]
pub enum CustomerError {
    Rejected(CustomerRejection),
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

#[derive(Debug, errlanes::Carrier)]
pub enum RepoWriteError<R> {
    Rejected(R),
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

#[derive(Debug, errlanes::Carrier)]
#[carrier(from(HostFault))]
pub enum PartyFault {
    Denied(errlanes::Denied),
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}
```

Each is an **enum with exactly the declared lanes**, so it matches like the
built-in, and a lane it does not declare is not a variant at all:

```rust
# #[derive(Debug, errlanes::Carrier)]
# pub enum HostFault {
#     Transient(errlanes::Transient),
#     Fatal(errlanes::Fatal),
# }
fn page(e: HostFault) -> &'static str {
    match e {
        HostFault::Transient(_) => "retry",
        HostFault::Fatal(_) => "page",
    }
}
# assert_eq!(page(HostFault::Fatal(errlanes::Fatal::invariant("x"))), "page");
```

What `?` does, in and out of a carrier `E`:

- **In:** any lane payload `E` declares; a `Fault<S>` with `S ⊆ E`'s lanes; for
  a `Fail`-like carrier, a `Fail<R, S>` with the same rejection; any
  `Classify` wrapper whose lanes fit; a bare `Rejection` that lifts totally
  (`Fail`-like only); and each carrier listed in `from(..)`.
- **Out:** into `Fault<M>` (`Fault`-like carriers, `S ⊆ M`), into `Fail<D, M>`, into
  a bare `Fatal` / `Transient` when it has just that lane, and into a foreign
  enum with `#[from] E`.
- `.lift()`, `into_fault`, `into_fail`, `narrow_transient`, `narrow_denied`, `narrow_rejected`,
  `rejected` and `map_rejected` work on `Result<T, E>`. A narrowing returns the
  narrowed **built-in** (`Fault<..>` / `Fail<..>`), which `?` carries on.
  `E: Laned` for retry, `record` and `#[errlanes::instrument]`.

**Carrier to carrier is not automatic.** `?` is `From::from`, and a blanket
`impl From<AnyCarrier> for E` would also cover `E` itself, which overlaps the
standard library's reflexive `impl<T> From<T> for T`. So the conversion you
want is listed: `#[carrier(from(HostFault))]` adds `impl From<HostFault> for E`. The list
only works for carriers declared in the *same crate* as `E` (see below). The
list-free route works for any carrier, in any crate: hand it over as its
built-in, and the outer `?` absorbs that.

```rust
# use errlanes::{ResultExt};
# #[derive(Debug, errlanes::Carrier)]
# pub enum HostFault {
#     Transient(errlanes::Transient),
#     Fatal(errlanes::Fatal),
# }
# #[derive(Debug, errlanes::Carrier)]
# pub enum PartyFault {
#     Denied(errlanes::Denied),
#     Transient(errlanes::Transient),
#     Fatal(errlanes::Fatal),
# }
fn host() -> Result<u8, HostFault> {
    Ok(1)
}

fn party() -> Result<u8, PartyFault> {
    let v = host().into_fault()?;
    Ok(v)
}
# assert_eq!(party().unwrap(), 1);
```

The variant names (`Rejected`, `Denied`, `Transient`, `Fatal`) are the
profile: each is a one-field tuple variant, at most one of each, and
`Rejected(T)` makes the carrier `Fail`-like. Variants may carry doc comments,
and `Debug` is yours to derive.

**Limitation.** `from(..)` cannot list a carrier declared in another crate,
and neither can a hand-written `impl From<OtherCrate::Carrier> for E`: the
carrier's one blanket inbound `From` overlaps it, because rustc cannot know
the other crate's carrier is not a plain lane source. Use
`.into_fault()?` or `.into_fail()?` across crates, and in generic code over a foreign
carrier, bound on its built-in (`E: From<Fault<lanes!(Transient, Fatal)>>`)
rather than on the carrier.

## Composing rejection families

An outer domain may also want to expose every case from an inner rejection
family, including cases added in the future. In that situation, there is no
individual mapping decision to make for each case.
`#[errlanes::compose(..)]` expresses that relationship directly. It takes the
source families to import and generates both `Rejection` and `Lift` for the
composed enum.

A source listed as `Source as Prefix` imports every one of its cases under a
prefixed name:

```rust
use errlanes::Rejection;

#[derive(Debug, errlanes::Rejection)]
pub enum Validation {
    #[rejection(code = "INVALID_AMOUNT")]
    InvalidAmount,
}

#[errlanes::compose(Validation as Amount)]
#[derive(Debug)]
pub enum Payment {
    WindowClosed,
}

let payment = Payment::from(Validation::InvalidAmount);
assert!(matches!(payment, Payment::AmountInvalidAmount));
assert_eq!(Into::<&'static str>::into(payment.code()), "INVALID_AMOUNT");
```

`Validation as Amount` expands into real variants such as
`AmountInvalidAmount`. The imported cases keep their payloads, codes, levels,
formatting, and sources. The attribute supplies both `Rejection` and `Lift`,
including total `From` conversions, so the resulting family supports
`?` for bare rejections and `.lift()?` for failures, and it can sit alongside
explicit lifts from other sources.

Composition includes new source cases automatically. Explicit lifts are
useful when each addition needs review or individual cases need different
names. When an outer layer adds no semantics of its own, it can simply reuse
the inner rejection type.

### The code catalogue

A boundary that speaks a schema (a GraphQL enum, an OpenAPI `enum`) needs
every code a family can resolve to, without a hand-written copy. Every
`Rejection::Code` implements `errlanes::RejectionCode`, whose `CODES` is that
list, one `CodeInfo { code, description }` per code:

- **Complete.** It follows `delegate`, `code_and_level_from`, `#[lift(..)]`
  and `compose`, however deeply nested. A variant that lifts *one* source
  variant contributes only that variant's codes, not all of the source's.
- **Deduplicated.** A code reached along several paths (a diamond) appears
  once, at its first occurrence.
- **Ordered.** Declaration order, depth-first.

`FooCode::ALL` is the `code` of every entry of `CODES`, in the same order. It
is complete, not leaves-only.

A leaf's description resolves in order:

1. `#[rejection(description = "..")]`, if present.
2. Else the first paragraph of the leaf's `///` doc comment, its lines joined
   by a space.
3. Else `None`.

The `#[error("..")]` literal is never used: it is a `Display` template, not
prose, and may interpolate. `description` is for when the doc comment is
aimed at developers and the catalogue needs different wording.

`description` belongs on a leaf; on a variant that forwards to another type it
is a compile error.

```rust
use errlanes::{CodeInfo, Rejection, RejectionCode};

#[derive(Debug, errlanes::Rejection)]
pub enum CloseRejection {
    /// The customer still has open facilities.
    #[error("customer {customer_id} still has open facilities")]
    HasOpenFacilities { customer_id: u64 },
    #[rejection(description = "The customer is already closed.")]
    #[error("customer {customer_id} is closed")]
    AlreadyClosed { customer_id: u64 },
    #[error("customer {customer_id} is frozen")]
    Frozen { customer_id: u64 },
}

assert_eq!(
    <CloseRejectionCode as RejectionCode>::CODES,
    &[
        CodeInfo {
            code: "HAS_OPEN_FACILITIES",
            description: Some("The customer still has open facilities."),
        },
        CodeInfo {
            code: "ALREADY_CLOSED",
            description: Some("The customer is already closed."),
        },
        CodeInfo {
            code: "FROZEN",
            description: None,
        },
    ]
);

// A boundary is generic over the family and never names a `*Code` type:
fn publish<R: Rejection>(add_value: &mut impl FnMut(&str, Option<&str>)) {
    for entry in <R::Code as RejectionCode>::CODES {
        add_value(entry.code, entry.description);
    }
}
fn value<R: Rejection>(rejection: &R) -> &'static str {
    rejection.code().into()
}

let mut published = Vec::new();
publish::<CloseRejection>(&mut |code, _| published.push(code.to_owned()));
assert_eq!(published, ["HAS_OPEN_FACILITIES", "ALREADY_CLOSED", "FROZEN"]);
assert_eq!(
    value(&CloseRejection::AlreadyClosed { customer_id: 1 }),
    "ALREADY_CLOSED"
);
```

errlanes exposes data only: the schema shape and type names stay with the
consumer. A hand-written `Rejection` impl must also implement
`RejectionCode` for its `Code` type.

---

The Rust snippets above are checked by the crate's doctests. The opening enum
sketch compiles, and the examples execute their assertions:

```sh
nix develop -c cargo test --profile mdbook-test -p errlanes --doc
```

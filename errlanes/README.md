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
hands the mapper the whole source value and returns the destination,
`into` converts the payload through `Into`, and `field = name`
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

The `From` an `into` arm needs is derived on the payload type when its shape
is mechanical: `#[lift(Source)]` on a unit struct drops the value, on a
single-field tuple struct wraps it, and `#[lift(Source, field = name)]` (or
`field = 0`) keeps one field, converting it through `Into`.

```rust
use errlanes::{Lift, Rejection};

#[derive(Debug)]
struct LimitExceeded { limit: u64 }

#[derive(Debug, errlanes::Rejection)]
enum Limit {
    #[rejection(code = "LIMIT_EXCEEDED")]
    Exceeded(LimitExceeded),
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[rejection(code = "PAYMENT_EXCEEDS_OUTSTANDING")]
#[lift(LimitExceeded)]
struct PaymentExceedsOutstanding;

fn is_large(exceeded: &LimitExceeded) -> bool { exceeded.limit > 100 }

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Limit)]
enum Payment {
    #[lift(Limit::Exceeded, when = is_large, into)]
    #[rejection(code = "PAYMENT_EXCEEDS_OUTSTANDING")]
    Outstanding(PaymentExceedsOutstanding),
    #[lift(Limit::Exceeded)]
    Exceeded(LimitExceeded),
}

let payment = Payment::lift(Limit::Exceeded(LimitExceeded { limit: 500 })).unwrap();
assert!(matches!(payment, Payment::Outstanding(_)));
```

A struct source lifts as a whole value: `#[lift(Payload)]` with no variant
suffix, on both the enum and the variant that receives it. Adding `into`
(`#[lift(Payload, into)]`) converts the whole value through `Into` first; the
source may then be an enum too, and an unguarded whole-value `into` is the
entire conversion.

A case whose payload the destination already accepts forwards with an
enum-level `#[lift(Source::Case, into)]`, and a case that belongs in a
composed-in variant, which has no declaration to annotate, is
`#[lift(Source::Case, to = Variant)]`:

```rust
use errlanes::Lift;

#[derive(Debug, errlanes::Rejection)]
enum Transition {
    #[rejection(code = "STALE")]
    Stale,
}

#[derive(Debug, errlanes::Rejection)]
enum Update {
    #[rejection(delegate)]
    Transition(Transition),
    #[rejection(code = "INVALID")]
    Invalid,
}

#[errlanes::compose(Transition)]
#[derive(Debug)]
#[lift(Update, unhandled = fatal)]
#[lift(Update::Transition, into)]
enum UpdateFamily {}

assert!(matches!(
    UpdateFamily::lift(Update::Transition(Transition::Stale)),
    Ok(UpdateFamily::Stale)
));
assert!(UpdateFamily::lift(Update::Invalid).is_err());
```

When one source variant carries several cases, `when = predicate` guards an
arm: the predicate is a path to a `fn(&Payload) -> bool` over the variant's
single payload, and a payload no guard accepts falls through to the next arm
for that variant, then to `unhandled = fatal`. A guarded source must be a
single-field tuple variant, guarded arms must precede the one unguarded arm
for the same variant, and a strict registration needs that unguarded arm as a
catch-all. A guarded plain arm forwards the source's code and level, and
`into`, `field` and `with` arms take a guard too.

```rust
use errlanes::{Fail, ResultExt, lanes};

#[derive(Debug)]
struct LimitExceeded { limit: &'static str }

#[derive(Debug, errlanes::Rejection)]
enum Limit {
    #[rejection(code = "LIMIT_EXCEEDED")]
    Exceeded(LimitExceeded),
}

fn is_overdraft(exceeded: &LimitExceeded) -> bool {
    exceeded.limit == "overdraft"
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Limit, unhandled = fatal)]
enum Checkout {
    #[lift(Limit::Exceeded, when = is_overdraft)]
    InsufficientFunds(LimitExceeded),
}

fn check(limit: &'static str) -> Result<(), Fail<Checkout, lanes!(Fatal)>> {
    Err::<(), _>(Limit::Exceeded(LimitExceeded { limit })).lift()
}

assert!(matches!(
    check("overdraft"),
    Err(Fail::Rejected(Checkout::InsufficientFunds(_)))
));
assert!(matches!(check("velocity"), Err(Fail::Fatal(_))));
```

A source case can reach into nested enums by wrapping each level in
parentheses: `#[lift(Outer::A(Mid::B(Leaf::C)))]`. The leaf takes every form
above, including `when`, and its payload is what the guard sees; every level
above the leaf must be a single-field tuple variant. A nested pattern never
covers the other variants of the intermediate enums, so nested arms require
`unhandled = fatal`.

```rust
use errlanes::Lift;

enum Batch { Apply(Apply), Other }
enum Apply { Velocity(Velocity), Locked }
enum Velocity { Exceeded(u8) }

fn is_floor(level: &u8) -> bool {
    *level < 10
}

#[derive(errlanes::Lift)]
#[lift(Batch, unhandled = fatal)]
enum Settlement {
    #[lift(Batch::Apply(Apply::Velocity(Velocity::Exceeded)), when = is_floor)]
    BelowFloor(u8),
}

let exceeded = Batch::Apply(Apply::Velocity(Velocity::Exceeded(3)));
assert!(matches!(Settlement::lift(exceeded), Ok(Settlement::BelowFloor(3))));
assert!(Settlement::lift(Batch::Apply(Apply::Locked)).is_err());
```

### One code, several causes

`#[rejection(code = "..", description = "..")]` on an enum is the code (and
description, and `level` if given) of every variant that declares neither its
own `code` nor `delegate`. A cause that the client cannot distinguish is a
variant under the family's code, not a wrapper around the foreign enum.
`#[error]`, `#[source]`, `#[rejection(from)]` and `#[lift]` work on each
variant as before, and a `#[lift]` arm takes the enum's code instead of
forwarding the source case's. The generated code type has one variant per
distinct code, and `CODES` lists each code once.

```rust
use errlanes::{Fail, Rejection, RejectionCode, ResultExt, lanes};

#[derive(Debug, errlanes::Rejection)]
enum Cala {
    #[rejection(code = "CALA_DUPLICATE_ID")]
    DuplicateId(u64),
    #[rejection(code = "CALA_CODE_ALREADY_EXISTS")]
    CodeAlreadyExists(String),
    #[rejection(code = "CALA_UNRELATED")]
    Unrelated,
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[rejection(
    code = "HOLDING_ACCOUNT_ALREADY_EXISTS",
    description = "A holding account with this id or reference already exists."
)]
#[lift(Cala, unhandled = fatal)]
enum CreateAccountRejection {
    #[error("holding account id {0} already exists")]
    #[lift(Cala::DuplicateId)]
    DuplicateId(u64),
    #[error("holding account code {0} already exists")]
    #[lift(Cala::CodeAlreadyExists)]
    CodeAlreadyExists(String),
}

assert_eq!(CreateAccountRejectionCode::CODES.len(), 1);

let duplicate: Result<(), Fail<CreateAccountRejection, lanes!(Fatal)>> =
    Err::<(), _>(Cala::CodeAlreadyExists("H1".into())).lift();
let rejected = duplicate.unwrap_err().rejected().unwrap();
assert_eq!(
    Into::<&str>::into(rejected.code()),
    "HOLDING_ACCOUNT_ALREADY_EXISTS"
);
assert_eq!(rejected.to_string(), "holding account code H1 already exists");
```

Variants that still declare their own `code`, or `delegate`, keep it, so a
family can report one code for most causes and a distinct one for a few. One
code has one description: a variant that takes the shared code cannot declare
its own `description`. An enum-level `code` is a compile error when every
variant declares its own code or delegates, when the enum has no variants, or
next to `code_prefix`.

```rust
use errlanes::{Rejection, RejectionCode};

#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "PARTNER_NOT_ACTIVE")]
#[error("the partner is not active")]
struct PartnerNotActive;

#[derive(Debug, errlanes::Rejection)]
#[rejection(
    code = "PARTNER_API_KEY_CREDENTIALS_REJECTED",
    description = "The presented API key credentials are not usable."
)]
enum ApiKeyCredentialsRejected {
    #[rejection(from)]
    #[error("the presented secret is not validly encoded")]
    InvalidEncoding(#[source] std::str::Utf8Error),
    #[rejection(from)]
    #[error("the presented secret has the wrong length")]
    InvalidLength(#[source] std::array::TryFromSliceError),
    #[error("no api key matches the presented secret")]
    Unknown,
    #[error("the api key is not usable")]
    Inactive,
    #[rejection(delegate, from)]
    #[error("{0}")]
    PartnerNotActive(PartnerNotActive),
}

assert_eq!(ApiKeyCredentialsRejectedCode::CODES.len(), 2);
assert_eq!(
    Into::<&str>::into(ApiKeyCredentialsRejected::Unknown.code()),
    "PARTNER_API_KEY_CREDENTIALS_REJECTED"
);
assert_eq!(
    Into::<&str>::into(ApiKeyCredentialsRejected::from(PartnerNotActive).code()),
    "PARTNER_NOT_ACTIVE"
);
```

## Errors from other crates

Every local error type says, through one trait, how it enters the lanes:

```rust,ignore
pub trait Classify: Error + Send + Sync + 'static {
    type Rejected: RejectedSlot; // a `Rejection`, or `Infallible` if nothing is ever rejected
    type Lanes: LaneProfile;     // the narrowest profile the fault part can occupy
    fn classify(self) -> Fail<Self::Rejected, Self::Lanes>;
}
```

A `Rejection` gets `Classify` from a blanket impl, so a pure domain outcome
needs only `#[derive(errlanes::Rejection)]`. Everything else is written with
`#[derive(errlanes::Classify)]`, and a type is one or the other, never both.

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

## At a `Box<dyn Error>` boundary

A `Classify` wrapper carries its lane in its
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
| `Result<T, Fail<R, L>>` | `Result<T, Fault<L>>`, recovering some members and narrowing the rest | `.narrow_rejected_with(\|r\| ..)` |
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
  `narrow_rejected_with`, `rejected` and `map_rejected` work on `Result<T, E>`. A narrowing returns the
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

Composition is transitive: a family that composes `Posting` is also `From`
everything `Posting` composes or strictly lifts, so a bare `Validation` reaches
an outer family without every layer re-declaring the hop. The composing family
never has to spell the deeper source's path; it asks the family it imports for
the type. `#[errlanes::compose(Posting(no_from))]` imports `Posting`'s cases
only. Reach for it when the outer family maps one of `Posting`'s sources
differently, and the compiler reports two `From` impls for the same source.

A source is identified by the path as the family that converts from it wrote it.
If two imported families reach one type under different spellings, they export it
twice and the outer family gets two identical impls (E0119). Either import one of
them with `(no_from)`, or register that source once on the outer family with
`#[lift(Source)]`, spelled the same way as one of them.

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

The lanes are represented by two carrier enums. Leaving lane selection aside
for a moment, their structure looks like this:

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

Lane order does not matter, and `lanes!()` with no names disables all fault
lanes. `lanes!` names only the three fault lanes; the Rejected lane is selected
by which carrier a function uses, `Fail` or `Fault`.

Matching a borrowed failure still requires an arm for every variant, so use an
accessor instead. `as_denied`, `as_transient`, and `as_fatal` are available
only when the profile enables that lane, `Fail` adds `as_rejected`, and
`lane()` works on any profile.

### Example: Authorization adds a lane

Consider an inner operation that can only fail fatally. An outer function
checks whether the subject is allowed to perform it before calling it. The
outer function can therefore return Denied as well as Fatal:

```rust
use errlanes::{Denied, Fault, WidenResult, lanes};

fn authorize(subject: &str) -> Result<(), Denied> {
    if subject == "admin" { Ok(()) } else { Err(Denied::default()) }
}

fn inner() -> Result<u64, Fault<lanes!(Fatal)>> {
    Ok(42)
}

fn outer(subject: &str) -> Result<u64, Fault<lanes!(Fatal, Denied)>> {
    authorize(subject)?;
    let value = inner().widen()?;
    Ok(value)
}

assert_eq!(outer("admin").unwrap(), 42);
assert!(matches!(outer("guest"), Err(Fault::Denied(_))));
```

The authorization failure enters the Denied lane through `?`. The inner
operation's result uses `.widen()?` to fit the outer function's larger set of
lanes. This method comes from `WidenResult`, and its destination is inferred
from the return type. An inner Fatal remains Fatal, with its kind, context and
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
came out. Widening alone cannot remove a lane; the caller must account for the
outcome. Because a narrowed profile needs somewhere to put an exhaustion,
`lanes!(Transient)` on its own is neither narrowable nor retryable.

The loop illustrates who handles Transient. In practice, that owner also
decides whether repeating the operation is safe and when to stop retrying.
Widening only ever adds lanes and keeps every value; narrowing removes one
lane and says what its value becomes.

### Transient, and the narrower question

`is_transient()` answers "is this unit of work safe to re-run, later, from the
outside?" — every `TransientKind` says yes, a lost connection or a pool
timeout as much as a deadlock. `is_contention()` answers a narrower one: "did
Postgres confirm this attempt lost a race?" — `Deadlock` (`40P01`) or
`SerializationFailure` (`40001`), and nothing else. Both predicates exist on
`TransientKind`, `Transient`, `Fault` and `Fail`.

The narrow one is for the two places the broad one is wrong. Retrying at
`COMMIT`: only a server-confirmed abort guarantees the transaction rolled
back; any other error there is ambiguous, since the server may have committed
before the client saw it. And a batch search deciding whether to split a
failing range: contention says nothing about the data, only about the
interleaving, so splitting cannot isolate anything and may provoke the same
cycle again — whereas an `OptimisticConflict` names one stale row and *is*
worth splitting for, which is why it is transient but not contention.

## Narrowing a lane

`widen` has a dual. Where `widen` adds lanes losslessly and never drops one,
`narrow_<lane>` removes exactly one lane and turns its value into a `Fatal` —
the only lane left standing once nobody can act on the removed one. There is
one narrowing per lane that can reach a boundary with nobody left to help it:

| narrowed lane | what the value is afterwards | method | `FatalKind` |
|---|---|---|---|
| `Transient` | a transient that ran out of retries | `narrow_transient(attempts)` | `Exhausted` |
| `Denied` | still a denial, fatal only because nobody can be told | `narrow_denied()` | `Denied` |
| `Rejected(D)` | a caller-correctable outcome with no caller, which is a bug | `narrow_rejected()` | `Invariant` |

Each `FatalKind` names what the value truly *is* afterwards, never a guessed
cause — the same rule `Exhausted` already followed before it had two
siblings. A narrowing is always a method call, never a `From`: a `?` that
silently dropped a lane is exactly what the widening rule already forbids, so
narrowing cannot happen by accident either.

## At a `Box<dyn Error>` boundary

`Fatal` and `Transient` store their source as `Arc<dyn Error + Send + Sync>`
so a `Fault` or `Fail` can cross a thread or task spawn — this is load-bearing
and not a bound to work around. A boundary that only has a plain `Box<dyn
Error>` (a trait object with no `Send`/`Sync` bound, e.g. from a caller whose
own trait does not require it) therefore cannot move that error into a lane
payload's source. `Fault::classify` is the one call for that case: it only
ever borrows the error, so no `Send`/`Sync` bound is needed to call it.

```rust
use errlanes::Fault;

let boxed: Box<dyn std::error::Error> = Box::new(std::io::Error::other("disk full"));
let fault: Fault = Fault::classify(&*boxed);
assert!(matches!(fault, Fault::Fatal(_)));
```

`Fault::classify` tries, in order: a lane payload anywhere in the error's
`source()` chain, carried through with its kind, context and (now cloned out,
so `Send + Sync` again) its own source intact; then, for each blessed foreign
type whose `classify-*` feature is enabled (`sqlx::Error`, `serde_json::Error`,
`reqwest::Error`, in that order), the first one found anywhere in the chain,
classified exactly as that feature's `impl Classify` would; then
`Fatal(Dependency)`, with the error's `Display` chain as its context, so an
error this function cannot otherwise classify still surfaces as something a
boundary pages on, never silently as nothing.

Call `Fault::classify` before the error crosses an `.await` or a spawn, not
after — a `Box<dyn Error>` with no `Send` bound on its trait object cannot be
held across either:

```rust,ignore
// Inside a boundary whose own trait returns `Box<dyn Error>`, not
// `Box<dyn Error + Send + Sync>`:
let fault: errlanes::Fault<errlanes::lanes!(Transient, Fatal)> = match runner.run(job).await {
    Ok(done) => return Ok(done),
    // `e` is dropped here. A job is not an authorization boundary — nobody
    // is on the other end of it to be told no — so a `Denied` anywhere in
    // the chain is narrowed away into `Fatal(Denied)` on the spot.
    Err(e) => errlanes::Fault::classify(&*e).narrow_denied(),
};
// `fault` is `Send + Sync` from here on, and can cross a spawn.
persist(fault).await?;
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

The enum variant lets Rust callers match the case. The code lets an API expose
a stable identity and lets telemetry group occurrences of the same outcome,
even if its human-readable message changes. Codes remain typed inside the
application and can be converted to strings at the API boundary. Never build
either from `Display`: a rejection's message may embed caller-supplied input.

The level tells the recording boundary how severely to log the rejection.
Info is the default because rejections are expected domain outcomes. A case
that needs different operational visibility can override it with, for example,
`#[rejection(code = "INVALID_AMOUNT", level = "warn")]`. Its lane is still Rejected.

### Fault wrappers and mixed wrappers

The other two shapes are written with `#[derive(errlanes::Classify)]` instead.
A fault wrapper names exactly one lane and never rejects — a struct wrapping a
foreign error is the common case:

```rust
use errlanes::{Classify, ClassifyResult, Fatal, FatalKind};

#[derive(Debug, thiserror::Error, errlanes::Classify)]
#[error("stored json did not decode: {0}")]
#[classify(fatal(CorruptState), from)]
struct Stored(#[source] std::io::Error);

fn decode() -> Result<u8, std::io::Error> { Err(std::io::Error::other("x")) }

fn hydrate() -> Result<u8, errlanes::Fault<errlanes::lanes!(Fatal)>> {
    let _ = decode().classify::<Stored>()?;
    Ok(0)
}

assert!(matches!(hydrate(), Err(errlanes::Fault::Fatal(f)) if f.kind == FatalKind::CorruptState));
```

`#[classify(fatal(CorruptState), from)]` gives `Stored` one static lane — every
value is `Fatal(CorruptState)`, with the whole `Stored` value as its source —
and `from` emits `impl From<std::io::Error> for Stored` for the single tuple
field. `.classify::<Stored>()` is how a one-off call site wraps a foreign
error without the enclosing function itself returning `Stored` (see "Errors
from other crates" below).

A mixed wrapper combines both: some variants `delegate` to a payload that is
itself `Classify` (a `Rejection`, a fault wrapper, or another mixed wrapper),
others name a static lane directly. `Rejected` and `Lanes` are inferred from
whichever variants are present — never named by hand:

```rust
use errlanes::{Classify, ClassifyResult, Fail};

#[derive(Debug, thiserror::Error, errlanes::Rejection)]
#[error("constraint violated: {0}")]
#[rejection(code = "CONSTRAINT")]
struct Constraint(&'static str);

#[derive(Debug, thiserror::Error, errlanes::Classify)]
enum DbWrite {
    #[error("constraint: {0}")]
    #[classify(delegate)]                      // Rejected = Constraint, inferred
    Constraint(Constraint),
    #[error("conflict: {0}")]
    #[classify(transient(OptimisticConflict))]  // Lanes include Transient, inferred
    Conflict(std::io::Error),
}

impl From<std::io::Error> for DbWrite {
    fn from(e: std::io::Error) -> Self {
        match e.kind() {
            std::io::ErrorKind::AlreadyExists => DbWrite::Constraint(Constraint("users_email_key")),
            _ => DbWrite::Conflict(e),
        }
    }
}

fn insert() -> Result<u8, std::io::Error> { Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists)) }

fn write() -> Result<u8, Fail<Constraint, errlanes::lanes!(Transient, Fatal)>> {
    let _ = insert().classify::<DbWrite>()?;
    Ok(0)
}

assert!(matches!(write(), Err(Fail::Rejected(Constraint("users_email_key")))));
```

### Rejections alongside faults

An operation can reject a request for a domain reason and also encounter a
fault while carrying it out. `Fail<R, L>` expresses both: `R` names the domain
rejections, and `L` selects the fault lanes.

For example, paying can reject an invalid amount before reaching storage,
while storage can fail fatally:

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

The validation case converts into `Fail::Rejected`. The storage function
returns a `Fault`, which `?` converts into the compatible `Fail` while
retaining its lane. A function that only validates input can return
`Result<T, Validation>` directly; it has no fault lanes to combine with its
rejections, and the caller's `?` places it in the Rejected lane.

## Errors from other crates

`sqlx::Error`, `serde_json::Error`, and anything else outside this crate never
implements `Classify` directly — errlanes cannot know what a `404` from one
caller's upstream means versus another's. The fix is the same one always: wrap
it in a local type, and give *that* a lane. A one-off call site does this with
`.classify::<W>()`, which turns a `Result<T, Foreign>` into a `Result<T, W>`:

```rust
use errlanes::{ClassifyResult, FatalKind};

#[derive(Debug, thiserror::Error, errlanes::Classify)]
#[error("stored json did not decode: {0}")]
#[classify(fatal(CorruptState), from)]
struct Stored(#[source] std::io::Error);

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
the derive.

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

## Rejections across domain boundaries

As an error moves up the call stack, it may cross into a domain with its own
rejection enum. A payment operation, for example, may need to expose a
validation rejection as one of its own cases. The fault lanes already have
shared meanings, but the two domain enums need an explicit conversion.

`derive(Lift)` generates that conversion. The enum-level `#[lift(Validation)]`
names the source, and an annotation on each destination variant says which
source case it represents:

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

fn validate(amount: u64) -> Result<(), Validation> {
    if amount == 0 { Err(Validation::InvalidAmount) } else { Ok(()) }
}

fn outer() -> Result<(), Fail<Payment, lanes!(Denied, Fatal)>> {
    validate(0).widen()?;
    Ok(())
}

let rejection = outer().unwrap_err().rejected().unwrap();
assert!(matches!(rejection, Payment::AmountNotPositive));
assert_eq!(Into::<&'static str>::into(rejection.code()), "INVALID_AMOUNT");
```

`.widen()?` converts the validation rejection into the payment rejection and
places it in the Rejected lane of the outer result. `WidenResult` accepts a
bare rejection, as here, or a `Fail` or `Fault` result; faults retain their
payloads, and successful values pass through unchanged.

By default, a lift must account for every source case. The derive generates
an exhaustive mapping and a `From` implementation. If Validation gains another
case, this mapping must be updated before it compiles. Unit, tuple and named
payloads forward automatically; `#[lift(Source::Variant, with = mapper)]`
handles a genuine payload transformation.

Conversion and metadata are separate concerns. `Lift` generates the conversion;
`Rejection` generates the code and level. Either derive can be used alone.
When both are present, a simple lift annotation also identifies where to get
the metadata. The renamed `AmountNotPositive` case therefore keeps the source
code `INVALID_AMOUNT` and its level. A destination can declare its own
`#[rejection(code = "...")]` when the mapping gives the error a new domain meaning.

Sometimes only a subset of the source cases makes sense as a rejection in the
destination domain. If the remaining cases would indicate a violated invariant,
`#[lift(Source, unhandled = fatal)]` allows a partial mapping. An unmapped
rejection becomes Fatal with the original rejection as its source, so the
destination must permit Fatal, which the compiler checks. Whether a mapping is
total or partial is declared once, on the destination enum; the call site is
the same `.widen()?` either way. A partial mapping does not generate `From`,
because there is no infallible conversion to be had.

A source need not be an enum. A struct rejection — the shape a wrapped foreign
error naturally takes (see "Errors from other crates" above) — lifts as a
whole value: `#[lift(Payload)]` with no variant suffix names the registered
struct directly, and the entire value becomes the one field of the
destination variant that also writes `#[lift(Payload)]`:

```rust
use errlanes::{Lift, Rejection};

#[derive(Debug, thiserror::Error, errlanes::Rejection)]
#[error("invalid payload")]
#[rejection(code = "INVALID_PAYLOAD")]
struct Payload(#[source] std::num::ParseIntError);

#[derive(Debug, thiserror::Error, errlanes::Rejection, errlanes::Lift)]
#[lift(Payload)]
enum JobRejection {
    #[error("invalid payload: {0}")]
    #[lift(Payload)]
    InvalidPayload(Payload),
}

let bad = "x".parse::<u32>().unwrap_err();
let rejection: JobRejection = Payload(bad).into();
assert!(matches!(rejection, JobRejection::InvalidPayload(_)));
```

## Moving between signatures

A failure changes shape as it travels: the lane set grows when a caller can
produce outcomes the callee could not, and the rejection type changes when a
failure crosses into a domain with its own vocabulary. Two operations cover
every case:

| From | To | Use |
|---|---|---|
| the same carrier | itself | `?` |
| a lane marker (`Transient`, `Fatal`, `Denied`) | any carrier enabling it | `?` |
| `Fault<S>` | `Fail<R, D>` | `?` |
| a bare rejection `C` | `Fail<R, D>`, given `R: From<C>` | `?` |
| a bare rejection `C` | `Fail<R, D>`, given a partial `#[lift(C)]` on `R` | `.widen()?` |
| `Fail<C, S>` | `Fail<R, D>` | `.widen()?` |
| `Fault<S>` | `Fault<D>` | `.widen()?` |
| `Result<T, Foreign>` | `Result<T, W>` | `.classify::<W>()`, given `W: Classify + From<Foreign>` |
| `W: Classify` | `Fail<D, M>` | `?`, given `D` lifts `W::Rejected` totally and `W::Lanes ⊆ M` |
| `W: Classify` | `Fail<D, M>` | `.widen()?`, if `D` only lifts `W::Rejected` partially |
| `W: Classify<Rejected = Infallible>` | `Fault<M>` | `?`, given `W::Lanes ⊆ M` |
| `W: Classify<Rejected = Infallible, Lanes = lanes!(Fatal)>` | bare `Fatal` | `?` (same for a lone `Transient`) |
| anything with a rejected part | `Fault` | `.map_err(Fail::narrow_rejected)` |
| `Fault<L>` | `Fault<WithoutTransient<L>>` | `.narrow_transient(attempts)` |
| `Fault<L>` | `Fault<WithoutDenied<L>>` | `.narrow_denied()` |
| `Fail<D, L>` | `Fault<L>` | `.narrow_rejected()` |

`?` handles anything that needs no decision. `.widen()` is for the one case that
does, changing the rejection type, and its destination is inferred from the
return type. A total `#[lift(C)]` supplies the `R: From<C>` that lets a bare
rejection propagate with `?`; a partial lift does not, so a bare rejection
crosses it with `.widen()?` like any other source. Widening only ever adds
lanes: dropping one that the source can still produce is a compile error,
because somebody has to handle it. A narrowing goes the other way on purpose:
each removes exactly one lane by name, never by inference, which is why it is
always a method call and not a `From` (see "Narrowing a lane" above).

## Composing rejection families

An outer domain may also want to expose every case from an inner rejection
family, including cases added in the future. In that situation, there is no
individual mapping decision to make for each case.
`#[errlanes::compose]` expresses that relationship directly:

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

The placeholder `Amount(Validation)` expands into real variants such as
`AmountInvalidAmount`. Its name supplies the prefix; the imported cases keep
their payloads, codes, levels, formatting, and sources. The attribute supplies
both `Rejection` and `Lift`, including total `From` conversions, so the
resulting family supports `.widen()?` just like the explicit mapping above,
and it can sit alongside explicit lifts from other sources.

Composition includes new source cases automatically. Explicit lifts are
useful when each addition needs review or individual cases need different
names. When an outer layer adds no semantics of its own, it can simply reuse
the inner rejection type.

---

The Rust snippets above are checked by the crate's doctests. The opening enum
sketch compiles, and the examples execute their assertions:

```sh
nix develop -c cargo test --profile mdbook-test -p errlanes --doc
```

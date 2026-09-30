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

## Faults and lane selection

A particular operation will usually produce only some of these outcomes.
For example, a storage function might return Transient or Fatal but have no
reason to deny access. Its signature can express that with
`Fault<lanes!(Transient, Fatal)>`.

The `lanes!` macro selects which fault lanes a type permits. That selection
tells callers what they need to handle. It can change as an error travels up
the call stack: a caller may introduce an additional outcome or take
responsibility for handling one.

### Authorization adds a lane

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
from the return type. An inner Fatal remains Fatal, with its source and context
intact.

### Retrying handles a lane

The reverse situation occurs when an inner operation can return Transient,
but its caller owns retrying the operation. After handling Transient, that
caller only needs to expose Fatal to its own callers:

```rust
use errlanes::{Fault, lanes};

fn execute(
    mut inner: impl FnMut() -> Result<u64, Fault<lanes!(Transient, Fatal)>>,
) -> Result<u64, Fault<lanes!(Fatal)>> {
    loop {
        match inner() {
            Ok(value) => return Ok(value),
            Err(Fault::Transient(_)) => continue,
            Err(Fault::Fatal(error)) => return Err(Fault::Fatal(error)),
            Err(Fault::Denied(never)) => match never {},
        }
    }
}

let mut attempts = 0;
let result = execute(|| {
    attempts += 1;
    if attempts == 1 {
        Err(errlanes::Transient::new(errlanes::TransientKind::Deadlock).into())
    } else {
        Ok(42)
    }
});
assert_eq!(result.unwrap(), 42);
assert_eq!(attempts, 2);
assert!(matches!(
    execute(|| Err(errlanes::Fatal::invariant("broken state").into())),
    Err(Fault::Fatal(_))
));
```


Here, `execute` takes the inner operation as a closure so it can call it again.
Its return type omits Transient because the loop handles that case. Widening
alone cannot remove a lane; the caller must account for the outcome.

The Denied arm needs a little explanation. Disabled lanes still appear as enum
variants, but their payload is `Infallible`. The expression `match never {}`
tells Rust that there can be no value in that arm. Lane order does not matter,
and `lanes!()` with no names disables all fault lanes.

The loop illustrates who handles Transient. In practice, that owner also
decides whether repeating the operation is safe and when to stop retrying.
The optional `tokio` feature provides `retry` and `retry_with` with retry
budgets and delays.

## Domain rejections

Some errors require more specific handling than retrying, denying access, or
reporting a fatal failure. A payment amount might be invalid, or an email
address might already be registered. The caller may need to handle the case
directly or communicate it to an end user who can correct the request.

These cases belong in the Rejected lane. An application-defined enum describes
the individual outcomes. `derive(Rejection)` gives each outcome a stable
`code()` and a recording `level()`:

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
application and can be converted to strings at the API boundary.

The level tells the recording boundary how severely to log the rejection.
Info is the default because rejections are expected domain outcomes. A case
that needs different operational visibility can override it with, for example,
`#[rejection(code = "INVALID_AMOUNT", level = "warn")]`. Its lane is still Rejected.

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
        return Err(Fail::Rejected(Validation::InvalidAmount));
    }
    save()?;
    Ok(())
}

assert!(matches!(pay(0), Err(Fail::Rejected(Validation::InvalidAmount))));
assert!(pay(1).is_ok());
```


`Fail::Rejected` carries the validation case. The storage function returns a
`Fault`, which `?` converts into the compatible `Fail` while retaining its
lane. A function that only validates input can return `Result<T, Validation>`
directly; it has no fault lanes to combine with its rejections.

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

fn inner() -> Result<(), Fail<Validation, lanes!(Fatal)>> {
    Err(Fail::Rejected(Validation::InvalidAmount))
}

fn outer() -> Result<(), Fail<Payment, lanes!(Denied, Fatal)>> {
    inner().widen()?;
    Ok(())
}

let error = outer().unwrap_err().rejected().unwrap();
assert!(matches!(error, Payment::AmountNotPositive));
assert_eq!(Into::<&'static str>::into(error.code()), "INVALID_AMOUNT");
```


`WidenResult` works on `Fail` results as well as `Fault` results. In this
example, `.widen()?` converts the validation rejection into the payment
rejection and permits Denied in the result. Faults retain their payloads, and
successful values pass through unchanged.

By default, a lift must account for every source case. The derive generates
an exhaustive mapping and a `From` implementation. If Validation gains another
case, this mapping must be updated before it compiles. A bare Validation value
can also convert through `Payment::from(value)` or propagate through `?` into
a compatible `Fail`.

Conversion and metadata are separate concerns. `Lift` generates the conversion;
`Rejection` generates the code and level. Either derive can be used alone.
When both are present, a simple lift annotation also identifies where to get
the metadata. The renamed `AmountNotPositive` case therefore keeps the source
code `INVALID_AMOUNT` and its level. A destination can declare its own
`#[rejection(code = "...")]` when the mapping gives the error a new domain meaning.

Sometimes only a subset of the source cases makes sense as a rejection in the
destination domain. If the remaining cases would indicate a violated invariant,
`#[lift(Source, unhandled = fatal)]` allows a partial mapping. Callers then use
`.lift()?` from `LiftResult`. An unmapped rejection becomes Fatal with the
original rejection as its source, so the destination must permit Fatal. A
partial mapping does not generate `From`.

The [reference](https://github.com/GaloyMoney/es-entity/blob/main/errlanes/REFERENCE.md)
covers mapping options, metadata rules, feature integrations, and migration.

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


The placeholder `Amount(Validation)` expands into variants such as
`AmountInvalidAmount`. Its name supplies the prefix; the imported cases keep
their payloads, codes, levels, formatting, and sources. The attribute supplies
both `Rejection` and `Lift`, including total `From` conversions, so the
resulting family supports `.widen()?` just like the explicit mapping above.

Composition includes new source cases automatically. Explicit strict lifts
are useful when each addition needs review or individual cases need different
names. When an outer layer adds no semantics of its own, it can simply reuse
the inner rejection type.

---

The Rust snippets above are checked by the crate's doctests. The opening enum
sketch compiles, and the examples execute their assertions:

```sh
nix develop -c cargo test --profile mdbook-test -p errlanes --doc
```

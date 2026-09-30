# errlanes

Errors need different responses. **Rejected** means a caller-correctable domain
outcome, **Denied** means authorization failed, **Transient** means retrying may
succeed, and **Fatal** means operator attention is needed. These are the four
*lanes*: classify an error where its meaning is known, then preserve its lane
and source as it travels through the application.

## Start with fault lanes

`Fault<lanes!(Transient, Fatal)>` describes an operation that can fail transiently
or fatally. It cannot return Denied or Rejected. The type lists the outcomes its
caller must handle; `lanes!()` with no names enables no fault lanes.

Why be explicit? A boundary can **handle a lane**, removing it from its return
type, or **introduce a lane**, adding an outcome its inner operation cannot
produce.

### Handle Transient at a retry boundary

The inner operation can return Transient or Fatal. Once `execute` handles
Transient by retrying, its caller only needs to handle Fatal:

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

The disabled Denied slot contains `Infallible`; `match never {}` proves it
cannot occur. Removing a lane requires handling it: widening cannot discard it.

This small loop shows the type change. A production retry boundary also owns
the retry budget, delay, and decision that repeating the operation is safe.
The optional `tokio` feature provides `retry` and `retry_with` for bounded retries.

### Add Denied at an authorization boundary

An inner operation may only fail fatally, while its caller also checks access.
The outer signature makes that additional outcome visible:

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

`WidenResult` adds `.widen()` to the result. The destination is inferred from
`outer`'s return type. A Fatal stays Fatal with the same source and context;
widening only changes which lanes the type permits. Lane order does not matter.

## Give domain rejections an identity

Some failures need a domain-specific response: an invalid amount can be
corrected, and an existing email address can be reported to the user. Callers
need to match these cases explicitly instead of treating them as Fatal.

An enum names the cases. `derive(Rejection)` adds two pieces of metadata:
`code()` gives an API or telemetry boundary a stable identity without parsing
the error message; `level()` tells the recording boundary how severely to log
it. Rejections default to Info, with overrides available when a case deserves
different operational visibility.

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

The code is typed internally and converted to a string at the API boundary.
To override severity, use e.g. `#[rejection(code = "INVALID_AMOUNT", level = "warn")]`.
Changing code or level does not change the Rejected lane.

### Combine rejection and fault lanes

`Fail<R, L>` adds a Rejected lane carrying `R` alongside the fault lanes in
`L`. For example, validation can reject an amount while storage can fail
fatally:

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

A compatible `Fault` enters `Fail` through `?`. Pure validation can simply
return `Result<T, Validation>`; it does not need a carrier when there are no
faults to represent.

## Propagate domain rejections with Lift

An outer domain often has its own rejection enum. To propagate an inner
rejection, it needs a conversion into that enum.

`derive(Lift)` generates that conversion. `#[lift(Source)]` declares the source
family, and each `#[lift(Source::Variant)]` maps one case. By default every
source case must be mapped: adding a source case forces the outer domain to
review its mapping.

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

The same `WidenResult` trait works with `Fail`: it converts the rejection
through `From` and can add fault lanes at the same time. Fault payloads and
successful values pass through unchanged. A bare `Validation` can also convert
with `Payment::from(value)`, or propagate through `?` into a compatible `Fail`.

The derives have separate jobs: `Lift` generates conversions; `Rejection`
generates metadata. Either works independently. When both are present, a simple
lift mapping also tells `Rejection` to forward the source's code and level by
default. Renaming a case therefore preserves its public identity. An explicit
`#[rejection(code = "...")]` selects a new local identity.

If only some source cases are legitimate domain rejections, use
`#[lift(Source, unhandled = fatal)]` and `.lift()?` from `LiftResult` instead.
Unmapped cases become Fatal invariants with the original rejection as their
source, so the destination must enable Fatal. This partial mapping does not
generate `From`. See the [reference](https://github.com/GaloyMoney/es-entity/blob/main/errlanes/REFERENCE.md)
for mapping options, metadata rules, feature integrations, and migration notes.

## Compose an entire rejection family

Sometimes every inner rejection should remain available at the outer boundary.
Repeating one lift mapping per case adds no domain decision.
`#[errlanes::compose]` imports the whole family:

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

The placeholder `Amount(Validation)` becomes real prefixed variants such as
`AmountInvalidAmount`. Payloads, codes, levels, formatting, and sources are
preserved. `compose` supplies both `Rejection` and `Lift`, including total
`From` conversions, so the resulting family also supports `.widen()?`.

Whole-family composition automatically includes future source cases. Use
explicit strict lifts when additions must force review or names need individual
choices. If the outer layer adds no semantics, it can reuse the inner type.

---

Every Rust snippet above is compiled and executed by the crate's doctests:

```sh
nix develop -c cargo test --profile mdbook-test -p errlanes --doc
```

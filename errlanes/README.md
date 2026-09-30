# errlanes

Classify an error where its meaning is known, preserve that classification and its
source, and record it at the owning boundary. The vocabulary is closed:
`Rejected(R)` is caller-correctable, `Denied` is authorization failure,
`Transient` may succeed on retry, and `Fatal` requires operator attention.

## Choose the lanes in the type

```rust
use errlanes::{Fail, Fault, lanes};

#[derive(Debug, thiserror::Error, errlanes::Rejection)]
enum Validation {
    #[error("invalid amount")]
    InvalidAmount,
}
type Infra = lanes!(Transient, Fatal);
type WriteError = Fail<Validation, Infra>;
type ReadError = Fault<Infra>;
type ValidationError = Fail<Validation, lanes!(Fatal)>;
```

`R` selects the rejected payload; `L` selects the additional fault lanes.
`Fail<R>` and `Fault` default to all three fault lanes for compatibility.
`RepoLanes` is `lanes!(Transient, Fatal)`. Pure validation can return
`Result<T, R>` without a carrier. Lane order is immaterial; `lanes!()` enables
no fault lanes. Profiles are sealed: applications cannot replace a standard
lane's payload or register arbitrary lanes.

Disabled slots contain `Infallible`. Rust still exposes their variant names,
including in borrowed exhaustive matches:

```rust
use errlanes::{Fault, Lane, lanes};
fn lane(error: &Fault<lanes!(Fatal)>) -> Lane {
    match error {
        Fault::Fatal(_) => Lane::Fatal,
        Fault::Denied(never) | Fault::Transient(never) => match *never {},
    }
}
```

## Propagate and widen

Use `?` for the same failure type, a compatible `Fault` entering a `Fail`, or a
bare rejection with a total conversion into the destination rejection. Use
`ResultExt::widen()` when changing the rejection or profile inside a `Fail`:

```rust
use errlanes::{Fail, ResultExt, lanes};
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
pub enum Child {
    #[error("limit {0}")]
    Limit(u64),
}
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
#[lift(Child)]
pub enum Parent {
    #[error("limit {0}")]
    #[lift(Child::Limit)]
    LimitExceeded(u64),
}
fn bare() -> Result<(), Fail<Parent, lanes!(Fatal)>> {
    Err::<(), _>(Child::Limit(42))?;
    Ok(())
}
fn widened() -> Result<(), Fail<Parent, lanes!(Transient, Fatal)>> {
    let child: Result<(), Fail<Child, lanes!(Fatal)>> = Err(Fail::Rejected(Child::Limit(42)));
    child.widen()?;
    Ok(())
}
```

The target is inferred from the return type. Value-level `Fail::widen`,
`Fail::lift`, and `Fault::widen` are also available. Successes pass through
unchanged. Widening preserves markers, sources, context, and retry hints.
It is a compile error to drop an enabled source lane; in particular, there is
no `Denied -> Fatal` adaptation. A blanket `From<Fail<C>> for Fail<P>` would
conflict with Rust's identity conversion, so it deliberately does not exist.

## Strict and partial lifting

`Lift<Source>` consumes a source and returns `Result<Self, Self::Unmapped>`.
Enum-level `#[lift(Source)]` defaults to strict. Qualify every mapped variant
with its source family. Unit, tuple, and named fields forward automatically.
The generated exhaustive match and `From<Source>` make omissions, unknown
variants, wrong field types/shapes, and duplicate mappings compile errors.
Multiple source families may target one destination.

For a domain boundary that considers unaccepted repository cases invariant
failures, use `#[lift(Source, unhandled = fatal)]` and `.lift()?`. Partial
lifting returns the original unmapped rejection to the adapter, which wraps
it in `Fatal(Invariant)` with that rejection as its source. It does not generate
`From<Source>` for the domain rejection. The destination must accept Fatal and
all source fault lanes. Strict lifting works without Fatal, including `.lift()`.

For a genuine payload transformation, `#[lift(Source::Variant, with = mapper)]`
calls `mapper` with the selected **whole source enum** and expects the destination
rejection. Specify `#[rejection(code = "DOMAIN_CODE")]` for this reinterpretation.
A destination variant accepting multiple source cases must likewise explicitly
choose its canonical code. Ordinary forwarding requires neither a mapper nor
a new code.

## Compose rejection families

```rust
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
pub enum Velocity {
    #[error("limit {0}")]
    #[rejection(code = "VELOCITY_LIMIT", level = "warn")]
    Limit(u64),
}
#[errlanes::rejection]
#[derive(Debug, thiserror::Error)]
pub enum Posting {
    #[flatten(prefix = "Velocity")]
    Velocity(Velocity),
    #[error("batch too large")]
    BatchTooLarge,
}
let posting: Posting = Velocity::Limit(42).into();
assert!(matches!(posting, Posting::VelocityLimit(42)));
```

The attribute runs before derives and replaces the placeholder with real
variants. There is no leftover `Velocity(Velocity)` fallback. It generates the
same exhaustive `Lift` / `From` conversion as strict explicit mapping. A whole
family import intentionally picks up future source cases; use explicit strict
mapping when additions must force human review.

Use `#[flatten(prefix = "Velocity", rename(Limit = LimitExceeded))]` to rename
individual cases; explicit names override the prefix. Name collisions are errors.
Importing the same leaf through multiple composition paths is rejected. Narrow
those source families or write explicit mappings to one canonical destination.
A layer with no additional semantics should reuse the child type or an alias.

Each derived family exports a companion macro named `FamilySchema` beside
`Family`. Reexport both when reexporting or renaming a source:

```rust,ignore
pub use implementation::{Family as PublicFamily, FamilySchema as PublicFamilySchema};
```

The source provides field types through associated-type projections and metadata
through borrowed field tuples. Consumers do not scan source files or reconstruct
errors. Module-qualified paths, renamed dependencies (including errlanes itself),
reexports, and transitive imports are tested across real crates. Enabled source
variants are exported after source-side `cfg` processing. The supported families
are monomorphic enums with unit, tuple, or named fields; fields can themselves
contain generic concrete types. Generic rejection enums and composing through
an arbitrary type alias are unsupported: use a concrete enum or reexport the
actual enum together with its schema macro. Formatting attributes travel with
variants; field formatting should use field expressions or fully qualified paths,
not helper names imported only in the source module.

`Rejection::Code` remains typed; convert it to a string at the wire. Forwarding
preserves the source code and severity despite prefixes or intermediate type
names. Local leaves default to an uppercase variant code and `Info` severity;
`code_prefix`, per-variant `code`, and `level` customize them. `Code::ALL` lists
local static leaf codes; delegated/forwarded codes remain typed in their source
code family.

`#[from]` is thiserror's conversion/source annotation. It does **not** imply that
a payload implements `Rejection`. To intentionally delegate code and severity
through a single-field wrapper, add `#[rejection(delegate)]`; this conflicts with
leaf code/level overrides. Composition preserves source annotations but does not
repeat leaf `#[from]` conversions: it generates the family conversion instead.

## Retry, settlement, and recording

The `tokio` feature provides `retry` and `retry_with`. Retry only transient
outcomes, at the boundary that owns a safe retry. Classification alone does not
prove an ambiguous commit is safe to repeat.

`Settled<R, L>` / `SettledFault<L>` have no live transient lane. `Exhausted` is
inhabited only if the input profile enabled Transient and reports as Fatal.
Converting that settled value back into a carrier requires Fatal at the
destination. A no-transient profile does not acquire an exhaustion case.

The `tracing` feature provides `record_fail`, `record_fault`, `record`, and
`record_settled_fault`; declare the fields in `FIELDS` on the owning span.
Rejection codes and severity come from `Rejection`, never from parsing Display.
Transient records are Info, denied records Warn, and fatal/exhausted records
Error. Only fatal/exhausted records include operator-facing exception messages.
Avoid logging the same failure at every propagation layer.

`lane_of` / `transient_of` walk sources looking for standard markers, including
through boxed subset carriers. They do not reflect arbitrary erased rejection
payloads. Wrappers must expose a source chain. Fatal sources are for diagnosis
and tests; production code handles the Fatal lane uniformly.

The `sqlx` feature supplies the central SQL classifier. It requires Transient
and Fatal, never Denied. Repositories handle known constraints before invoking
that classifier; unknown constraints are invariants. SQLx `Protocol` remains
Fatal(Dependency), not a retryable connection loss.

## Compatibility

The old `Liftable` discriminator helpers and `#[rejection(lift(...))]` grammar
remain compatibility adapters; new APIs use the `#[lift(Source::Variant)]`
grammar above. `Failure` now has an associated `Lanes` profile. The legacy
`Failure` newtype derive remains available for all-lanes wrappers; canonical
module APIs should use `Fail<R, L>` / `Fault<L>` directly. `Classify` remains the
adapter for legacy heterogeneous errors.

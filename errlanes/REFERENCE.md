# errlanes reference

Start with the [README](https://github.com/GaloyMoney/es-entity/blob/main/errlanes/README.md)
for a gradual introduction. This page covers detailed rules and migration.

## Lane profiles

`R` selects the rejected payload; `L` selects the additional fault lanes.
`Fail<R>` and `Fault` default to all three fault lanes for compatibility.
Pure validation can return `Result<T, R>` without a carrier. Lane order is
immaterial; `lanes!()` enables no fault lanes. Profiles are sealed: applications
cannot replace a standard lane's payload or register arbitrary lanes.

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

## Result conversions

Import `WidenResult` for `.widen()` on either `Result<T, Fault<L>>` or
`Result<T, Fail<R, L>>`. It widens Fault to Fault or Fail to Fail; the latter
also converts the rejection through `From`. Import `LiftResult` for `.lift()`
on a Fail result when using strict or partial rejection mappings.
These replace the former combined `ResultExt` trait. Destination types are
inferred from context (or supplied in a result type annotation).

Use `?` for an unchanged carrier, a compatible Fault entering a Fail, or a
bare rejection with a total conversion into the destination rejection.
Value-level `Fault::widen`, `Fail::widen`, and `Fail::lift` are also available.
All source fault lanes must be allowed by the destination. Markers, sources,
context, and retry hints survive widening; successes pass through unchanged.

## Strict and partial lifting

Derive `errlanes::Lift` for conversions and `errlanes::Rejection` for codes and
levels. Either derive can be used alone. `Lift` works on ordinary enums without
requiring `Error` or `Rejection`; it never generates rejection metadata.
`Rejection` never generates `Lift` or `From` implementations.

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

With both derives, a simple `#[lift(Source::Variant)]` also supplies the default
code and level to `Rejection`: renaming a destination variant preserves the
source's identity and severity. The source must derive `Rejection` (or implement
its metadata protocol). Explicit `#[rejection(code = "LOCAL", level = "info")]`
selects local metadata instead; specifying only a code uses the default Info
level, and specifying only a level uses the destination's default variant code.
There is no need to repeat the source in a second annotation.

To forward metadata independently of conversion, use
`#[rejection(forward = Source::Variant)]`. The destination fields must match the
source case. This forwards code and level only, and conflicts with `code`,
`level`, or `delegate` on the same variant.

## Compose rejection families

```rust
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
pub enum Velocity {
    #[error("limit {0}")]
    #[rejection(code = "VELOCITY_LIMIT", level = "warn")]
    Limit(u64),
}
#[errlanes::compose]
#[derive(Debug, thiserror::Error)]
pub enum Posting {
    #[compose(flatten)]
    Velocity(Velocity),
    #[error("batch too large")]
    BatchTooLarge,
}
let posting: Posting = Velocity::Limit(42).into();
assert!(matches!(posting, Posting::VelocityLimit(42)));
```

The attribute runs before derives and replaces the placeholder with real
variants. There is no leftover `Velocity(Velocity)` fallback. `compose` supplies
both `Rejection` and `Lift`, leaving `Debug`, `Error`, and unrelated derives to
the caller. Redundant `Rejection`/`Lift` entries in ordinary derive lists are
deduplicated. Standalone derives remain independent.

`#[compose(flatten)] Wrapper(Source)` prefixes every imported case with
`Wrapper`. It preserves leaf codes, levels, formatting, payloads, and sources,
and generates exhaustive `Lift` / `From` conversions. A whole-family import
intentionally picks up future source cases; use explicit strict mapping when
additions must force human review or cases need custom/unprefixed names.
There are no `prefix` or `rename` arguments. Name collisions are errors.
Importing the same leaf through multiple composition paths is rejected. Narrow
those source families or write explicit mappings to one canonical destination.
A layer with no additional semantics should reuse the child type or an alias.

Whole-family imports and explicit lifts from other sources can coexist:

```rust,ignore
#[errlanes::compose]
#[derive(Debug, thiserror::Error)]
#[lift(AccountConstraintViolation, unhandled = fatal)]
pub enum OperationRejection {
    #[compose(flatten)]
    Velocity(VelocityEnforcementRejection),

    #[lift(AccountConstraintViolation::CodeKey)]
    #[rejection(code = "ACCOUNT_CODE_ALREADY_EXISTS")]
    #[error("account code already exists: {0}")]
    AccountCodeAlreadyExists(#[source] ConstraintConflict<String>),
}
```

Velocity converts totally and supports `.widen()?`; the partial repository
mapping uses `.lift()?`, preserving unaccepted cases as the source of
Fatal(Invariant). `compose` also supports enums with only explicit lifts.
Do not import a whole family and explicitly map that same source again.

Migration: replace the old enum attribute `#[errlanes::rejection]` with
`#[errlanes::compose]` and remove explicit `Rejection`/`Lift` derives. Replace
`#[flatten(prefix = "X")] Placeholder(Source)` with
`#[compose(flatten)] X(Source)` to retain public names. For old unprefixed or
renamed cases, write explicit strict lifts to preserve names, or deliberately
update public match paths. The old composition spelling is no longer exported.

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

The optional `sqlx` feature supplies the SQL classifier and conversions from
`sqlx::Error`. It is off by default; without it, errlanes has no SQLx dependency.
The classifier returns `Fault<lanes!(Transient, Fatal)>`; conversions require
Transient and Fatal in the destination. Repositories handle known constraints
before invoking that classifier; unknown constraints are invariants. SQLx
`Protocol` remains Fatal(Dependency), not a retryable connection loss.

## Compatibility

The old `Liftable` runtime helpers remain available, but the combined
`#[rejection(lift(...))]` / `key` / `via` conversion grammar is no longer accepted.
Use `#[derive(errlanes::Lift)]` with `#[lift(Source)]` and qualified source cases.
Existing v2 mappings also need the separate `Lift` derive; their default code
and level forwarding is unchanged. `Failure` has an associated `Lanes` profile. The legacy
`Failure` newtype derive remains available for all-lanes wrappers; canonical
module APIs should use `Fail<R, L>` / `Fault<L>` directly. `Classify` remains the
adapter for legacy heterogeneous errors.

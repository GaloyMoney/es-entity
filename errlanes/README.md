# errlanes

Four error lanes, carried in the type instead of re-derived at every layer.

| Lane | Retry | Operator level | User-facing | Who produces it |
|---|---|---|---|---|
| `Rejected(D)` | never | `D::level()` (default info) | yes — `D` is safe to show | domain code |
| `Denied` | never | warn | `FORBIDDEN` | authorization checks |
| `Transient` | yes, per policy | info per attempt, error once exhausted | no (never seen unless exhausted) | infra classifiers (sqlx, es-entity OCC) |
| `Fatal` | no | error | no — redacted, keep a trace id | bugs, corrupt state, misconfiguration |

`Fail<D>` is the generic view before retries have run. `Settled<D>` is the
view after: it has no `Transient` arm, so a `match` that forgets to handle
exhaustion does not compile.

## Classify at birth, carry forever, record once

The code that turns a `sqlx::Error` into "optimistic conflict" is the only
code that can. After that point the lane travels unchanged through every
`?` — a `Transient` three crates up is still `Fail::Transient`. Record it
exactly once, at the boundary that disposes of the error (a job finalizer,
a request handler), with [`record`]/[`record_fail`].

```rust
use errlanes::lane_of;

fn is_worth_retrying(e: &(dyn std::error::Error + 'static)) -> bool {
    lane_of(e) == Some(errlanes::Lane::Transient)
}
```

## Adoption tiers

Only es-entity and `errlanes` are mandatory together; everything above that
adopts at its own pace.

**Tier 0 — opaque.** Wrap `Fail<{Entity}ConstraintViolation>` with a plain
`#[from]` or `?` into `Box<dyn Error>`/`anyhow::Error`. Zero lanes
knowledge required.

```rust,ignore
#[derive(Debug, thiserror::Error)]
enum MyError {
    #[error(transparent)]
    Repo(#[from] errlanes::Fail<UserConstraintViolation>),
}
```

**Tier 1 — lane-shaped enum.** A hand-written `thiserror` enum with
`Rejected`/`Transient`/`Fatal` variants and a short `From<Fail<X>>`.

```rust,ignore
#[derive(Debug, thiserror::Error)]
enum MyError {
    #[error(transparent)]
    Domain(#[from] MyRejection),
    #[error(transparent)]
    Transient(#[from] errlanes::Transient),
    #[error(transparent)]
    Fatal(#[from] errlanes::Fatal),
}
```

**Tier 2 — derived carrier.** `#[derive(errlanes::Rejection)]` on the
domain enum and `#[derive(errlanes::Failure)]` on a newtype wrapping
`Fail<D>` generate every conversion above, plus constraint lifting:

```rust,ignore
#[derive(Debug, thiserror::Error, errlanes::Rejection)]
#[rejection(repo = UserConstraintViolation)]
enum UserRejection {
    #[error("email already in use")]
    #[rejection(constraint = UserConstraint::EmailKey)]
    EmailTaken,
    #[error(transparent)]
    Repo(#[from] UserConstraintViolation),
}

#[derive(Debug, Clone, errlanes::Failure)]
#[failure(repo(UserConstraintViolation))]
struct UserError(errlanes::Fail<UserRejection>);
```

An unlisted constraint violation demotes to `Fatal(Invariant)` with the
constraint name in `context` — a constraint the domain did not anticipate
is a bug, not a rejection to show a client.

## Why not a generic `Fail<Local>` carrier everywhere

A downstream crate cannot `impl From<Fail<Upstream>> for Fail<Local>` —
neither type is local (E0117), and a blanket impl in this crate would
overlap `From<T> for T`. Every conversion the derives emit is therefore
concrete: `errlanes/tests/coherence.rs` pins this with a compile-fail test
so nobody "simplifies" it back into a blanket impl that will not compile
for a third crate.

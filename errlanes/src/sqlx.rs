//! Classifies a raw `sqlx::Error` into a lane, exactly once, at the point it
//! is born.
//!
//! `sqlx::Error::Protocol(_)` is deliberately mapped to `Fatal(Dependency)`
//! rather than treated as connection loss. es-entity itself constructs
//! `Protocol(..)` errors to smuggle domain conditions through `sqlx::Error`
//! at five call sites (`src/operation/hooks.rs:631,712`,
//! `src/operation/batch/mod.rs:162`, `src/operation/savepoint.rs:49,384`);
//! a real dropped connection surfaces as `Io`, not `Protocol`. Treating
//! `Protocol` as transient would make those synthesized errors retry
//! silently instead of surfacing as the bug they are.

use crate::{
    fail::{Fail, Fault},
    lane::{Fatal, FatalKind, Transient, TransientKind},
};

pub fn transient_sqlstate(code: &str) -> Option<TransientKind> {
    match code {
        "40001" => Some(TransientKind::SerializationFailure),
        "40P01" => Some(TransientKind::Deadlock),
        "57P01" | "57P02" | "57P03" | "08000" | "08003" | "08006" | "08001" | "08004" => {
            Some(TransientKind::ConnectionLost)
        }
        _ => None,
    }
}

/// Classifies a raw `sqlx::Error`, moving it. See the module docs for why
/// `Protocol(_)` lands in `Fatal(Dependency)`. A `sqlx::Error` never carries
/// a domain rejection, so the real work happens in [`classify_sqlx_fault`];
/// this widens that into whatever `D` the caller needs.
pub fn classify_sqlx<D, L: crate::LaneProfile<Transient = Transient, Fatal = Fatal>>(
    e: ::sqlx::Error,
) -> Fail<D, L> {
    classify_sqlx_fault(e).into()
}

/// Same classification as [`classify_sqlx`], as a [`Fault`] rather than a
/// `Fail<D>` — what `impl From<sqlx::Error> for Fault` delegates to.
pub fn classify_sqlx_fault(e: ::sqlx::Error) -> Fault<crate::lanes!(Transient, Fatal)> {
    match e {
        ::sqlx::Error::PoolTimedOut => Transient::new(TransientKind::PoolTimeout).into(),
        ::sqlx::Error::Io(err) => Transient::new(TransientKind::ConnectionLost)
            .with_source(err)
            .into(),
        ::sqlx::Error::Tls(err) => Transient::new(TransientKind::ConnectionLost)
            .with_source_boxed(err)
            .into(),
        ::sqlx::Error::PoolClosed => Transient::new(TransientKind::ConnectionLost).into(),
        ::sqlx::Error::WorkerCrashed => Transient::new(TransientKind::ConnectionLost).into(),
        ::sqlx::Error::Database(db) => {
            if let Some(kind) = db.code().and_then(|c| transient_sqlstate(&c)) {
                return Transient::new(kind)
                    .with_source(::sqlx::Error::Database(db))
                    .into();
            }
            if db.is_unique_violation() || db.is_foreign_key_violation() || db.is_check_violation()
            {
                let name = db.constraint().map(str::to_string);
                let mut fatal =
                    Fatal::from_error(FatalKind::Invariant, ::sqlx::Error::Database(db));
                if let Some(name) = name {
                    fatal = fatal.with_context(name);
                }
                return fatal.into();
            }
            Fatal::from_error(FatalKind::Config, ::sqlx::Error::Database(db)).into()
        }
        ::sqlx::Error::RowNotFound
        | ::sqlx::Error::ColumnNotFound(_)
        | ::sqlx::Error::ColumnIndexOutOfBounds { .. }
        | ::sqlx::Error::ColumnDecode { .. }
        | ::sqlx::Error::Decode(_)
        | ::sqlx::Error::Encode(_)
        | ::sqlx::Error::TypeNotFound { .. } => {
            Fatal::from_error(FatalKind::CorruptState, e).into()
        }
        ::sqlx::Error::Configuration(_) | ::sqlx::Error::AnyDriverError(_) => {
            Fatal::from_error(FatalKind::Config, e).into()
        }
        ::sqlx::Error::Protocol(_) => Fatal::from_error(FatalKind::Dependency, e).into(),
        other => Fatal::from_error(FatalKind::Dependency, other).into(),
    }
}

impl<D, L: crate::LaneProfile<Transient = Transient, Fatal = Fatal>> From<::sqlx::Error>
    for Fail<D, L>
{
    fn from(e: ::sqlx::Error) -> Self {
        classify_sqlx(e)
    }
}

impl<L: crate::LaneProfile<Transient = Transient, Fatal = Fatal>> From<::sqlx::Error> for Fault<L> {
    fn from(e: ::sqlx::Error) -> Self {
        classify_sqlx_fault(e).widen()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lane::Lane;

    #[test]
    fn pool_timed_out_is_transient_pool_timeout() {
        let f = classify_sqlx_fault(::sqlx::Error::PoolTimedOut);
        assert_eq!(f.lane(), Lane::Transient);
    }

    #[test]
    fn row_not_found_is_fatal_corrupt_state() {
        let f = classify_sqlx_fault(::sqlx::Error::RowNotFound);
        match f {
            Fault::Fatal(fatal) => assert_eq!(fatal.kind, FatalKind::CorruptState),
            other => panic!("expected Fatal, got {other:?}"),
        }
    }

    #[test]
    fn io_error_is_transient_connection_lost() {
        let io = std::io::Error::other("boom");
        let f = classify_sqlx_fault(::sqlx::Error::Io(io));
        match f {
            Fault::Transient(t) => assert_eq!(t.kind, TransientKind::ConnectionLost),
            other => panic!("expected Transient, got {other:?}"),
        }
    }

    #[test]
    fn protocol_error_is_fatal_dependency_not_transient() {
        let f = classify_sqlx_fault(::sqlx::Error::Protocol("synthesized".into()));
        match f {
            Fault::Fatal(fatal) => assert_eq!(fatal.kind, FatalKind::Dependency),
            other => panic!("expected Fatal(Dependency), got {other:?}"),
        }
    }

    #[test]
    fn configuration_error_is_fatal_config() {
        let f = classify_sqlx_fault(::sqlx::Error::Configuration("bad config".into()));
        match f {
            Fault::Fatal(fatal) => assert_eq!(fatal.kind, FatalKind::Config),
            other => panic!("expected Fatal(Config), got {other:?}"),
        }
    }

    /// `classify_sqlx<D>` must still widen through to any `D` — pinned
    /// separately from the `Fault` cases above so a regression in the
    /// `Fail<D>` wrapper (not just the shared `Fault` classification) fails
    /// its own test.
    #[test]
    fn classify_sqlx_widens_into_any_rejection_type() {
        #[derive(Debug)]
        struct NeverRejects;
        let f: Fail<NeverRejects> = classify_sqlx(::sqlx::Error::PoolTimedOut);
        assert_eq!(f.lane(), Lane::Transient);
    }

    #[test]
    fn transient_sqlstate_table_is_exhaustive_for_the_documented_codes() {
        let expected: &[(&str, TransientKind)] = &[
            ("40001", TransientKind::SerializationFailure),
            ("40P01", TransientKind::Deadlock),
            ("57P01", TransientKind::ConnectionLost),
            ("57P02", TransientKind::ConnectionLost),
            ("57P03", TransientKind::ConnectionLost),
            ("08000", TransientKind::ConnectionLost),
            ("08003", TransientKind::ConnectionLost),
            ("08006", TransientKind::ConnectionLost),
            ("08001", TransientKind::ConnectionLost),
            ("08004", TransientKind::ConnectionLost),
        ];
        assert_eq!(expected.len(), 10);
        for (code, kind) in expected {
            assert_eq!(transient_sqlstate(code), Some(*kind), "code {code}");
        }
        assert_eq!(transient_sqlstate("22012"), None);
        assert_eq!(transient_sqlstate("23505"), None);
    }
}

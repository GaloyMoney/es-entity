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
    fail::Fail,
    lane::{Fatal, FatalKind, Lane, Transient, TransientKind},
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
/// `Protocol(_)` lands in `Fatal(Dependency)`.
pub fn classify_sqlx<D>(e: ::sqlx::Error) -> Fail<D> {
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

/// Same table as [`classify_sqlx`] without consuming the error — for
/// `#[derive(errlanes::Classify)]`'s `#[lane(sqlx)]`.
pub fn lane_of_sqlx(e: &::sqlx::Error) -> Lane {
    match e {
        ::sqlx::Error::PoolTimedOut
        | ::sqlx::Error::Io(_)
        | ::sqlx::Error::Tls(_)
        | ::sqlx::Error::PoolClosed
        | ::sqlx::Error::WorkerCrashed => Lane::Transient,
        ::sqlx::Error::Database(db) => {
            if db.code().is_some_and(|c| transient_sqlstate(&c).is_some()) {
                Lane::Transient
            } else {
                Lane::Fatal
            }
        }
        _ => Lane::Fatal,
    }
}

impl<D> From<::sqlx::Error> for Fail<D> {
    fn from(e: ::sqlx::Error) -> Self {
        classify_sqlx(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane<D>(f: Fail<D>) -> Lane {
        f.lane()
    }

    #[test]
    fn pool_timed_out_is_transient_pool_timeout() {
        let f: Fail<core::convert::Infallible> = classify_sqlx(::sqlx::Error::PoolTimedOut);
        assert_eq!(lane(f), Lane::Transient);
    }

    #[test]
    fn row_not_found_is_fatal_corrupt_state() {
        let f: Fail<core::convert::Infallible> = classify_sqlx(::sqlx::Error::RowNotFound);
        match f {
            Fail::Fatal(fatal) => assert_eq!(fatal.kind, FatalKind::CorruptState),
            other => panic!("expected Fatal, got {other:?}"),
        }
    }

    #[test]
    fn io_error_is_transient_connection_lost() {
        let io = std::io::Error::other("boom");
        let f: Fail<core::convert::Infallible> = classify_sqlx(::sqlx::Error::Io(io));
        match f {
            Fail::Transient(t) => assert_eq!(t.kind, TransientKind::ConnectionLost),
            other => panic!("expected Transient, got {other:?}"),
        }
    }

    #[test]
    fn protocol_error_is_fatal_dependency_not_transient() {
        let f: Fail<core::convert::Infallible> =
            classify_sqlx(::sqlx::Error::Protocol("synthesized".into()));
        match f {
            Fail::Fatal(fatal) => assert_eq!(fatal.kind, FatalKind::Dependency),
            other => panic!("expected Fatal(Dependency), got {other:?}"),
        }
    }

    #[test]
    fn configuration_error_is_fatal_config() {
        let f: Fail<core::convert::Infallible> =
            classify_sqlx(::sqlx::Error::Configuration("bad config".into()));
        match f {
            Fail::Fatal(fatal) => assert_eq!(fatal.kind, FatalKind::Config),
            other => panic!("expected Fatal(Config), got {other:?}"),
        }
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

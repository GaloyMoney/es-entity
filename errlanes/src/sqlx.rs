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

/// [`classify_sqlx_fault`] for a borrowed error — the same table, with
/// `context` carrying the message in place of the source, which cannot be
/// moved out of a shared reference. For a boundary that only has `&(dyn
/// Error + 'static)` — see [`crate::classify_dyn`], which calls this after
/// finding a `sqlx::Error` in a chain it cannot otherwise classify. Prefer
/// [`classify_sqlx_fault`] when you own the error: it keeps the source.
///
/// Kept in sync with [`classify_sqlx_fault`] by this module's
/// `classify_sqlx_ref_matches_classify_sqlx_fault` test — a change to one
/// table without the other fails that test.
pub fn classify_sqlx_ref(e: &::sqlx::Error) -> Fault<crate::lanes!(Transient, Fatal)> {
    match e {
        ::sqlx::Error::PoolTimedOut => Transient::new(TransientKind::PoolTimeout).into(),
        ::sqlx::Error::Io(_) | ::sqlx::Error::Tls(_) => {
            Transient::new(TransientKind::ConnectionLost)
                .with_context(e.to_string())
                .into()
        }
        ::sqlx::Error::PoolClosed => Transient::new(TransientKind::ConnectionLost).into(),
        ::sqlx::Error::WorkerCrashed => Transient::new(TransientKind::ConnectionLost).into(),
        ::sqlx::Error::Database(db) => {
            if let Some(kind) = db.code().and_then(|c| transient_sqlstate(&c)) {
                return Transient::new(kind).with_context(e.to_string()).into();
            }
            if db.is_unique_violation() || db.is_foreign_key_violation() || db.is_check_violation()
            {
                let context = match db.constraint() {
                    Some(name) => format!("{name}: {e}"),
                    None => e.to_string(),
                };
                return Fatal::new(FatalKind::Invariant)
                    .with_context(context)
                    .into();
            }
            Fatal::new(FatalKind::Config)
                .with_context(e.to_string())
                .into()
        }
        ::sqlx::Error::RowNotFound
        | ::sqlx::Error::ColumnNotFound(_)
        | ::sqlx::Error::ColumnIndexOutOfBounds { .. }
        | ::sqlx::Error::ColumnDecode { .. }
        | ::sqlx::Error::Decode(_)
        | ::sqlx::Error::Encode(_)
        | ::sqlx::Error::TypeNotFound { .. } => Fatal::new(FatalKind::CorruptState)
            .with_context(e.to_string())
            .into(),
        ::sqlx::Error::Configuration(_) | ::sqlx::Error::AnyDriverError(_) => {
            Fatal::new(FatalKind::Config)
                .with_context(e.to_string())
                .into()
        }
        _ => Fatal::new(FatalKind::Dependency)
            .with_context(e.to_string())
            .into(),
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

    /// A minimal `DatabaseError` so tests can build `sqlx::Error::Database`
    /// without a live Postgres connection. Only what [`classify_sqlx_fault`]
    /// and [`classify_sqlx_ref`] actually read is wired up.
    #[derive(Debug, Clone, Copy)]
    enum TestDbKind {
        Unique,
        Other,
    }

    #[derive(Debug)]
    struct TestDbError {
        sqlstate: Option<&'static str>,
        kind: TestDbKind,
        constraint: Option<&'static str>,
    }

    impl std::fmt::Display for TestDbError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "test db error")
        }
    }
    impl std::error::Error for TestDbError {}

    impl ::sqlx::error::DatabaseError for TestDbError {
        fn message(&self) -> &str {
            "test db error"
        }
        fn code(&self) -> Option<std::borrow::Cow<'_, str>> {
            self.sqlstate.map(std::borrow::Cow::Borrowed)
        }
        fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
            self
        }
        fn kind(&self) -> ::sqlx::error::ErrorKind {
            match self.kind {
                TestDbKind::Unique => ::sqlx::error::ErrorKind::UniqueViolation,
                TestDbKind::Other => ::sqlx::error::ErrorKind::Other,
            }
        }
        fn constraint(&self) -> Option<&str> {
            self.constraint
        }
    }

    fn database_error(
        sqlstate: Option<&'static str>,
        kind: TestDbKind,
        constraint: Option<&'static str>,
    ) -> ::sqlx::Error {
        ::sqlx::Error::Database(Box::new(TestDbError {
            sqlstate,
            kind,
            constraint,
        }))
    }

    #[test]
    fn database_sqlstate_transient_is_transient_in_both_forms() {
        let f = classify_sqlx_fault(database_error(Some("40001"), TestDbKind::Other, None));
        assert_eq!(f.lane(), Lane::Transient);

        let e = database_error(Some("40P01"), TestDbKind::Other, None);
        let f = classify_sqlx_ref(&e);
        match f {
            Fault::Transient(t) => assert_eq!(t.kind, TransientKind::Deadlock),
            other => panic!("expected Transient, got {other:?}"),
        }
    }

    #[test]
    fn database_unique_violation_is_fatal_invariant_with_constraint_context() {
        let e = database_error(None, TestDbKind::Unique, Some("users_email_key"));
        let f = classify_sqlx_ref(&e);
        match f {
            Fault::Fatal(fatal) => {
                assert_eq!(fatal.kind, FatalKind::Invariant);
                assert!(
                    fatal
                        .context
                        .as_deref()
                        .unwrap()
                        .contains("users_email_key")
                );
            }
            other => panic!("expected Fatal(Invariant), got {other:?}"),
        }
    }

    /// `classify_sqlx_ref` must classify every case `classify_sqlx_fault`
    /// does, identically — this is the test that keeps the two tables in
    /// sync, since `sqlx::Error` is not `Clone` and the two functions
    /// therefore cannot share one match arm by value.
    #[test]
    fn classify_sqlx_ref_matches_classify_sqlx_fault() {
        let cases: Vec<(::sqlx::Error, ::sqlx::Error)> = vec![
            (::sqlx::Error::PoolTimedOut, ::sqlx::Error::PoolTimedOut),
            (
                ::sqlx::Error::Io(std::io::Error::other("a")),
                ::sqlx::Error::Io(std::io::Error::other("a")),
            ),
            (::sqlx::Error::PoolClosed, ::sqlx::Error::PoolClosed),
            (::sqlx::Error::WorkerCrashed, ::sqlx::Error::WorkerCrashed),
            (::sqlx::Error::RowNotFound, ::sqlx::Error::RowNotFound),
            (
                ::sqlx::Error::Configuration("bad".into()),
                ::sqlx::Error::Configuration("bad".into()),
            ),
            (
                ::sqlx::Error::Protocol("synthesized".into()),
                ::sqlx::Error::Protocol("synthesized".into()),
            ),
            (
                database_error(Some("40001"), TestDbKind::Other, None),
                database_error(Some("40001"), TestDbKind::Other, None),
            ),
            (
                database_error(None, TestDbKind::Unique, Some("c")),
                database_error(None, TestDbKind::Unique, Some("c")),
            ),
            (
                database_error(None, TestDbKind::Other, None),
                database_error(None, TestDbKind::Other, None),
            ),
        ];
        for (owned, borrowed) in cases {
            let by_value = classify_sqlx_fault(owned);
            let by_ref = classify_sqlx_ref(&borrowed);
            assert_eq!(
                by_value.lane(),
                by_ref.lane(),
                "lane mismatch for {borrowed}"
            );
            match (by_value, by_ref) {
                (Fault::Transient(a), Fault::Transient(b)) => {
                    assert_eq!(a.kind, b.kind, "kind mismatch for {borrowed}")
                }
                (Fault::Fatal(a), Fault::Fatal(b)) => {
                    assert_eq!(a.kind, b.kind, "kind mismatch for {borrowed}")
                }
                (a, b) => panic!("shape mismatch: {a:?} vs {b:?}"),
            }
        }
    }

    /// `classify_dyn` (in `crate::dynamic`) falls back to walking for a
    /// `sqlx::Error` when nothing in the chain is already laned — exercised
    /// here, alongside the sqlx table, since it only compiles under this
    /// feature.
    #[test]
    fn classify_dyn_finds_a_nested_sqlx_error() {
        #[derive(Debug)]
        struct Wrapper(::sqlx::Error);
        impl std::fmt::Display for Wrapper {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "wrapper")
            }
        }
        impl std::error::Error for Wrapper {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }

        let chain = Wrapper(::sqlx::Error::PoolTimedOut);
        match crate::dynamic::classify_dyn(&chain) {
            Fault::Transient(t) => assert_eq!(t.kind, TransientKind::PoolTimeout),
            other => panic!("expected Transient(PoolTimeout), got {other:?}"),
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

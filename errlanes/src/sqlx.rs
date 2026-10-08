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

/// Which lane each `sqlx::Error` variant belongs to — the one table both
/// public classifiers read, so there is no second copy to drift.
///
/// Payloads come back bare: `kind`, plus the constraint name as a
/// `Fatal(Invariant)`'s `context`, which is the only detail not recoverable
/// from the error's own `Display`. Each caller then attaches whatever it can
/// carry — the owned error as a `source`, or its message as `context`.
fn lane_table(e: &::sqlx::Error) -> Fault<crate::lanes!(Transient, Fatal)> {
    match e {
        ::sqlx::Error::PoolTimedOut => Transient::new(TransientKind::PoolTimeout).into(),
        ::sqlx::Error::Io(_)
        | ::sqlx::Error::Tls(_)
        | ::sqlx::Error::PoolClosed
        | ::sqlx::Error::WorkerCrashed => Transient::new(TransientKind::ConnectionLost).into(),
        ::sqlx::Error::Database(db) => {
            if let Some(kind) = db.code().and_then(|c| TransientKind::from_sqlstate(&c)) {
                return Transient::new(kind).into();
            }
            if db.is_unique_violation() || db.is_foreign_key_violation() || db.is_check_violation()
            {
                let fatal = Fatal::new(FatalKind::Invariant);
                return match db.constraint() {
                    Some(name) => fatal.with_context(name.to_string()),
                    None => fatal,
                }
                .into();
            }
            Fatal::new(FatalKind::Config).into()
        }
        ::sqlx::Error::RowNotFound
        | ::sqlx::Error::ColumnNotFound(_)
        | ::sqlx::Error::ColumnIndexOutOfBounds { .. }
        | ::sqlx::Error::ColumnDecode { .. }
        | ::sqlx::Error::Decode(_)
        | ::sqlx::Error::Encode(_)
        | ::sqlx::Error::TypeNotFound { .. } => Fatal::new(FatalKind::CorruptState).into(),
        ::sqlx::Error::Configuration(_) | ::sqlx::Error::AnyDriverError(_) => {
            Fatal::new(FatalKind::Config).into()
        }
        // `Protocol(_)` arrives here: see the module docs for why it is a
        // dependency bug rather than connection loss.
        _ => Fatal::new(FatalKind::Dependency).into(),
    }
}

/// Classifies a raw `sqlx::Error`, moving it — the error itself becomes the
/// lane payload's `source`, so the whole chain survives. What
/// `impl From<sqlx::Error> for Fault`/`Fail` delegates to.
fn classify_sqlx_fault(e: ::sqlx::Error) -> Fault<crate::lanes!(Transient, Fatal)> {
    match lane_table(&e) {
        Fault::Transient(t) => t.with_source(e).into(),
        Fault::Fatal(f) => f.with_source(e).into(),
    }
}

/// [`classify_sqlx_fault`] for a borrowed error — the same [`lane_table`],
/// with the message folded into `context` in place of the source, which
/// cannot be moved out of a shared reference. [`crate::Fault::classify`]
/// calls this after finding a `sqlx::Error` in a chain it cannot otherwise
/// classify. Prefer [`classify_sqlx_fault`] when you own the error: it keeps
/// the source.
pub(crate) fn classify_sqlx_ref(e: &::sqlx::Error) -> Fault<crate::lanes!(Transient, Fatal)> {
    /// The table's own `context` (a constraint name, when it set one) stays in
    /// front of the message rather than being overwritten by it.
    fn fold(context: Option<&str>, e: &::sqlx::Error) -> String {
        match context {
            Some(name) => format!("{name}: {e}"),
            None => e.to_string(),
        }
    }
    match lane_table(e) {
        Fault::Transient(t) => {
            let context = fold(t.context.as_deref(), e);
            t.with_context(context).into()
        }
        Fault::Fatal(f) => {
            let context = fold(f.context.as_deref(), e);
            f.with_context(context).into()
        }
    }
}

/// `sqlx::Error` never rejects — it enters the lanes with bare `?` through
/// the `Classify` blankets (`classify.rs`), replacing the hand-written
/// `From<sqlx::Error>` impls this feature used to carry.
impl crate::Classify for ::sqlx::Error {
    type Rejected = core::convert::Infallible;
    type Lanes = crate::lanes!(Transient, Fatal);

    fn classify(self) -> Fail<Self::Rejected, Self::Lanes> {
        match classify_sqlx_fault(self) {
            Fault::Transient(t) => Fail::Transient(t),
            Fault::Fatal(x) => Fail::Fatal(x),
        }
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

    /// A `sqlx::Error` must still expand through to any `D` — pinned
    /// separately from the `Fault` cases above so a regression in the
    /// `Fail<D>` wrapper (not just the shared `Fault` classification) fails
    /// its own test.
    #[test]
    fn classify_sqlx_expands_into_any_rejection_type() {
        #[derive(Debug)]
        struct NeverRejects;
        let f: Fail<NeverRejects> = ::sqlx::Error::PoolTimedOut.into();
        assert_eq!(f.lane(), Lane::Transient);
    }

    /// Every arm attaches the error it classified, including the unit
    /// variants that used to arrive sourceless — otherwise a
    /// `Fatal`/`Transient` born here records no `exception.message` beyond
    /// its own kind.
    #[test]
    fn classify_sqlx_fault_always_keeps_the_error_as_its_source() {
        for e in [
            ::sqlx::Error::PoolTimedOut,
            ::sqlx::Error::PoolClosed,
            ::sqlx::Error::WorkerCrashed,
            ::sqlx::Error::RowNotFound,
            ::sqlx::Error::Protocol("synthesized".into()),
            database_error(None, TestDbKind::Unique, Some("c")),
        ] {
            let display = e.to_string();
            let fault = classify_sqlx_fault(e);
            let payload = std::error::Error::source(&fault).expect("the lane payload");
            assert!(
                payload.source().is_some(),
                "no source attached for {display}"
            );
        }
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

    /// The borrowed form cannot keep a source, so the message has to land in
    /// `context` — without displacing the constraint name the table put
    /// there.
    #[test]
    fn classify_sqlx_ref_folds_the_message_behind_the_tables_context() {
        let e = database_error(None, TestDbKind::Unique, Some("users_email_key"));
        let message = e.to_string();
        match classify_sqlx_ref(&e) {
            Fault::Fatal(f) => {
                let context = f.context.as_deref().expect("context carries the message");
                assert!(context.starts_with("users_email_key: "), "got {context:?}");
                assert!(context.ends_with(&message), "got {context:?}");
            }
            other => panic!("expected Fatal(Invariant), got {other:?}"),
        }

        let e = ::sqlx::Error::PoolTimedOut;
        match classify_sqlx_ref(&e) {
            Fault::Transient(t) => {
                assert_eq!(t.context.as_deref(), Some(e.to_string().as_str()));
            }
            other => panic!("expected Transient, got {other:?}"),
        }
    }

    /// `Fault::classify` falls back to walking for a
    /// `sqlx::Error` when nothing in the chain is already laned — exercised
    /// here, alongside the sqlx table, since it only compiles under this
    /// feature.
    #[test]
    fn classify_finds_a_nested_sqlx_error() {
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
        match Fault::classify(&chain) {
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
            assert_eq!(
                TransientKind::from_sqlstate(code),
                Some(*kind),
                "code {code}"
            );
        }
        assert_eq!(TransientKind::from_sqlstate("22012"), None);
        assert_eq!(TransientKind::from_sqlstate("23505"), None);
    }
}

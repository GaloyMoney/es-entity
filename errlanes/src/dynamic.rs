use std::error::Error;

use crate::fail::Fault;
use crate::lane::{Denied, Fatal, FatalKind, Lane, Transient};

/// Walks `source()` looking for a concrete lane payload, reporting only which
/// lane it found. Allocation-free, so it is the one to reach for in a
/// predicate; [`fault_of`] is the same walk when you need the payload.
///
/// `None` means either the chain is not lanes-aware, or it bottoms out in a
/// `Rejected(D)` whose `D` is not known here — a caller that needs the
/// rejected value must go through [`crate::Failure`] instead.
///
/// An `Exhausted` is always carried *inside* a `Fatal`
/// ([`crate::profile::Settling`]), so it is not looked for separately.
pub fn lane_of(e: &(dyn Error + 'static)) -> Option<Lane> {
    let mut cur: Option<&(dyn Error + 'static)> = Some(e);
    while let Some(x) = cur {
        if x.is::<Transient>() {
            return Some(Lane::Transient);
        }
        if x.is::<Fatal>() {
            return Some(Lane::Fatal);
        }
        if x.is::<Denied>() {
            return Some(Lane::Denied);
        }
        cur = x.source();
    }
    None
}

/// Same walk as [`lane_of`], returning the lane payload itself as an owned
/// [`Fault`] rather than borrowing one field of it. A payload built under
/// errlanes' bounds is already `Send + Sync` (its own `source`, if any, is
/// stored behind that bound), so this is how a boundary holding a `&(dyn
/// Error + 'static)` that is *not* `Send`/`Sync` itself — a borrowed `Box<dyn
/// Error>`, say — hands the lane across a thread or task boundary: clone the
/// payload out here, then move the clone, not the original error.
///
/// `None` means either the chain is not lanes-aware, or it bottoms out in a
/// `Rejected(D)` whose `D` is not known here, same as [`lane_of`].
pub fn fault_of(e: &(dyn Error + 'static)) -> Option<Fault> {
    let mut cur: Option<&(dyn Error + 'static)> = Some(e);
    while let Some(x) = cur {
        if let Some(t) = x.downcast_ref::<Transient>() {
            return Some(Fault::Transient(t.clone()));
        }
        if let Some(f) = x.downcast_ref::<Fatal>() {
            return Some(Fault::Fatal(f.clone()));
        }
        if let Some(d) = x.downcast_ref::<Denied>() {
            return Some(Fault::Denied(d.clone()));
        }
        cur = x.source();
    }
    None
}

/// [`Error::to_string`] of `e` and every [`Error::source`] below it, joined
/// with `": "` — the one-line form a boundary persists, or writes as
/// `exception.message`, when it cannot keep the chain itself (e.g. because
/// the error, or the boundary's own storage, is not `Send`/`Sync`).
pub fn message_chain(e: &(dyn Error + 'static)) -> String {
    let mut parts = Vec::new();
    let mut cur: Option<&(dyn Error + 'static)> = Some(e);
    while let Some(x) = cur {
        parts.push(x.to_string());
        cur = x.source();
    }
    parts.join(": ")
}

/// Classify an erased error at a boundary that only has `&(dyn Error +
/// 'static)` — e.g. `&*boxed` for a `Box<dyn Error>` that is not
/// `Send`/`Sync` and so cannot become a lane payload's `source` (`Fatal`'s
/// and `Transient`'s sources are `Arc<dyn Error + Send + Sync>`, by design —
/// see the crate README). No `Send`/`Sync` bound is needed here: every step
/// only ever borrows `e`.
///
/// 1. A lane payload anywhere in the chain ([`fault_of`]) wins, carried
///    through intact — kind, context, and (now `Send`/`Sync`, having been
///    cloned out) its own original source.
/// 2. Else, with the `sqlx` feature, the first [`::sqlx::Error`] anywhere in
///    the chain is classified exactly as
///    [`classify_sqlx_fault`](crate::sqlx::classify_sqlx_fault) would,
///    through [`classify_sqlx_ref`](crate::sqlx::classify_sqlx_ref) — with
///    the error's message as `context` in place of the source this function
///    cannot move out of a shared reference.
/// 3. Else [`Fatal`]`(`[`FatalKind::Dependency`]`)`, with [`message_chain`]
///    as `context`.
///
/// Rule 3 is a safety default, not a shrug: an error this function cannot
/// otherwise classify must still surface as something a boundary pages on,
/// never silently as nothing.
pub fn classify_dyn(e: &(dyn Error + 'static)) -> Fault {
    if let Some(f) = fault_of(e) {
        return f;
    }
    #[cfg(feature = "sqlx")]
    {
        let mut cur: Option<&(dyn Error + 'static)> = Some(e);
        while let Some(x) = cur {
            if let Some(sql) = x.downcast_ref::<::sqlx::Error>() {
                return crate::sqlx::classify_sqlx_ref(sql).widen();
            }
            cur = x.source();
        }
    }
    Fault::Fatal(Fatal::new(FatalKind::Dependency).with_context(message_chain(e)))
}

#[cfg(test)]
mod tests {
    use std::fmt;

    use super::*;
    use crate::lane::{FatalKind, TransientKind};

    /// A source-chain link around any inner `Error`, so the same type nests
    /// to any depth: `Wrapped(Wrapped(Wrapped(payload)))` is three hops deep.
    #[derive(Debug)]
    struct Wrapped<E>(E);
    impl<E: fmt::Display> fmt::Display for Wrapped<E> {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "wrapped: {}", self.0)
        }
    }
    impl<E: Error + 'static> Error for Wrapped<E> {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn finds_transient_three_levels_deep() {
        let t = Transient::new(TransientKind::Deadlock);
        let chain = Wrapped(Wrapped(Wrapped(t)));
        assert_eq!(lane_of(&chain), Some(Lane::Transient));
        assert!(matches!(fault_of(&chain), Some(Fault::Transient(_))));
    }

    #[test]
    fn finds_fatal_three_levels_deep() {
        let f = Fatal::new(FatalKind::CorruptState);
        let chain = Wrapped(Wrapped(Wrapped(f)));
        assert_eq!(lane_of(&chain), Some(Lane::Fatal));
        assert!(matches!(fault_of(&chain), Some(Fault::Fatal(_))));
    }

    #[test]
    fn finds_denied_three_levels_deep() {
        let d = Denied::default();
        let chain = Wrapped(Wrapped(Wrapped(d)));
        assert_eq!(lane_of(&chain), Some(Lane::Denied));
        assert!(matches!(fault_of(&chain), Some(Fault::Denied(_))));
    }

    /// `Exhausted` is always carried *inside* a `Fatal` (`Fatal::from_error`
    /// in [`crate::profile::Settling`]), so neither walker special-cases it:
    /// the `Fatal` they find is the one whose own source downcasts to
    /// `Exhausted`.
    #[test]
    fn fault_of_exhausted_arrives_as_the_wrapping_fatal() {
        use crate::lane::Exhausted;

        let fatal = Fatal::from_error(
            FatalKind::Exhausted,
            Exhausted {
                attempts: 3,
                last: Transient::new(TransientKind::Deadlock),
            },
        );
        let chain = Wrapped(fatal);
        assert_eq!(lane_of(&chain), Some(Lane::Fatal));
        match fault_of(&chain) {
            Some(Fault::Fatal(f)) => {
                assert_eq!(f.kind, FatalKind::Exhausted);
                assert!(f.source().unwrap().downcast_ref::<Exhausted>().is_some());
            }
            other => panic!("expected Fault::Fatal wrapping Exhausted, got {other:?}"),
        }
    }

    #[test]
    fn message_chain_joins_display_of_every_hop() {
        #[derive(Debug)]
        struct Inner;
        impl fmt::Display for Inner {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "inner")
            }
        }
        impl Error for Inner {}

        #[derive(Debug)]
        struct Outer(Inner);
        impl fmt::Display for Outer {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "outer")
            }
        }
        impl Error for Outer {
            fn source(&self) -> Option<&(dyn Error + 'static)> {
                Some(&self.0)
            }
        }

        assert_eq!(message_chain(&Outer(Inner)), "outer: inner");
    }

    #[test]
    fn classify_dyn_finds_a_laned_error_in_the_chain() {
        let t = Transient::new(TransientKind::Deadlock);
        let chain = Wrapped(Wrapped(t));
        match classify_dyn(&chain) {
            Fault::Transient(t) => assert_eq!(t.kind, TransientKind::Deadlock),
            other => panic!("expected Fault::Transient, got {other:?}"),
        }
    }

    #[test]
    fn classify_dyn_falls_back_to_fatal_dependency() {
        let io = std::io::Error::other("disk full");
        match classify_dyn(&io) {
            Fault::Fatal(f) => {
                assert_eq!(f.kind, FatalKind::Dependency);
                assert_eq!(f.context.as_deref(), Some("disk full"));
            }
            other => panic!("expected Fault::Fatal(Dependency), got {other:?}"),
        }
    }

    /// The whole point of `classify_dyn`: it classifies by reference, so the
    /// resulting `Fault` is `Send` even when the boxed error behind it is
    /// not. If this stops compiling, the no-`Send`-required boundary has
    /// regressed.
    #[test]
    fn classify_dyn_result_is_send_even_when_the_source_error_is_not() {
        use std::rc::Rc;

        #[derive(Debug)]
        struct NotSend(#[allow(dead_code)] Rc<()>);
        impl fmt::Display for NotSend {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "not send")
            }
        }
        impl Error for NotSend {}

        let boxed: Box<dyn Error> = Box::new(NotSend(Rc::new(())));
        let fault = classify_dyn(&*boxed);
        drop(boxed); // the !Send error never needs to cross the thread boundary
        let handle = std::thread::spawn(move || {
            assert!(matches!(fault, Fault::Fatal(_)));
        });
        handle.join().unwrap();
    }

    #[test]
    fn bare_non_lanes_error_yields_none() {
        #[derive(Debug)]
        struct Plain;
        impl std::fmt::Display for Plain {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "plain")
            }
        }
        impl Error for Plain {}

        assert_eq!(lane_of(&Plain), None);
        assert!(fault_of(&Plain).is_none());
    }
}

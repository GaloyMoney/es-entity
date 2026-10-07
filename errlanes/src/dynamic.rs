use std::error::Error;

use crate::fail::Fault;
use crate::lane::{Denied, Exhausted, Fatal, FatalKind, Lane, Transient};

impl Lane {
    /// Walks `source()` looking for a concrete lane payload, reporting only
    /// which lane it found. Allocation-free, so it is the one to reach for
    /// in a predicate; `find_in` (private — reached through
    /// [`Fault::classify`]) is the same walk when you need the payload.
    ///
    /// `None` means either the chain is not lanes-aware, or it bottoms out
    /// in a `Rejected(D)` whose `D` is not known here — a caller that needs
    /// the rejected value must go through a [`crate::Carrier`] or [`crate::Fail`] instead.
    ///
    /// A bare `Exhausted` (not wrapped in a `Fatal`, which is how
    /// [`crate::profile::NarrowTransient`] always produces one) is still
    /// `Fatal`: it is a terminal outcome by construction, and its own
    /// `source()` is the last `Transient` it gave up on, which this walker
    /// must not mistake for the chain's own classification.
    pub fn of(e: &(dyn Error + 'static)) -> Option<Lane> {
        let mut cur: Option<&(dyn Error + 'static)> = Some(e);
        while let Some(x) = cur {
            if x.is::<Transient>() {
                return Some(Lane::Transient);
            }
            if x.is::<Fatal>() || x.is::<Exhausted>() {
                return Some(Lane::Fatal);
            }
            if x.is::<Denied>() {
                return Some(Lane::Denied);
            }
            cur = x.source();
        }
        None
    }
}

/// Same walk as [`Lane::of`], returning the lane payload itself as an owned
/// [`Fault`] rather than borrowing one field of it. A payload built under
/// errlanes' bounds is already `Send + Sync` (its own `source`, if any, is
/// stored behind that bound), so this is how a boundary holding a `&(dyn
/// Error + 'static)` that is *not* `Send`/`Sync` itself — a borrowed `Box<dyn
/// Error>`, say — hands the lane across a thread or task boundary: clone the
/// payload out here, then move the clone, not the original error.
///
/// `None` means either the chain is not lanes-aware, or it bottoms out in a
/// `Rejected(D)` whose `D` is not known here, same as [`Lane::of`]. A bare
/// `Exhausted` is rewrapped into the `Fatal(Exhausted)` it is always meant
/// to travel inside, for the same reason `Lane::of` special-cases it.
fn find_in(e: &(dyn Error + 'static)) -> Option<Fault> {
    let mut cur: Option<&(dyn Error + 'static)> = Some(e);
    while let Some(x) = cur {
        if let Some(t) = x.downcast_ref::<Transient>() {
            return Some(Fault::Transient(t.clone()));
        }
        if let Some(f) = x.downcast_ref::<Fatal>() {
            return Some(Fault::Fatal(f.clone()));
        }
        if let Some(ex) = x.downcast_ref::<Exhausted>() {
            return Some(Fault::Fatal(Fatal::from_error(
                FatalKind::Exhausted,
                ex.clone(),
            )));
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
pub(crate) fn message_chain(e: &(dyn Error + 'static)) -> String {
    let mut parts = Vec::new();
    let mut cur: Option<&(dyn Error + 'static)> = Some(e);
    while let Some(x) = cur {
        parts.push(x.to_string());
        // A `Fatal` built over a domain value kept only for programmatic
        // access (`Fatal::with_opaque_source` — a partial lift's unmapped
        // rejection, a narrowed `Rejected`) must not have that value's
        // `Display` walked into: it may embed caller-supplied input, and
        // this chain is the text a boundary treats as operator-safe.
        if x.downcast_ref::<Fatal>()
            .is_some_and(Fatal::has_opaque_source)
        {
            break;
        }
        cur = x.source();
    }
    parts.join(": ")
}

impl Fault {
    /// Classify an erased error at a boundary that only has `&(dyn Error +
    /// 'static)` — e.g. `&*boxed` for a `Box<dyn Error>` that is not
    /// `Send`/`Sync` and so cannot become a lane payload's `source`
    /// (`Fatal`'s and `Transient`'s sources are `Arc<dyn Error + Send +
    /// Sync>`, by design — see the crate README). No `Send`/`Sync` bound is
    /// needed here: every step only ever borrows `e`. Call before the next
    /// `.await` — the chain being walked may not be `Send`.
    ///
    /// 1. A lane payload anywhere in the chain wins, carried through intact
    ///    — kind, context, and (now `Send`/`Sync`, having been cloned out)
    ///    its own original source.
    /// 2. Else, for each blessed foreign type whose `classify-*` feature is
    ///    enabled (`sqlx::Error`, `serde_json::Error`, `reqwest::Error`, in
    ///    that order), the first one found anywhere in the chain is
    ///    classified exactly as that feature's `impl Classify` would — with
    ///    the error's message as `context` in place of the source this
    ///    function cannot move out of a shared reference.
    /// 3. Else [`Fatal`]`(`[`FatalKind::Dependency`]`)`, with
    ///    `message_chain` (private) as `context`.
    ///
    /// Rule 3 is a safety default, not a shrug: an error this function
    /// cannot otherwise classify must still surface as something a boundary
    /// pages on, never silently as nothing. A lane payload found by this
    /// rule may itself be a `Denied` — narrowing it away, if the boundary
    /// has no subject, is the caller's explicit next step (see
    /// [`Fault::narrow_denied`]).
    pub fn classify(e: &(dyn Error + 'static)) -> Fault {
        if let Some(f) = find_in(e) {
            return f;
        }
        #[cfg(feature = "classify-sqlx")]
        {
            let mut cur: Option<&(dyn Error + 'static)> = Some(e);
            while let Some(x) = cur {
                if let Some(sql) = x.downcast_ref::<::sqlx::Error>() {
                    return crate::sqlx::classify_sqlx_ref(sql).widen();
                }
                cur = x.source();
            }
        }
        #[cfg(feature = "classify-serde-json")]
        {
            let mut cur: Option<&(dyn Error + 'static)> = Some(e);
            while let Some(x) = cur {
                if let Some(json) = x.downcast_ref::<::serde_json::Error>() {
                    return crate::serde_json::classify_serde_json_ref(json).widen();
                }
                cur = x.source();
            }
        }
        #[cfg(feature = "classify-reqwest")]
        {
            let mut cur: Option<&(dyn Error + 'static)> = Some(e);
            while let Some(x) = cur {
                if let Some(req) = x.downcast_ref::<::reqwest::Error>() {
                    return crate::reqwest::classify_reqwest_ref(req).widen();
                }
                cur = x.source();
            }
        }
        Fault::Fatal(Fatal::new(FatalKind::Dependency).with_context(message_chain(e)))
    }
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
        assert_eq!(Lane::of(&chain), Some(Lane::Transient));
        assert!(matches!(find_in(&chain), Some(Fault::Transient(_))));
    }

    #[test]
    fn finds_fatal_three_levels_deep() {
        let f = Fatal::new(FatalKind::CorruptState);
        let chain = Wrapped(Wrapped(Wrapped(f)));
        assert_eq!(Lane::of(&chain), Some(Lane::Fatal));
        assert!(matches!(find_in(&chain), Some(Fault::Fatal(_))));
    }

    #[test]
    fn finds_denied_three_levels_deep() {
        let d = Denied::default();
        let chain = Wrapped(Wrapped(Wrapped(d)));
        assert_eq!(Lane::of(&chain), Some(Lane::Denied));
        assert!(matches!(find_in(&chain), Some(Fault::Denied(_))));
    }

    /// `Exhausted` is normally carried *inside* a `Fatal` (`Fatal::from_error`
    /// in [`crate::profile::NarrowTransient`]), so this exercises the
    /// ordinary shape: the `Fatal` they find is the one whose own source
    /// downcasts to `Exhausted`.
    #[test]
    fn find_in_exhausted_arrives_as_the_wrapping_fatal() {
        use crate::lane::Exhausted;

        let fatal = Fatal::from_error(
            FatalKind::Exhausted,
            Exhausted {
                attempts: 3,
                last: Transient::new(TransientKind::Deadlock),
            },
        );
        let chain = Wrapped(fatal);
        assert_eq!(Lane::of(&chain), Some(Lane::Fatal));
        match find_in(&chain) {
            Some(Fault::Fatal(f)) => {
                assert_eq!(f.kind, FatalKind::Exhausted);
                assert!(f.source().unwrap().downcast_ref::<Exhausted>().is_some());
            }
            other => panic!("expected Fault::Fatal wrapping Exhausted, got {other:?}"),
        }
    }

    /// Regression: a BARE `Exhausted` (not wrapped in a `Fatal` -- nothing
    /// in the public API stops a caller from building or boxing one
    /// directly) must still classify as `Fatal`, not fall through to its
    /// own `source()`, which is the last `Transient` it gave up on. Before
    /// the fix, neither walker special-cased `Exhausted` itself: it is not
    /// a `Transient`/`Fatal`/`Denied`, so the loop moved on to its source
    /// and found that `Transient` instead -- reopening retries after the
    /// budget was already spent.
    #[test]
    fn bare_exhausted_classifies_as_fatal_not_its_inner_transient() {
        use crate::lane::Exhausted;

        let exhausted = Exhausted {
            attempts: 3,
            last: Transient::new(TransientKind::Deadlock),
        };
        assert_eq!(Lane::of(&exhausted), Some(Lane::Fatal));
        match find_in(&exhausted) {
            Some(Fault::Fatal(f)) => assert_eq!(f.kind, FatalKind::Exhausted),
            other => panic!("expected Fault::Fatal(Exhausted), got {other:?}"),
        }

        // Same, one hop deeper in a chain.
        let chain = Wrapped(exhausted);
        assert_eq!(Lane::of(&chain), Some(Lane::Fatal));
        match find_in(&chain) {
            Some(Fault::Fatal(f)) => assert_eq!(f.kind, FatalKind::Exhausted),
            other => panic!("expected Fault::Fatal(Exhausted), got {other:?}"),
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
    fn classify_finds_a_laned_error_in_the_chain() {
        let t = Transient::new(TransientKind::Deadlock);
        let chain = Wrapped(Wrapped(t));
        match Fault::classify(&chain) {
            Fault::Transient(t) => assert_eq!(t.kind, TransientKind::Deadlock),
            other => panic!("expected Fault::Transient, got {other:?}"),
        }
    }

    #[test]
    fn classify_falls_back_to_fatal_dependency() {
        let io = std::io::Error::other("disk full");
        match Fault::classify(&io) {
            Fault::Fatal(f) => {
                assert_eq!(f.kind, FatalKind::Dependency);
                assert_eq!(f.context.as_deref(), Some("disk full"));
            }
            other => panic!("expected Fault::Fatal(Dependency), got {other:?}"),
        }
    }

    /// The whole point of `Fault::classify`: it classifies by reference, so the
    /// resulting `Fault` is `Send` even when the boxed error behind it is
    /// not. If this stops compiling, the no-`Send`-required boundary has
    /// regressed.
    #[test]
    fn classify_result_is_send_even_when_the_source_error_is_not() {
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
        let fault = Fault::classify(&*boxed);
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

        assert_eq!(Lane::of(&Plain), None);
        assert!(find_in(&Plain).is_none());
    }
}

use std::error::Error;

use crate::lane::{Denied, Fatal, Lane, Transient};

/// Walks `source()` looking for a concrete lane payload.
///
/// `None` means either the chain is not lanes-aware, or it bottoms out in a
/// `Rejected(D)` whose `D` is not known here — a caller that needs the
/// rejected value must go through [`crate::Failure`] instead. Never
/// allocates.
pub fn lane_of(e: &(dyn Error + 'static)) -> Option<Lane> {
    let mut cur: Option<&(dyn Error + 'static)> = Some(e);
    while let Some(x) = cur {
        if x.is::<Transient>() {
            return Some(Lane::Transient);
        }
        if x.is::<crate::lane::Exhausted>() || x.is::<Fatal>() {
            return Some(Lane::Fatal);
        }
        if x.is::<Denied>() {
            return Some(Lane::Denied);
        }
        cur = x.source();
    }
    None
}

/// Same walk as [`lane_of`], returning the `Transient` payload rather than
/// just the lane.
pub fn transient_of<'a>(e: &'a (dyn Error + 'static)) -> Option<&'a Transient> {
    let mut cur: Option<&(dyn Error + 'static)> = Some(e);
    while let Some(x) = cur {
        if let Some(t) = x.downcast_ref::<Transient>() {
            return Some(t);
        }
        cur = x.source();
    }
    None
}

/// Same walk as [`transient_of`], returning the `Fatal` payload. An
/// `Exhausted` in the chain is always wrapped by a `Fatal`, so this finds it.
pub fn fatal_of<'a>(e: &'a (dyn Error + 'static)) -> Option<&'a Fatal> {
    let mut cur: Option<&(dyn Error + 'static)> = Some(e);
    while let Some(x) = cur {
        if let Some(f) = x.downcast_ref::<Fatal>() {
            return Some(f);
        }
        cur = x.source();
    }
    None
}

/// Same walk as [`transient_of`] and [`fatal_of`], returning the `Denied`
/// payload.
pub fn denied_of<'a>(e: &'a (dyn Error + 'static)) -> Option<&'a Denied> {
    let mut cur: Option<&(dyn Error + 'static)> = Some(e);
    while let Some(x) = cur {
        if let Some(d) = x.downcast_ref::<Denied>() {
            return Some(d);
        }
        cur = x.source();
    }
    None
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
        assert!(transient_of(&chain).is_some());
    }

    #[test]
    fn finds_fatal_of_three_levels_deep() {
        let f = Fatal::new(FatalKind::CorruptState);
        let chain = Wrapped(Wrapped(Wrapped(f)));
        assert_eq!(lane_of(&chain), Some(Lane::Fatal));
        assert!(fatal_of(&chain).is_some());
    }

    #[test]
    fn finds_denied_of_three_levels_deep() {
        let d = Denied::default();
        let chain = Wrapped(Wrapped(Wrapped(d)));
        assert_eq!(lane_of(&chain), Some(Lane::Denied));
        assert!(denied_of(&chain).is_some());
    }

    #[test]
    fn finds_fatal_three_levels_deep() {
        let fatal = Fatal::new(FatalKind::CorruptState);
        #[derive(Debug)]
        struct W(Fatal);
        impl std::fmt::Display for W {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
        impl Error for W {
            fn source(&self) -> Option<&(dyn Error + 'static)> {
                Some(&self.0)
            }
        }
        let chain = W(fatal);
        assert_eq!(lane_of(&chain), Some(Lane::Fatal));
        assert!(fatal_of(&chain).is_some());
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
    }
}

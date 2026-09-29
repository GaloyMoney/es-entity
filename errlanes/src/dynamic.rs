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

/// Same walk as [`lane_of`], returning the `Transient` payload (for its
/// `retry_after`) rather than just the lane.
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

/// Bridge trait for legacy `thiserror` enums that have not adopted
/// [`crate::Fail`]. Derived by `#[derive(errlanes::Classify)]`.
pub trait Classify {
    fn lane(&self) -> Lane;
}

impl<D: crate::fail::Rejection> Classify for crate::fail::Fail<D> {
    fn lane(&self) -> Lane {
        crate::fail::Fail::lane(self)
    }
}

impl<D: crate::fail::Rejection> Classify for crate::fail::Settled<D> {
    fn lane(&self) -> Lane {
        crate::fail::Settled::lane(self)
    }
}

impl Classify for crate::fail::Fault {
    fn lane(&self) -> Lane {
        crate::fail::Fault::lane(self)
    }
}

impl Classify for crate::fail::SettledFault {
    fn lane(&self) -> Lane {
        crate::fail::SettledFault::lane(self)
    }
}

impl Classify for Transient {
    fn lane(&self) -> Lane {
        Lane::Transient
    }
}

impl Classify for Fatal {
    fn lane(&self) -> Lane {
        Lane::Fatal
    }
}

impl Classify for Denied {
    fn lane(&self) -> Lane {
        Lane::Denied
    }
}

impl Classify for crate::lane::Exhausted {
    fn lane(&self) -> Lane {
        Lane::Fatal
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lane::{FatalKind, TransientKind};

    #[derive(Debug)]
    struct WrapperA(Transient);
    impl std::fmt::Display for WrapperA {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "wrapper a: {}", self.0)
        }
    }
    impl Error for WrapperA {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.0)
        }
    }

    #[derive(Debug)]
    struct WrapperB(WrapperA);
    impl std::fmt::Display for WrapperB {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "wrapper b: {}", self.0)
        }
    }
    impl Error for WrapperB {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.0)
        }
    }

    #[derive(Debug)]
    struct WrapperC(WrapperB);
    impl std::fmt::Display for WrapperC {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "wrapper c: {}", self.0)
        }
    }
    impl Error for WrapperC {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&self.0)
        }
    }

    #[test]
    fn finds_transient_three_levels_deep() {
        let t = Transient::new(TransientKind::Deadlock);
        let chain = WrapperC(WrapperB(WrapperA(t)));
        assert_eq!(lane_of(&chain), Some(Lane::Transient));
        assert!(transient_of(&chain).is_some());
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

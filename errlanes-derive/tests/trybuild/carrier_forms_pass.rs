use errlanes::{Fault, lanes};

#[derive(Debug, errlanes::Rejection)]
pub enum Closed {
    Yes,
}

/// docs are kept on the enum
#[derive(Debug, errlanes::Carrier)]
#[derive(Clone)]
pub enum Any3 {
    Denied(errlanes::Denied),
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

#[derive(Debug, errlanes::Carrier)]
pub enum NoLanes {
    Rejected(Closed),
}

#[derive(Debug, errlanes::Carrier)]
pub enum Generic<R> {
    Rejected(R),
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

#[derive(Debug, errlanes::Carrier)]
pub enum Hand {
    /// doc on a variant
    Transient(errlanes::Transient),
    Fatal(errlanes::Fatal),
}

fn main() {
    let a = Any3::Denied(errlanes::Denied::new());
    assert!(a.is_denied() && a.as_denied().is_some() && a.clone().lane() == errlanes::Lane::Denied);
    match a {
        Any3::Denied(_) | Any3::Transient(_) | Any3::Fatal(_) => {}
    }
    let n = NoLanes::Rejected(Closed::Yes);
    assert!(n.is_rejected());
    assert_eq!(n.message(), "YES");
    let g: Generic<Closed> = Generic::Rejected(Closed::Yes);
    assert!(g.as_rejected().is_some());
    let h: Hand = errlanes::Fatal::invariant("x").into();
    assert!(h.is_fatal());
    let f: Fault<lanes!(Transient, Fatal)> = h.into_repr_for_test();
    assert!(f.is_fatal());
}

trait IntoReprForTest: errlanes::Carrier {
    fn into_repr_for_test(self) -> Self::Repr {
        self.into_repr()
    }
}
impl IntoReprForTest for Hand {}

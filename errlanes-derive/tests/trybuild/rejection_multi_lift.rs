// `lift = X` is repeatable: a domain rejection can lift keys from more than
// one foreign `Liftable`. A `key = X::Variant` path is routed to the `impl
// Lift<X>` whose target its owner segments match — this fixture pins that
// routing across two distinct targets sharing no key names.

#[derive(Debug, Clone, Copy, PartialEq, Eq, std::hash::Hash)]
enum AKey {
    Frozen,
}
impl From<AKey> for &'static str {
    fn from(k: AKey) -> Self {
        match k {
            AKey::Frozen => "FROZEN",
        }
    }
}
impl std::fmt::Display for AKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str((*self).into())
    }
}

#[derive(Debug)]
struct AViolation(Option<AKey>);
impl std::fmt::Display for AViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "a violation")
    }
}
impl std::error::Error for AViolation {}
impl errlanes::Rejection for AViolation {
    type Code = AKey;
    fn code(&self) -> Self::Code {
        self.0.unwrap_or(AKey::Frozen)
    }
}
impl errlanes::Liftable for AViolation {
    type Key = AKey;
    fn key(&self) -> Option<AKey> {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, std::hash::Hash)]
enum BKey {
    Blocked,
}
impl From<BKey> for &'static str {
    fn from(k: BKey) -> Self {
        match k {
            BKey::Blocked => "BLOCKED",
        }
    }
}
impl std::fmt::Display for BKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str((*self).into())
    }
}

#[derive(Debug)]
struct BViolation(Option<BKey>);
impl std::fmt::Display for BViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "b violation")
    }
}
impl std::error::Error for BViolation {}
impl errlanes::Rejection for BViolation {
    type Code = BKey;
    fn code(&self) -> Self::Code {
        self.0.unwrap_or(BKey::Blocked)
    }
}
impl errlanes::Liftable for BViolation {
    type Key = BKey;
    fn key(&self) -> Option<BKey> {
        self.0
    }
}

#[derive(Debug, Clone, thiserror::Error, errlanes::Rejection)]
#[rejection(lift(AViolation, BViolation))]
enum MultiRejection {
    #[error("a frozen")]
    #[rejection(key = AKey::Frozen, via = AViolation)]
    AFrozen,
    #[error("b blocked")]
    #[rejection(key = BKey::Blocked, via = BViolation)]
    BBlocked,
}

fn main() {
    use errlanes::Lift;

    match MultiRejection::lift(AViolation(Some(AKey::Frozen))) {
        Ok(MultiRejection::AFrozen) => {}
        other => panic!("expected AFrozen, got {other:?}"),
    }
    match MultiRejection::lift(BViolation(Some(BKey::Blocked))) {
        Ok(MultiRejection::BBlocked) => {}
        other => panic!("expected BBlocked, got {other:?}"),
    }
    match MultiRejection::lift(AViolation(None)) {
        Err(fatal) => assert_eq!(fatal.kind, errlanes::FatalKind::Invariant),
        other => panic!("expected Fatal(Invariant), got {other:?}"),
    }
}

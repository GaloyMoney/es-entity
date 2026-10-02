// Multiple source families remain independent from rejection metadata.
#[derive(Debug)]
enum A { Frozen, Other }
#[derive(Debug)]
enum B { Blocked }

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(A, unhandled = fatal)]
#[lift(B)]
enum MultiRejection {
    #[rejection(code = "FROZEN")]
    #[lift(A::Frozen)]
    AFrozen,
    #[rejection(code = "BLOCKED")]
    #[lift(B::Blocked)]
    BBlocked,
}

fn main() {
    use errlanes::Lift;
    assert!(matches!(MultiRejection::lift(A::Frozen), Ok(MultiRejection::AFrozen)));
    assert!(matches!(MultiRejection::from(B::Blocked), MultiRejection::BBlocked));
    assert!(matches!(MultiRejection::lift(A::Other), Err(A::Other)));
}

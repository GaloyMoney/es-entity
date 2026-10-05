#[derive(Debug, errlanes::Rejection)]
enum Left { Shared }
#[derive(Debug, errlanes::Rejection)]
enum Right { Shared }
#[derive(Debug, errlanes::Rejection)]
enum AddedLater { Shared }

#[errlanes::compose(Left, Right, AddedLater)]
enum ReviewedParticipants {
    #[compose(merge(Left::Shared, Right::Shared))]
    #[rejection(code = "CANONICAL")]
    Shared,
}
fn main() {}

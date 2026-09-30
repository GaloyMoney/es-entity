#[derive(Debug, thiserror::Error, errlanes::Rejection)]
pub enum Leaf { #[error("one")] One }
#[errlanes::rejection]
#[derive(Debug, thiserror::Error, errlanes::Lift)]
pub enum Left { #[flatten] Leaf(Leaf) }
#[errlanes::rejection]
#[derive(Debug, thiserror::Error, errlanes::Lift)]
pub enum Right { #[flatten] Leaf(Leaf) }
#[errlanes::rejection]
#[derive(Debug, thiserror::Error, errlanes::Lift)]
pub enum Diamond {
    #[flatten(prefix = "Left")] Left(Left),
    #[flatten(prefix = "Right")] Right(Right),
}
fn main() {}

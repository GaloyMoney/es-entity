#[derive(Debug, thiserror::Error, errlanes::Rejection)]
pub enum Leaf { #[error("one")] One }
#[errlanes::rejection]
#[derive(Debug, thiserror::Error)]
pub enum Left { #[flatten] Leaf(Leaf) }
#[errlanes::rejection]
#[derive(Debug, thiserror::Error)]
pub enum Right { #[flatten] Leaf(Leaf) }
#[errlanes::rejection]
#[derive(Debug, thiserror::Error)]
pub enum Diamond {
    #[flatten(prefix = "Left")] Left(Left),
    #[flatten(prefix = "Right")] Right(Right),
}
fn main() {}

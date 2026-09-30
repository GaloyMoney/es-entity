#[derive(Debug, thiserror::Error, errlanes::Rejection)]
pub enum Leaf { #[error("one")] One }
#[errlanes::compose]
#[derive(Debug, thiserror::Error)]
pub enum Left { #[compose(flatten)] Leaf(Leaf) }
#[errlanes::compose]
#[derive(Debug, thiserror::Error)]
pub enum Right { #[compose(flatten)] Leaf(Leaf) }
#[errlanes::compose]
#[derive(Debug, thiserror::Error)]
pub enum Diamond {
    #[compose(flatten)] Left(Left),
    #[compose(flatten)] Right(Right),
}
fn main() {}

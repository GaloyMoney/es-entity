mod raw {
    #[derive(Debug)]
    pub struct Raw(pub u8);
}

mod alias {
    pub use crate::raw::Raw;
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(raw::Raw)]
enum Left {
    #[error("left")]
    #[rejection(code = "LEFT")]
    #[lift(raw::Raw)]
    A(raw::Raw),
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(alias::Raw)]
enum Right {
    #[error("right")]
    #[rejection(code = "RIGHT")]
    #[lift(alias::Raw)]
    B(alias::Raw),
}

#[errlanes::compose(Left, Right)]
#[derive(Debug)]
enum Top {}

fn main() {}

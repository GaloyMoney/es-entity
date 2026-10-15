#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "SHARED")]
enum Unused {
    #[rejection(code = "A")]
    A,
    #[rejection(code = "B")]
    B,
}

fn main() {}

#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "SHARED", description = "shared")]
enum Described {
    #[rejection(description = "mine")]
    A,
    B,
}

fn main() {}

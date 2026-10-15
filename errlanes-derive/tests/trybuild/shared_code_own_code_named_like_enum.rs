#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "SHARED")]
enum Clash {
    #[rejection(code = "OWN")]
    Clash,
    Other,
}

fn main() {}

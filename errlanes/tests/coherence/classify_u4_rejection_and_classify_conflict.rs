// Appendix B, U4: a type is a `Rejection` or it implements `Classify`
// directly, never both — a direct impl conflicts with the blanket
// `impl<R: Rejection> Classify for R` (E0119).
#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "CORRUPT")]
struct Stored;

impl errlanes::Classify for Stored {
    type Rejected = std::convert::Infallible;
    type Lanes = errlanes::lanes!(Fatal);

    fn classify(self) -> errlanes::Fail<Self::Rejected, Self::Lanes> {
        errlanes::Fail::Fatal(errlanes::Fatal::new(errlanes::FatalKind::CorruptState))
    }
}

fn main() {}

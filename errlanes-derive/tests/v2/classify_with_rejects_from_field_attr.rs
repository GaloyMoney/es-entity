fn classify_it(_: Wrapped) -> errlanes::Fail<core::convert::Infallible, errlanes::lanes!(Transient, Fatal)> {
    unimplemented!()
}

#[derive(Debug, errlanes::Classify)]
#[classify(with = classify_it, lanes(Transient, Fatal))]
struct Wrapped(#[from] std::io::Error);

fn main() {}

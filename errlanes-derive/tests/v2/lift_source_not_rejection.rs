// `Lift` sources are rejections: the blanket `impl<X: Rejection, P: From<X>>
// Lift<X> for P` (bounded this way so it does not overlap `impl<P>
// Lift<Infallible> for P`) means a strict-mode `#[derive(Lift)]` still emits
// `From<Source>`, but the `Lift` trait view is only available when `Source`
// is a `Rejection`.
#[derive(Debug, PartialEq)]
enum Source {
    Value(u32),
}

#[derive(Debug, PartialEq, errlanes::Lift)]
#[lift(Source)]
enum Destination {
    #[lift(Source::Value)]
    Value(u32),
}

fn main() {
    let _: Result<Destination, _> = errlanes::Lift::lift(Source::Value(1));
}

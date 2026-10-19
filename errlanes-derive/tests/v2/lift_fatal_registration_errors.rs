enum Source {
    A,
    B,
}

#[derive(errlanes::Lift)]
#[lift(Source, unhandled = fatal, fatal(B))]
enum CatchAllAndCases {
    #[lift(Source::A)]
    A,
}

#[derive(errlanes::Lift)]
#[lift(Source, fatal(B), unhandled = fatal)]
enum CasesAndCatchAll {
    #[lift(Source::A)]
    A,
}

#[derive(errlanes::Lift)]
#[lift(Source, strict, fatal(B))]
enum StrictAndCases {
    #[lift(Source::A)]
    A,
}

#[derive(errlanes::Lift)]
#[lift(Source, fatal(B), strict)]
enum CasesAndStrict {
    #[lift(Source::A)]
    A,
}

#[derive(errlanes::Lift)]
#[lift(Source, fatal())]
enum Empty {
    #[lift(Source::A)]
    A,
}

#[derive(errlanes::Lift)]
#[lift(Source, fatal(Source::B))]
enum RepeatedSource {
    #[lift(Source::A)]
    A,
}

fn main() {}

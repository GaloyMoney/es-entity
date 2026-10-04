use errlanes::{Fail, ResultExt, lanes};

#[derive(Debug, errlanes::Rejection)]
enum Source { Value { value: u32 }, Other }

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[rejection(code = "DESTINATION")]
#[lift(Source, variant = Value, unhandled = fatal)]
struct Destination { value: u32 }

fn boundary(result: Result<(), Source>) -> Result<(), Fail<Destination, lanes!()>> {
    result.widen()
}

fn main() {}

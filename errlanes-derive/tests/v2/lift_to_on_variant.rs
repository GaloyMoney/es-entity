#[derive(Debug)]
enum Source {
    Value,
}

#[derive(Debug, errlanes::Lift)]
#[lift(Source)]
enum Destination {
    #[lift(Source::Value, to = Destination)]
    Present,
}

fn main() {}

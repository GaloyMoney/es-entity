#[derive(Debug, errlanes::Rejection)]
enum Source {
    Value(u8),
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Source)]
enum Destination {
    #[lift(Source::Value, into)]
    Value(u64),
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Source)]
enum LevelOnly {
    #[lift(Source::Value, into)]
    #[rejection(level = "warn")]
    Value(u64),
}

fn main() {}

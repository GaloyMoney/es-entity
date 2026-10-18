#[derive(Debug, errlanes::Rejection)]
enum Leaf {
    Disabled,
}

#[errlanes::compose(Leaf)]
#[derive(Debug)]
enum Middle {
    Own,
}

#[errlanes::compose(Middle(no_from))]
#[derive(Debug)]
enum Top {}

fn main() {
    let _ = Top::from(Middle::Own);
    let _ = Top::from(Leaf::Disabled);
}

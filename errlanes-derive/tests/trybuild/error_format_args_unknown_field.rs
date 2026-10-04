// A trailing argument's root must be one of the unit's own fields, same as
// a named placeholder.
#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "X")]
#[error("bad: {}", nope.field)]
struct Bad {
    id: i32,
}

fn main() {}

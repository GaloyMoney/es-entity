// A `key = ...` variant's own path names the *key* type, not the *lift*
// (`Liftable`) type — the two are unrelated types the macro cannot connect
// syntactically. With more than one `lift = ...` target declared, a
// `key = ...` variant must say which target it belongs to via `via = ...`.

#[derive(Debug, Clone, thiserror::Error, errlanes::Rejection)]
#[rejection(lift(AViolation, BViolation))]
enum MyRejection {
    #[error("email taken")]
    #[rejection(key = SomeConstraint::EmailKey)]
    EmailTaken,
}

fn main() {}

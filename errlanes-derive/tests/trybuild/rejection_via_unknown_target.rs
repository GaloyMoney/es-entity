// `via = ...` must name one of the enum-level `lift = ...` targets.

#[derive(Debug, Clone, thiserror::Error, errlanes::Rejection)]
#[rejection(lift(AViolation))]
enum MyRejection {
    #[error("email taken")]
    #[rejection(key = SomeConstraint::EmailKey, via = NotDeclared)]
    EmailTaken,
}

fn main() {}

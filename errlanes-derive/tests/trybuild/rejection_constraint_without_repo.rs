#[derive(Debug, Clone, thiserror::Error, errlanes::Rejection)]
enum MyRejection {
    #[error("email taken")]
    #[rejection(constraint = SomeConstraint::EmailKey)]
    EmailTaken,
}

fn main() {}

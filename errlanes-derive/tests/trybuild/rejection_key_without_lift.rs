#[derive(Debug, Clone, thiserror::Error, errlanes::Rejection)]
enum MyRejection {
    #[error("email taken")]
    #[rejection(key = SomeConstraint::EmailKey)]
    EmailTaken,
}

fn main() {}

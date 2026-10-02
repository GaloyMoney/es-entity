struct CustomerRejection;

impl From<upstream::DepositRejection> for CustomerRejection {
    fn from(_: upstream::DepositRejection) -> Self {
        CustomerRejection
    }
}

impl From<errlanes::Fail<upstream::DepositRejection>> for errlanes::Fail<CustomerRejection> {
    fn from(f: errlanes::Fail<upstream::DepositRejection>) -> Self {
        f.widen()
    }
}

fn main() {}

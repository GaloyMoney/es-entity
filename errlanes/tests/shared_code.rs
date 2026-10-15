use std::error::Error;

use errlanes::{Fail, Level, Rejection, RejectionCode, ResultExt, lanes};

#[allow(dead_code)]
#[derive(Debug, errlanes::Rejection)]
enum Cala {
    #[rejection(code = "CALA_DUPLICATE_ID")]
    DuplicateId(u64),
    #[rejection(code = "CALA_CODE_ALREADY_EXISTS")]
    CodeAlreadyExists(#[source] CodeExists),
    #[rejection(code = "CALA_EXTERNAL_ID_ALREADY_EXISTS")]
    ExternalIdAlreadyExists(#[source] ExternalIdExists),
    #[rejection(code = "CALA_OTHER")]
    Other,
}

#[derive(Debug)]
struct CodeExists(String);

impl std::fmt::Display for CodeExists {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "code {} exists", self.0)
    }
}

impl Error for CodeExists {}

#[derive(Debug)]
struct ExternalIdExists(String);

impl std::fmt::Display for ExternalIdExists {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "external id {} exists", self.0)
    }
}

impl Error for ExternalIdExists {}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[rejection(
    code = "HOLDING_ACCOUNT_ALREADY_EXISTS",
    description = "A holding account with this id or reference already exists."
)]
#[lift(Cala, unhandled = fatal)]
enum CreateAccountRejection {
    #[error("holding account id {0} already exists")]
    #[lift(Cala::DuplicateId)]
    DuplicateId(u64),
    #[error("{0}")]
    #[lift(Cala::CodeAlreadyExists)]
    CodeAlreadyExists(#[source] CodeExists),
    #[error("{0}")]
    #[lift(Cala::ExternalIdAlreadyExists)]
    ExternalIdAlreadyExists(#[source] ExternalIdExists),
}

fn code<R: Rejection>(r: &R) -> &'static str {
    r.code().into()
}

#[test]
fn shared_code_reports_once_in_catalogue() {
    assert_eq!(CreateAccountRejectionCode::CODES.len(), 1);
    assert_eq!(
        CreateAccountRejectionCode::ALL,
        &["HOLDING_ACCOUNT_ALREADY_EXISTS"]
    );
    let entry = <CreateAccountRejectionCode as RejectionCode>::CODES[0];
    assert_eq!(
        entry.description,
        Some("A holding account with this id or reference already exists.")
    );
    assert_eq!(
        CreateAccountRejection::DuplicateId(1).code().to_string(),
        "HOLDING_ACCOUNT_ALREADY_EXISTS"
    );
}

#[test]
fn shared_code_variant_display_is_per_variant() {
    assert_eq!(
        CreateAccountRejection::DuplicateId(7).to_string(),
        "holding account id 7 already exists"
    );
    let v = CreateAccountRejection::CodeAlreadyExists(CodeExists("X".into()));
    assert_eq!(v.to_string(), "code X exists");
    assert_eq!(v.source().unwrap().to_string(), "code X exists");
    assert_eq!(
        code(&CreateAccountRejection::ExternalIdAlreadyExists(
            ExternalIdExists("e".into())
        )),
        "HOLDING_ACCOUNT_ALREADY_EXISTS"
    );
}

#[test]
fn lift_arm_takes_local_shared_code_over_source_code() {
    let src: Result<(), Fail<Cala, lanes!(Fatal)>> = Err(Fail::Rejected(Cala::DuplicateId(3)));
    let lifted: Result<(), Fail<CreateAccountRejection, lanes!(Fatal)>> = src.lift();
    let r = lifted.unwrap_err().rejected().unwrap();
    assert!(matches!(r, CreateAccountRejection::DuplicateId(3)));
    assert_eq!(code(&r), "HOLDING_ACCOUNT_ALREADY_EXISTS");

    let src: Result<(), Fail<Cala, lanes!(Fatal)>> = Err(Fail::Rejected(Cala::Other));
    let lifted: Result<(), Fail<CreateAccountRejection, lanes!(Fatal)>> = src.lift();
    assert!(matches!(lifted.unwrap_err(), Fail::Fatal(_)));
}

#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "PARTNER_NOT_ACTIVE")]
#[error("partner not active")]
struct PartnerNotActive;

#[derive(Debug)]
struct DecodeError;
impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("decode")
    }
}
impl Error for DecodeError {}

#[derive(Debug)]
struct LengthError;
impl std::fmt::Display for LengthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("length")
    }
}
impl Error for LengthError {}

#[derive(Debug, errlanes::Rejection)]
#[rejection(
    code = "PARTNER_API_KEY_CREDENTIALS_REJECTED",
    description = "The presented API key credentials are not usable.",
    level = "info"
)]
enum ApiKeyCredentialsRejected {
    #[rejection(from)]
    #[error("the presented secret is not validly encoded")]
    InvalidEncoding(#[source] DecodeError),
    #[rejection(from)]
    #[error("the presented secret has the wrong length")]
    InvalidLength(#[source] LengthError),
    #[error("no api key matches the presented secret")]
    Unknown,
    #[rejection(level = "error")]
    #[error("the api key is not usable")]
    Inactive,
    #[rejection(delegate, from)]
    #[error("{0}")]
    PartnerNotActive(PartnerNotActive),
}

#[test]
fn delegate_variant_keeps_its_own_code_under_shared_code() {
    assert_eq!(ApiKeyCredentialsRejectedCode::CODES.len(), 2);
    assert_eq!(
        ApiKeyCredentialsRejectedCode::ALL,
        &["PARTNER_API_KEY_CREDENTIALS_REJECTED", "PARTNER_NOT_ACTIVE"]
    );
    assert_eq!(
        code(&ApiKeyCredentialsRejected::PartnerNotActive(
            PartnerNotActive
        )),
        "PARTNER_NOT_ACTIVE"
    );
    assert_eq!(
        code(&ApiKeyCredentialsRejected::Unknown),
        "PARTNER_API_KEY_CREDENTIALS_REJECTED"
    );
}

#[test]
fn shared_level_is_the_default_and_a_variant_level_wins() {
    assert_eq!(ApiKeyCredentialsRejected::Unknown.level(), Level::Info);
    assert_eq!(ApiKeyCredentialsRejected::Inactive.level(), Level::Error);
}

#[test]
fn from_on_two_variants_emits_two_from_impls() {
    assert!(matches!(
        ApiKeyCredentialsRejected::from(DecodeError),
        ApiKeyCredentialsRejected::InvalidEncoding(_)
    ));
    assert!(matches!(
        ApiKeyCredentialsRejected::from(LengthError),
        ApiKeyCredentialsRejected::InvalidLength(_)
    ));
    assert!(matches!(
        ApiKeyCredentialsRejected::from(PartnerNotActive),
        ApiKeyCredentialsRejected::PartnerNotActive(_)
    ));
}

#[errlanes::compose(ApiKeyCredentialsRejected as Key)]
#[derive(Debug)]
enum Gateway {
    #[error("local")]
    #[rejection(code = "GATEWAY_LOCAL")]
    Local,
}

#[test]
fn compose_imports_shared_code_family() {
    let imported = Gateway::from(ApiKeyCredentialsRejected::Unknown);
    assert_eq!(code(&imported), "PARTNER_API_KEY_CREDENTIALS_REJECTED");
    assert_eq!(imported.level(), Level::Info);
    let delegated = Gateway::from(ApiKeyCredentialsRejected::PartnerNotActive(
        PartnerNotActive,
    ));
    assert_eq!(code(&delegated), "PARTNER_NOT_ACTIVE");
    assert_eq!(code(&Gateway::Local), "GATEWAY_LOCAL");
}

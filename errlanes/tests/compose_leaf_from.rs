use errlanes::Rejection;

#[derive(Debug, errlanes::Rejection)]
#[error("member {0} already added")]
#[rejection(code = "MEMBER_ALREADY_ADDED", level = "warn")]
struct MemberAlreadyAdded(u64);

#[derive(Debug, errlanes::Rejection)]
enum Accounts {
    #[rejection(delegate, from)]
    Member(MemberAlreadyAdded),
}

#[errlanes::compose(Accounts)]
#[derive(Debug)]
enum WithLeafFrom {}

#[errlanes::compose(Accounts(no_leaf_from))]
#[derive(Debug)]
enum WithoutLeafFrom {}

#[errlanes::compose(Accounts(no_from))]
#[derive(Debug)]
enum CasesOnly {}

#[test]
fn leaf_from_is_on_by_default_and_keeps_the_leaf_metadata() {
    let error = WithLeafFrom::from(MemberAlreadyAdded(1));
    assert!(matches!(error, WithLeafFrom::Member(_)));
    assert_eq!(
        Into::<&'static str>::into(error.code()),
        "MEMBER_ALREADY_ADDED"
    );
    assert_eq!(error.level(), errlanes::Level::Warn);
    assert!(std::error::Error::source(&error).is_some());
}

#[test]
fn no_leaf_from_keeps_the_family_conversion() {
    let error = WithoutLeafFrom::from(Accounts::Member(MemberAlreadyAdded(2)));
    assert!(matches!(error, WithoutLeafFrom::Member(_)));
}

#[test]
fn no_from_imports_the_cases_only() {
    let _ = CasesOnly::Member(MemberAlreadyAdded(3));
}

#[derive(Debug, errlanes::Rejection)]
enum Sets {
    #[rejection(delegate, from)]
    Set(MemberAlreadyAdded),
}

#[errlanes::compose(Accounts, Sets as Other(no_leaf_from))]
#[derive(Debug)]
enum OneOptedOut {}

#[errlanes::compose(Accounts, Sets as Other)]
#[derive(Debug)]
enum Merged {
    #[compose(merge(Accounts::Member, Sets::Set))]
    #[error("{0}")]
    #[rejection(delegate, from)]
    Member(MemberAlreadyAdded),
}

#[test]
fn two_sources_sharing_a_payload_need_a_merge_or_an_opt_out() {
    assert!(matches!(
        OneOptedOut::from(MemberAlreadyAdded(4)),
        OneOptedOut::Member(_)
    ));
    assert!(matches!(
        Merged::from(MemberAlreadyAdded(5)),
        Merged::Member(_)
    ));
}

#[errlanes::compose(Accounts, Sets as Other(no_leaf_from))]
#[derive(Debug)]
enum Mixed {}

#[test]
fn the_three_modifier_states_coexist_in_one_attribute() {
    #[errlanes::compose(Accounts(no_from), Sets as Other(no_leaf_from))]
    #[derive(Debug)]
    enum Coexist {}
    let _ = Coexist::Member(MemberAlreadyAdded(6));
    let _ = Mixed::from(MemberAlreadyAdded(7));
}

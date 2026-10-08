//! The code catalogue: `RejectionCode::CODES` is complete through every
//! forwarding form, deduplicated (first occurrence wins), in declaration
//! order, and carries a description per leaf.

use errlanes::{CodeInfo, Rejection, RejectionCode};

fn entry(code: &'static str, description: Option<&'static str>) -> CodeInfo {
    CodeInfo { code, description }
}

fn codes<R: Rejection>() -> Vec<&'static str> {
    <R::Code as RejectionCode>::CODES
        .iter()
        .map(|e| e.code)
        .collect()
}

/// The property the whole feature rests on: whatever a value resolves to is
/// listed in its type's catalogue. Copy this into a consumer's tests.
fn assert_catalogued<R: Rejection>(value: &R) {
    let code: &'static str = value.code().into();
    assert!(
        <R::Code as RejectionCode>::CODES
            .iter()
            .any(|e| e.code == code),
        "{code} is missing from the catalogue {:?}",
        codes::<R>()
    );
}

// 1. struct leaf --------------------------------------------------------

#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "A_CODE")]
#[error("the literal")]
struct ALeaf;

#[test]
fn struct_leaf_carries_its_literal() {
    assert_eq!(
        <ALeafCode as RejectionCode>::CODES,
        &[entry("A_CODE", Some("the literal"))]
    );
    assert_eq!(ALeafCode::ALL, &["A_CODE"]);
    assert_catalogued(&ALeaf);
}

// 2. mixed descriptions -------------------------------------------------

#[derive(Debug, errlanes::Rejection)]
enum Mixed {
    #[rejection(description = "overridden")]
    #[error("ignored in favour of the override")]
    Over,
    #[error("{field} is bad")]
    Interpolating {
        field: String,
    },
    Silent,
}

#[derive(Debug, errlanes::Rejection)]
#[rejection(error = manual)]
enum Manual {
    Bare,
    #[rejection(description = "documented by hand")]
    Documented,
}
impl std::fmt::Display for Manual {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("manual")
    }
}
impl std::error::Error for Manual {}

#[test]
fn descriptions_are_override_then_verbatim_literal_then_none() {
    assert_eq!(
        <MixedCode as RejectionCode>::CODES,
        &[
            entry("OVER", Some("overridden")),
            entry("INTERPOLATING", Some("{field} is bad")),
            entry("SILENT", None),
        ]
    );
    assert_eq!(
        <ManualCode as RejectionCode>::CODES,
        &[
            entry("BARE", None),
            entry("DOCUMENTED", Some("documented by hand"))
        ]
    );
    assert_catalogued(&Mixed::Over);
    assert_catalogued(&Mixed::Interpolating { field: "x".into() });
    assert_catalogued(&Mixed::Silent);
    assert_catalogued(&Manual::Bare);
    assert_catalogued(&Manual::Documented);
}

// 3. delegation is complete, in order ------------------------------------

#[derive(Debug, errlanes::Rejection)]
enum LeafEnum {
    #[error("first")]
    First,
    #[error("second")]
    Second,
}

#[derive(Debug, errlanes::Rejection)]
enum Family {
    #[rejection(delegate, from)]
    A(ALeaf),
    #[rejection(delegate, from)]
    B(LeafEnum),
}

#[test]
fn delegation_lists_every_forwarded_code_in_order() {
    assert_eq!(
        <FamilyCode as RejectionCode>::CODES,
        &[
            entry("A_CODE", Some("the literal")),
            entry("FIRST", Some("first")),
            entry("SECOND", Some("second")),
        ]
    );
    assert_eq!(FamilyCode::ALL, &["A_CODE", "FIRST", "SECOND"]);
    assert_catalogued(&Family::from(ALeaf));
    assert_catalogued(&Family::from(LeafEnum::First));
    assert_catalogued(&Family::from(LeafEnum::Second));
}

// 4. diamond dedupes, first wins ------------------------------------------

#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "CUSTOMER_NOT_FOUND")]
#[error("no such customer")]
struct CustomerNotFound;

#[derive(Debug, errlanes::Rejection)]
enum SubjectNotFound {
    #[rejection(delegate, from)]
    Customer(CustomerNotFound),
    #[error("no such user")]
    User,
}

#[derive(Debug, errlanes::Rejection)]
enum Schedule {
    #[rejection(delegate, from)]
    Subject(SubjectNotFound),
    #[rejection(delegate, from)]
    Customer(CustomerNotFound),
    #[error("bad window")]
    BadWindow,
}

#[test]
fn a_diamond_lists_each_code_once_at_its_first_path() {
    assert_eq!(
        codes::<Schedule>(),
        vec!["CUSTOMER_NOT_FOUND", "USER", "BAD_WINDOW"]
    );
    assert_eq!(
        ScheduleCode::ALL,
        &["CUSTOMER_NOT_FOUND", "USER", "BAD_WINDOW"]
    );
    assert_catalogued(&Schedule::from(CustomerNotFound));
    assert_catalogued(&Schedule::from(SubjectNotFound::User));
    assert_catalogued(&Schedule::BadWindow);
}

// 5./6. code_and_level_from and lift are exact ---------------------------

#[derive(Debug, errlanes::Rejection)]
enum Source {
    #[rejection(code = "SX")]
    #[error("x")]
    X(u32),
    #[rejection(code = "SY")]
    #[error("y")]
    Y(u32),
}

#[derive(Debug, errlanes::Rejection)]
enum Dest {
    #[rejection(code_and_level_from = Source::X)]
    OnlyX(u32),
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Source, unhandled = fatal)]
enum Lifted {
    #[lift(Source::X)]
    OnlyX(u32),
}

#[derive(Debug, errlanes::Rejection)]
enum WholeValue {
    #[lift(Source)]
    Everything(Source),
}

#[test]
fn code_and_level_from_lists_only_the_named_variant() {
    assert_eq!(
        <DestCode as RejectionCode>::CODES,
        &[entry("SX", Some("x"))]
    );
    assert_catalogued(&Dest::OnlyX(1));
}

#[test]
fn an_inferred_lift_lists_only_the_named_variant() {
    assert_eq!(
        <LiftedCode as RejectionCode>::CODES,
        &[entry("SX", Some("x"))]
    );
    assert_catalogued(&Lifted::OnlyX(1));
}

#[test]
fn a_whole_value_lift_lists_every_source_code() {
    assert_eq!(
        <WholeValueCode as RejectionCode>::CODES,
        &[entry("SX", Some("x")), entry("SY", Some("y"))]
    );
    assert_catalogued(&WholeValue::Everything(Source::X(1)));
    assert_catalogued(&WholeValue::Everything(Source::Y(1)));
}

// 7./9. nesting through two levels, depth-first --------------------------

#[derive(Debug, errlanes::Rejection)]
enum Leaves {
    One,
    Two,
    Three,
}

#[derive(Debug, errlanes::Rejection)]
enum Middle {
    #[rejection(delegate, from)]
    Inner(Leaves),
}

#[derive(Debug, errlanes::Rejection)]
enum Outer {
    #[rejection(delegate, from)]
    Mid(Middle),
}

#[test]
fn nesting_flattens_through_two_levels_in_order() {
    assert_eq!(
        <OuterCode as RejectionCode>::CODES.len(),
        3,
        "{:?}",
        codes::<Outer>()
    );
    assert_eq!(codes::<Outer>(), vec!["ONE", "TWO", "THREE"]);
    for leaf in [Leaves::One, Leaves::Two, Leaves::Three] {
        assert_catalogued(&Outer::from(Middle::from(leaf)));
    }
}

// 11. generic reach -----------------------------------------------------

fn names<R: Rejection>() -> Vec<&'static str> {
    <R::Code as RejectionCode>::CODES
        .iter()
        .map(|e| e.code)
        .collect()
}

#[test]
fn a_boundary_reaches_the_catalogue_without_naming_a_code_type() {
    assert_eq!(names::<Family>(), vec!["A_CODE", "FIRST", "SECOND"]);
    assert_eq!(names::<ALeaf>(), vec!["A_CODE"]);
}

// `Classify` in pure mode is the `Rejection` derive under another name,
// `description` included.
#[derive(Debug, errlanes::Classify)]
enum PureViaClassify {
    #[classify(code = "TOO_SMALL", description = "below the minimum")]
    TooSmall,
}

#[test]
fn pure_classify_carries_the_catalogue() {
    assert_eq!(
        <PureViaClassifyCode as RejectionCode>::CODES,
        &[entry("TOO_SMALL", Some("below the minimum"))]
    );
    assert_catalogued(&PureViaClassify::TooSmall);
}

// `compose` expands to the same machinery: prefixed imports stay complete.
#[errlanes::compose(LeafEnum as Leaf, Source as Src)]
#[derive(Debug)]
enum Composed {
    #[error("own")]
    Own,
}

#[test]
fn composition_lists_every_imported_code() {
    // The prefix renames variants, not codes; local variants come first.
    assert_eq!(
        codes::<Composed>(),
        vec!["OWN", "FIRST", "SECOND", "SX", "SY"]
    );
    assert_catalogued(&Composed::Own);
    assert_catalogued(&Composed::from(LeafEnum::Second));
    assert_catalogued(&Composed::from(Source::Y(1)));
}

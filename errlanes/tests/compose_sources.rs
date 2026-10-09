use std::error::Error;

use errlanes::{Fail, Level, Rejection, ResultExt, lanes};

#[derive(Debug, errlanes::Rejection)]
#[error("member {0} already added")]
#[rejection(code = "MEMBER_ALREADY_ADDED", level = "warn")]
struct MemberAlreadyAdded(u64);

#[derive(Debug, errlanes::Rejection)]
enum Accounts {
    #[error("{0}")]
    #[rejection(delegate, from)]
    AlreadyAdded(MemberAlreadyAdded),
    #[error("account {0} missing")]
    #[rejection(code = "ACCOUNT_MISSING")]
    AccountMissing(u64),
}

#[derive(Debug, errlanes::Rejection)]
enum Sets {
    #[error("{0}")]
    #[rejection(delegate, from)]
    AlreadyAdded(MemberAlreadyAdded),
    #[error("different journals")]
    #[rejection(code = "JOURNAL_MISMATCH")]
    JournalMismatch,
    #[error("depth {depth} exceeds {max}")]
    #[rejection(code = "DEPTH", level = "warn")]
    Depth { depth: u32, max: u32 },
}

#[errlanes::compose(Accounts, Sets)]
#[derive(Debug)]
enum AddMember {
    #[compose(merge)]
    #[error("{0}")]
    #[rejection(delegate, from)]
    AlreadyAdded(MemberAlreadyAdded),
    #[error("local rejection")]
    Local,
}

#[test]
fn bare_sources_share_leaf_outcomes_and_import_every_other_shape() {
    for error in [
        AddMember::from(Accounts::AlreadyAdded(MemberAlreadyAdded(1))),
        AddMember::from(Sets::AlreadyAdded(MemberAlreadyAdded(1))),
        AddMember::from(MemberAlreadyAdded(1)),
    ] {
        assert!(matches!(
            error,
            AddMember::AlreadyAdded(MemberAlreadyAdded(1))
        ));
        assert_eq!(
            Into::<&'static str>::into(error.code()),
            "MEMBER_ALREADY_ADDED"
        );
        assert_eq!(error.level(), Level::Warn);
        assert_eq!(error.to_string(), "member 1 already added");
        assert!(error.source().unwrap().is::<MemberAlreadyAdded>());
    }
    let missing = AddMember::from(Accounts::AccountMissing(2));
    assert!(matches!(missing, AddMember::AccountMissing(2)));
    assert_eq!(missing.to_string(), "account 2 missing");
    assert_eq!(
        Into::<&'static str>::into(missing.code()),
        "ACCOUNT_MISSING"
    );
    let journal = AddMember::from(Sets::JournalMismatch);
    assert!(matches!(journal, AddMember::JournalMismatch));
    assert_eq!(
        Into::<&'static str>::into(journal.code()),
        "JOURNAL_MISMATCH"
    );
    let depth = AddMember::from(Sets::Depth { depth: 9, max: 8 });
    assert!(matches!(depth, AddMember::Depth { depth: 9, max: 8 }));
    assert_eq!(depth.to_string(), "depth 9 exceeds 8");
    assert_eq!(Into::<&'static str>::into(depth.code()), "DEPTH");
    assert_eq!(depth.level(), Level::Warn);
    assert_eq!(AddMember::Local.to_string(), "local rejection");
}

#[test]
fn composition_supports_bare_question_mark_and_lane_expansion_without_changing_faults() {
    fn bare() -> Result<(), Fail<AddMember, lanes!(Fatal)>> {
        Err::<(), _>(Accounts::AccountMissing(3))?;
        Ok(())
    }
    assert!(matches!(
        bare(),
        Err(Fail::Rejected(AddMember::AccountMissing(3)))
    ));
    let source: Result<(), Fail<Sets, lanes!(Fatal)>> = Err(Sets::JournalMismatch.into());
    let converted: Result<(), Fail<AddMember, lanes!(Transient, Fatal)>> =
        source.lift::<AddMember>().map_err(Into::into);
    assert!(matches!(
        converted,
        Err(Fail::Rejected(AddMember::JournalMismatch))
    ));
    let source: Result<(), Fail<Accounts, lanes!(Fatal)>> =
        Err(errlanes::Fatal::invariant("broken graph").into());
    let converted: Result<(), Fail<AddMember, lanes!(Transient, Fatal)>> =
        source.lift::<AddMember>().map_err(Into::into);
    assert!(
        matches!(converted, Err(Fail::Fatal(ref fatal)) if fatal.kind == errlanes::FatalKind::Invariant)
    );
}

#[derive(Debug, errlanes::Rejection)]
enum Left {
    #[rejection(code = "LEFT", level = "warn")]
    Missing { id: u64 },
}
#[derive(Debug, errlanes::Rejection)]
enum Right {
    #[rejection(code = "RIGHT", level = "error")]
    Absent { id: u64 },
}

#[errlanes::compose(Left, Right)]
#[derive(Debug)]
enum Renamed {
    #[compose(merge(Left::Missing, Right::Absent))]
    #[rejection(code = "NOT_FOUND", level = "info")]
    #[error("missing {id}")]
    NotFound { id: u64 },
}

#[errlanes::compose(Right, Left)]
#[derive(Debug)]
enum Reversed {
    #[compose(merge(Right::Absent, Left::Missing))]
    #[rejection(code = "NOT_FOUND", level = "info")]
    #[error("missing {id}")]
    NotFound { id: u64 },
}

#[test]
fn explicit_participants_rename_cases_and_choose_canonical_metadata() {
    for error in [
        Renamed::from(Left::Missing { id: 4 }),
        Renamed::from(Right::Absent { id: 4 }),
    ] {
        assert!(matches!(error, Renamed::NotFound { id: 4 }));
        assert_eq!(Into::<&'static str>::into(error.code()), "NOT_FOUND");
        assert_eq!(error.level(), Level::Info);
        assert_eq!(error.to_string(), "missing 4");
    }
    for error in [
        Reversed::from(Left::Missing { id: 4 }),
        Reversed::from(Right::Absent { id: 4 }),
    ] {
        assert!(matches!(error, Reversed::NotFound { id: 4 }));
        assert_eq!(Into::<&'static str>::into(error.code()), "NOT_FOUND");
        assert_eq!(error.level(), Level::Info);
        assert_eq!(error.to_string(), "missing 4");
    }
}

#[errlanes::compose(AddMember)]
#[derive(Debug)]
enum Outer {}

#[errlanes::compose(AddMember as Member)]
#[derive(Debug)]
// Every variant is intentionally prefixed in this composition fixture.
#[allow(clippy::enum_variant_names)]
enum Prefixed {}

#[test]
fn compositions_export_schemas_for_further_bare_and_prefixed_imports() {
    let outer = Outer::from(AddMember::from(MemberAlreadyAdded(5)));
    assert!(matches!(outer, Outer::AlreadyAdded(MemberAlreadyAdded(5))));
    assert_eq!(
        Into::<&'static str>::into(outer.code()),
        "MEMBER_ALREADY_ADDED"
    );
    assert!(outer.source().unwrap().is::<MemberAlreadyAdded>());
    let prefixed = Prefixed::from(AddMember::from(Sets::Depth { depth: 9, max: 8 }));
    assert!(matches!(
        prefixed,
        Prefixed::MemberDepth { depth: 9, max: 8 }
    ));
    assert_eq!(prefixed.level(), Level::Warn);
}

#[errlanes::compose(Left, Sets as Set)]
#[derive(Debug)]
enum Mixed {}

#[test]
fn bare_and_prefixed_sources_coexist_in_one_list() {
    assert!(matches!(
        Mixed::from(Left::Missing { id: 6 }),
        Mixed::Missing { id: 6 }
    ));
    assert!(matches!(
        Mixed::from(Sets::JournalMismatch),
        Mixed::SetJournalMismatch
    ));
}

#[errlanes::compose(Left)]
#[derive(Debug)]
enum BranchA {}
#[errlanes::compose(Left)]
#[derive(Debug)]
enum BranchB {}
#[errlanes::compose(BranchA, BranchB)]
#[derive(Debug)]
enum Diamond {
    #[compose(merge)]
    #[rejection(code = "CANONICAL_MISSING")]
    Missing { id: u64 },
}

#[test]
fn explicit_merge_resolves_a_shared_origin_diamond() {
    for error in [
        Diamond::from(BranchA::from(Left::Missing { id: 7 })),
        Diamond::from(BranchB::from(Left::Missing { id: 7 })),
    ] {
        assert!(matches!(error, Diamond::Missing { id: 7 }));
        assert_eq!(
            Into::<&'static str>::into(error.code()),
            "CANONICAL_MISSING"
        );
    }
}

use errlanes::{Fail, Lift, Rejection, ResultExt, lanes};
use std::{
    error::Error,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Debug)]
struct Diagnostic(Arc<AtomicBool>);

impl Drop for Diagnostic {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[derive(Debug)]
struct Conflict<T> {
    attempted: Option<T>,
    _diagnostic: Diagnostic,
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[rejection(code = "ACCOUNT_EXTERNAL_ID_ALREADY_EXISTS")]
#[error("External id already exists: {0:?}")]
#[lift(Conflict<Option<String>>, field = attempted)]
struct ExternalIdAlreadyExists(Option<Option<String>>);

#[test]
fn projection_moves_the_selected_field_and_drops_the_rest() {
    for attempted in [None, Some(None), Some(Some("external".to_owned()))] {
        let dropped = Arc::new(AtomicBool::new(false));
        let source = Conflict {
            attempted: attempted.clone(),
            _diagnostic: Diagnostic(dropped.clone()),
        };
        // The source needs neither Rejection nor Clone for the generated From.
        let result = ExternalIdAlreadyExists::from(source);
        assert_eq!(result.0, attempted);
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(
            Into::<&str>::into(result.code()),
            "ACCOUNT_EXTERNAL_ID_ALREADY_EXISTS"
        );
    }
}

#[derive(Debug, errlanes::Rejection)]
enum Constraint {
    #[rejection(code = "EXTERNAL_ID_KEY")]
    #[error("External id constraint")]
    ExternalIdKey(Conflict<Option<String>>),
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[lift(Constraint)]
enum PersistRejection {
    #[rejection(delegate, from)]
    #[error("{0}")]
    #[lift(Constraint::ExternalIdKey, into)]
    ExternalId(ExternalIdAlreadyExists),
}

#[test]
fn projected_leaf_composes_with_into_and_delegates_its_own_metadata() {
    let source = Constraint::ExternalIdKey(Conflict {
        attempted: Some(Some("external".into())),
        _diagnostic: Diagnostic(Arc::new(AtomicBool::new(false))),
    });
    let result: Result<(), Fail<PersistRejection, lanes!()>> = Err(source).widen();
    let Fail::Rejected(rejection) = result.unwrap_err();
    assert_eq!(
        Into::<&str>::into(rejection.code()),
        "ACCOUNT_EXTERNAL_ID_ALREADY_EXISTS"
    );
    let leaf = rejection
        .source()
        .unwrap()
        .downcast_ref::<ExternalIdAlreadyExists>()
        .unwrap();
    assert_eq!(leaf.0, Some(Some("external".into())));
    assert_eq!(rejection.level(), leaf.level());
    assert_eq!(rejection.to_string(), leaf.to_string());
}

mod generic_source {
    pub struct Source<'a, T, const N: usize> {
        pub r#type: &'a [T; N],
        pub _unused: String,
    }
}

#[derive(errlanes::Lift)]
#[lift(generic_source::Source<'a, T, N>, field = r#type,)]
struct Borrowed<'a, T, const N: usize>(&'a [T; N])
where
    T: Eq;

#[test]
fn projection_preserves_generics_lifetimes_where_clauses_and_raw_field_names() {
    let values = [String::from("borrowed")];
    let source = generic_source::Source {
        r#type: &values,
        _unused: String::from("discarded"),
    };
    let borrowed = Borrowed::from(source);
    assert!(std::ptr::eq(borrowed.0, &values));
}

struct OwnedSource<T> {
    value: T,
}

#[derive(errlanes::Lift)]
#[lift(OwnedSource<T>, field = value)]
struct Owned<T>(T);

#[test]
fn projection_does_not_require_clone_or_rejection_on_the_payload() {
    struct NonClone(String);
    let owned = Owned::from(OwnedSource {
        value: NonClone("moved".into()),
    });
    assert_eq!(owned.0.0, "moved");
}

#[derive(Debug, errlanes::Rejection)]
#[rejection(code = "INPUT")]
struct Input {
    value: String,
}

#[derive(Debug, errlanes::Rejection, errlanes::Lift)]
#[rejection(code = "PROJECTED")]
#[lift(Input, field = value)]
struct Projected(String);

#[test]
fn generated_from_supplies_total_lift_without_a_fatal_lane() {
    let projected = Projected::lift(Input {
        value: "lift".into(),
    })
    .unwrap();
    assert_eq!(projected.0, "lift");
    let result: Result<(), Fail<Projected, lanes!()>> = Err(Input {
        value: "widen".into(),
    })
    .widen();
    let Fail::Rejected(projected) = result.unwrap_err();
    assert_eq!(projected.0, "widen");
}

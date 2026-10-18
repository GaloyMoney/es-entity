use errlanes::{Fail, Rejection, lanes};

#[derive(Debug, errlanes::Rejection)]
enum Leaf {
    #[rejection(code = "LEAF_DISABLED", level = "warn")]
    Disabled,
}

#[errlanes::compose(Leaf)]
#[derive(Debug)]
enum Middle {
    #[error("middle")]
    Own,
}

#[errlanes::compose(Middle)]
#[derive(Debug)]
enum Top {}

#[test]
fn a_family_converts_from_everything_its_import_converts_from() {
    assert!(matches!(Top::from(Leaf::Disabled), Top::Disabled));
    assert!(matches!(Top::from(Middle::Own), Top::Own));
}

#[test]
fn question_mark_reaches_the_composing_family_from_the_deepest_source() {
    fn run() -> Result<(), Fail<Top, lanes!(Fatal)>> {
        Err::<(), _>(Leaf::Disabled)?;
        Ok(())
    }
    let top = run().unwrap_err().rejected().unwrap();
    assert!(matches!(top, Top::Disabled));
    assert_eq!(Into::<&'static str>::into(top.code()), "LEAF_DISABLED");
    assert_eq!(top.level(), errlanes::Level::Warn);
}

#[errlanes::compose(Middle)]
#[derive(Debug)]
#[lift(Leaf)]
enum Remapped {
    #[lift(Leaf::Disabled)]
    #[rejection(code = "REMAPPED")]
    Gone,
}

#[test]
fn an_own_registration_of_the_deeper_source_replaces_the_transitive_conversion() {
    assert!(matches!(Remapped::from(Leaf::Disabled), Remapped::Gone));
    assert!(matches!(Remapped::from(Middle::Own), Remapped::Own));
}

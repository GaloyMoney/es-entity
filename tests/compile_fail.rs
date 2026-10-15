//! The id source is enforced by the type system: a `FromRepo` `New*` has no
//! route by which a caller-chosen id reaches an insert, a source mismatch
//! between repo and `New*` does not compile in either direction, and the only
//! constructor of an `EntityEvents` container is not public.

#[test]
fn id_source_misuse_does_not_compile() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}

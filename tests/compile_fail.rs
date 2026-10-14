//! The id policy is enforced by the type system: a `Generated` `New*` has no
//! route by which a caller-chosen id reaches an insert, a policy mismatch
//! between repo and `New*` does not compile in either direction, and the only
//! constructor of an `EntityEvents` container is not public.

#[test]
fn id_policy_misuse_does_not_compile() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/*.rs");
}

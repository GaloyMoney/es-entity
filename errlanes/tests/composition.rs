#[test]
fn transitive_composition_and_inferred_conversions() {
    composition_consumer::check();
}

#[test]
fn union_composition_crosses_crates_and_renamed_dependencies() {
    composition_consumer::check_union();
}

#[test]
fn composition_is_transitive_across_crates_and_hops() {
    composition_consumer::check_transitive();
}

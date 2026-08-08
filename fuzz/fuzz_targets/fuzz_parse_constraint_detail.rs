#![no_main]

//! Coverage-guided fuzz target for `parse_constraint_detail_value`.
//!
//! The function parses PostgreSQL unique-violation detail strings, which are
//! attacker-influenced (they echo the rejected value, which may be user input).
//! It does byte-index arithmetic over that input, so we assert it never panics
//! AND that any extracted value is an honest substring bounded by the real
//! markers — not just "doesn't crash".

use libfuzzer_sys::fuzz_target;

use es_entity::parse_constraint_detail_value;

fuzz_target!(|data: &str| {
    if let Some(v) = parse_constraint_detail_value(Some(data)) {
        // Returned value must be a genuine substring of the input.
        assert!(data.contains(&v));
        // The markers that drove the index arithmetic must really be present,
        // and the slice must equal the returned value exactly.
        let start = data.find("=(").expect("start marker") + 2;
        let end = data.rfind(") already").expect("end marker");
        assert!(start <= end);
        assert_eq!(&data[start..end], v.as_str());
    }
});

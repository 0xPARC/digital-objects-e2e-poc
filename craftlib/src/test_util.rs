#[cfg(test)]
pub mod test {

    use std::collections::HashMap;

    use pod2::middleware::{VDSet, Value};

    pub fn mock_vd_set() -> VDSet {
        VDSet::new(6, &[]).unwrap()
    }

    pub fn check_matched_wildcards(
        matched: HashMap<String, Value>,
        expected: HashMap<String, Value>,
    ) {
        assert_eq!(matched.len(), expected.len(), "len");
        for name in expected.keys() {
            assert_eq!(matched[name], expected[name], "{name}");
        }
    }
}

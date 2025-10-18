use std::collections::HashSet;

use pod2::middleware::{Hash, Params, Value, containers::Set};

pub fn set_from_hashes(params: &Params, items: &HashSet<Hash>) -> pod2::middleware::Result<Set> {
    Set::new(
        params.max_depth_mt_containers,
        items.iter().map(|i| Value::from(*i)).collect(),
    )
}

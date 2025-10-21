pub mod macros;

use std::sync::Arc;

use itertools::Itertools;
use pod2::middleware::{CustomPredicateBatch, CustomPredicateRef, Hash, Params};

// Generic data-drive struct for holidng a set of custom predicates built from
// 1 or more batches.
pub struct PredicateDefs {
    pub batches: Vec<Arc<CustomPredicateBatch>>,
    pub batch_ids: Vec<Hash>,
    pub imports: String,
}

impl PredicateDefs {
    // Builds the imports and by_name fields automatically from an array batch
    // code in PODLang.  Later batches will import previous batches automatically,
    // while use statements for external batches can be included using
    // external_batches.
    // This is meant for use on constant PODLang definitions, so it panics on
    // errors.
    pub fn new(params: &Params, batch_code: &[&str], external_defs: &[PredicateDefs]) -> Self {
        let external_imports = external_defs.iter().map(|d| d.imports.clone()).join("\n");
        let external_batches = external_defs.iter().map(|d| d.batches.clone()).concat();

        let mut batches = Vec::<Arc<CustomPredicateBatch>>::new();
        let mut imports = String::new();

        for podlang_code in batch_code {
            let batch = pod2::lang::parse(
                &format!(
                    "{}\n{}\n{}",
                    external_imports,
                    imports.clone(),
                    podlang_code
                ),
                params,
                &[external_batches.clone(), batches.clone()].concat(),
            )
            .unwrap()
            .custom_batch;

            imports += &format!(
                "use batch {} from {:#}\n",
                batch.predicates().iter().map(|p| p.name.clone()).join(", "),
                batch.id()
            );

            batches.push(batch);
        }

        PredicateDefs {
            batch_ids: batches.clone().iter().map(|b| b.id()).collect(),
            batches,
            imports,
        }
    }

    // Finds a predicate by name in any of the included batches.
    pub fn predicate_ref_by_name(&self, pred_name: &str) -> Option<CustomPredicateRef> {
        for batch in self.batches.iter() {
            let found = CustomPredicateBatch::predicate_ref_by_name(batch, pred_name);
            if found.is_some() {
                return found;
            }
        }

        None
    }
}

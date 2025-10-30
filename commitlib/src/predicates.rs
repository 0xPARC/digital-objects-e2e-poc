use pod2::middleware::{CustomPredicateRef, Params};
use pod2utils::PredicateDefs;

use crate::CONSUMED_ITEM_EXTERNAL_NULLIFIER;

pub struct CommitPredicates {
    pub defs: PredicateDefs,

    pub subset_of: CustomPredicateRef,
    pub subset_of_recursive: CustomPredicateRef,
    pub item_def: CustomPredicateRef,
    pub item_key: CustomPredicateRef,

    pub nullifiers: CustomPredicateRef,
    pub nullifiers_empty: CustomPredicateRef,
    pub nullifiers_recursive: CustomPredicateRef,
    pub commit_creation: CustomPredicateRef,
}

impl CommitPredicates {
    pub fn compile(params: &Params) -> Self {
        // maximum allowed:
        // 4 batches
        // 4 predicates per batch
        // 8 arguments per predicate, at most 5 of which are public
        // 5 statements per predicate
        let batch_defs = [
            r#"
            // Generic recursive construction confirming subset.  Relies on the Merkle
            // tree already requiring unique keys (so no inserts on super)
            SubsetOf(sub, super) = OR(
                Equal(sub, {})
                SubsetOfRecursive(sub, super)
            )

            SubsetOfRecursive(sub, super, private: i, smaller) = AND(
                SetContains(super, i)
                SetInsert(sub, smaller, i)
                SubsetOf(smaller, super)
            )

            // Prove proper derivation of item ID from defined inputs
            // The ingredients dict is explicitly allowed to contain more fields
            // for use in item predicates.
            ItemDef(item, ingredients, inputs, key, work) = AND(
                DictContains(ingredients, "inputs", inputs)
                DictContains(ingredients, "key", key)
                HashOf(item, ingredients, work)
            )

            // Helper to expose just the item and key from ItemId calculation.
            // This is just the CreatedItem pattern with some of inupts private.
            ItemKey(item, key, private: ingredients, inputs, work) = AND(
                ItemDef(item, ingredients, inputs, key, work)
            )
            "#,
            &format!(
                r#"
            // Derive nullifiers from items (using a recursive foreach construction)
            // This proves the relationship between an item and its key before using
            // the key to calculate a nullifier.
            Nullifiers(nullifiers, inputs) = OR(
                NullifiersEmpty(nullifiers, inputs)
                NullifiersRecursive(nullifiers, inputs)
            )

            NullifiersEmpty(nullifiers, inputs) = AND(
                Equal(nullifiers, {{}})
                Equal(inputs, {{}})
            )

            NullifiersRecursive(nullifiers, inputs,
                    private: input, nullifier, key, nullifiers_prev, inputs_prev) = AND(
                ItemKey(input, key)
                HashOf(nullifier, key, "{CONSUMED_ITEM_EXTERNAL_NULLIFIER}")
                SetInsert(nullifiers, nullifiers_prev, nullifier)
                SetInsert(inputs, inputs_prev, input)
                Nullifiers(nullifiers_prev, inputs_prev)
            )

            // ZK version of CreatedItem for committing on-chain.
            // Validator/Logger/Archiver needs to maintain 2 append-only
            // sets of items and nullifiers.  New creating is
            // accepted iff:
            // - item is not already in item set
            // - all nullifiers are not already in nullifier set
            // - createdItems is one of the historical item set roots
            CommitCreation(item, nullifiers, created_items,
                    private: ingredients, inputs, key, work) = AND(
                // Prove the item hash includes all of its committed properties
                ItemDef(item, ingredients, inputs, key, work)

                // Prove all inputs are in the created set
                SubsetOf(inputs, created_items)

                // Expose nullifiers for all inputs
                Nullifiers(nullifiers, inputs)
            )
            "#
            ),
        ];

        let defs = PredicateDefs::new(params, &batch_defs, &[]);

        CommitPredicates {
            subset_of: defs.predicate_ref_by_name("SubsetOf").unwrap(),
            subset_of_recursive: defs.predicate_ref_by_name("SubsetOfRecursive").unwrap(),
            item_def: defs.predicate_ref_by_name("ItemDef").unwrap(),
            item_key: defs.predicate_ref_by_name("ItemKey").unwrap(),
            nullifiers: defs.predicate_ref_by_name("Nullifiers").unwrap(),
            nullifiers_empty: defs.predicate_ref_by_name("NullifiersEmpty").unwrap(),
            nullifiers_recursive: defs.predicate_ref_by_name("NullifiersRecursive").unwrap(),
            commit_creation: defs.predicate_ref_by_name("CommitCreation").unwrap(),
            defs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_custom_predicates() {
        let params = Params::default();
        let commit_preds = CommitPredicates::compile(&params);
        assert!(commit_preds.defs.batches.len() == 2);
    }
}

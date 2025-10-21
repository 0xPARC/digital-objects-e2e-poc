use pod2::middleware::{CustomPredicateRef, Params};
use pod2utils::PredicateDefs;

use crate::CONSUMED_ITEM_EXTERNAL_NULLIFIER;

pub struct CommitPredicates {
    pub defs: PredicateDefs,

    pub super_sub_set: CustomPredicateRef,
    pub super_sub_set_recursive: CustomPredicateRef,
    pub item_def: CustomPredicateRef,
    pub item_key: CustomPredicateRef,

    pub nullifiers: CustomPredicateRef,
    pub nullifiers_empty: CustomPredicateRef,
    pub nullifiers_recursive: CustomPredicateRef,
    pub commit_crafting: CustomPredicateRef,
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
            SuperSubSet(super, sub) = OR(
                Equal(sub, {})
                SuperSubSetRecursive(super, sub)
            )

            SuperSubSetRecursive(super, sub, private: i, smaller) = AND(
                SetContains(super, i)
                SetInsert(sub, smaller, i)
                SuperSubSet(super, smaller)
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
            // This is just the CraftedItem pattern with some of inupts private.
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

            NullifiersRecursive(nullifiers, inputs, private: i, n, k, ns, is) = AND(
                ItemKey(i, k)
                HashOf(n, k, "{CONSUMED_ITEM_EXTERNAL_NULLIFIER}")
                SetInsert(nullifiers, ns, n)
                SetInsert(inputs, is, i)
                Nullifiers(ns, is)
            )

            // ZK version of CraftedItem for committing on-chain.
            // Validator/Logger/Archiver needs to maintain 2 append-only
            // sets of items and nullifiers.  New crafting is
            // accepted iff:
            // - item is not already in item set
            // - all nullifiers are not already in nullifier set
            // - createdItems is one of the historical item set roots
            CommitCrafting(item, nullifiers, created_items, private: ingredients, inputs, key, work) = AND(
                // Prove the item hash includes all of its committed properties
                ItemDef(item, ingredients, inputs, key, work)

                // Prove all inputs are in the created set
                SuperSubSet(created_items, inputs)

                // Expose nullifiers for all inputs
                Nullifiers(nullifiers, inputs)
            )
            "#
            ),
        ];

        let defs = PredicateDefs::new(params, &batch_defs, &[]);

        CommitPredicates {
            super_sub_set: defs.predicate_ref_by_name("SuperSubSet").unwrap(),
            super_sub_set_recursive: defs.predicate_ref_by_name("SuperSubSetRecursive").unwrap(),
            item_def: defs.predicate_ref_by_name("ItemDef").unwrap(),
            item_key: defs.predicate_ref_by_name("ItemKey").unwrap(),
            nullifiers: defs.predicate_ref_by_name("Nullifiers").unwrap(),
            nullifiers_empty: defs.predicate_ref_by_name("NullifiersEmpty").unwrap(),
            nullifiers_recursive: defs.predicate_ref_by_name("NullifiersRecursive").unwrap(),
            commit_crafting: defs.predicate_ref_by_name("CommitCrafting").unwrap(),
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

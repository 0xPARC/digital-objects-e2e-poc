use pod2::middleware::{CustomPredicateRef, Params};
use pod2utils::PredicateDefs;

use crate::CONSUMED_ITEM_EXTERNAL_NULLIFIER;

pub struct CommitPredicates {
    pub defs: PredicateDefs,

    pub subset_of: CustomPredicateRef,
    pub subset_of_recursive: CustomPredicateRef,
    pub batch_def: CustomPredicateRef,
    pub item_in_batch: CustomPredicateRef,

    pub item_def: CustomPredicateRef,
    pub all_items_in_batch: CustomPredicateRef,
    pub all_items_in_batch_empty: CustomPredicateRef,
    pub all_items_in_batch_recursive: CustomPredicateRef,

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
            // 1
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

            // Core commitment to a crafting operation:
            // batch = a single hash representing all outputs
            // ingredients = dict with 2 required fields (inputs, keys),
            //   but allowing other fields usable by the item layer
            // keys = root of a dict containing one key per item
            // inputs = root of a set of item IDs of inputs consumed
            // work = opaque value (hash) used by item layer for sequential work
            BatchDef(batch, ingredients, inputs, keys, work) = AND(
                DictContains(ingredients, "inputs", inputs)
                DictContains(ingredients, "keys", keys)
                HashOf(item, ingredients, work)
            )

            // Each item in a batch has an index (likely 0..N, but could be any
            // value) which must correspond to its key.
            // It confirms that the item ID and keys use the same indexes, for
            // consistent nullifiers.
            ItemInBatch(item, batch, private: index, key) = AND(
                HashOf(item, batch, index)
                SetContains(keys, index, key)
            )
            "#,
            // 2
            r#"
            // Predicate constructing the ID of one item from a batch without
            // any reference to the batch,.
            // Each item in a batch has an index (possibly 0..N, but could be
            // any value) which must correspond to the index of its key.
            ItemDef(item, ingredients, inputs, key, work, private: batch, index) = AND(
                BatchDef(batch, ingredients, inputs, keys, work)
                ItemInBatch(item, batch, index)
            )

            // Recursive construction to extract all the individual item IDs
            // from a batch.  They must be 1:1 with the input keys. The `All` in
            // the name means this is a strict predicate, which doesn't allow
            // for extra values in sets. Note that `keys` is public here,
            // because CommitCrafting needs to match it up against the
            // CraftingDef. Note this allows for empty batches.
            AllItemsInBatch(items, batch, keys) = OR(
                AllItemsInBatchEmpty(items, batch, keys)
                AllItemsInBatchRecursive(items, batch, keys)
            )

            AllItemsInBatchEmpty(items, batch, keys) = AND(
                Equal(items, #{})
                Equal(keys, #{})
                // batch is intentionally unconstrained
            )

            AllItemsInBatchRecursive(items, batch, keys) = AND(
                SetInsert(items, prev_items, item)
                DictInsert(keys, prev_keys, index, key)

                // Inlined version of ItemInBatch. Here we need to explicitly
                // set all values, and don't want to pay for an extra statement
                // to do so.
                HashOf(item, batch, index)
                SetContains(keys, index, key)
            )
            "#,
            // 3
            r#"
            // Helper to expose just the item and key from ItemId calculation.
            // This is just the CreatedItem pattern with some of inupts private.
            ItemKey(item, key, private: ingredients, inputs, work) = AND(
                ItemDef(item, ingredients, inputs, key, work)
            )

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
            "#,
            // 4
            &format!(
                r#"
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
                BatchDef(batch, ingredients, inputs, keys, work)

                // Prove that the item set represents all outputs of this batch.
                AllItemsInBatch(items, batch, keys)

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
            batch_def: defs.predicate_ref_by_name("BatchDef").unwrap(),
            item_in_batch: defs.predicate_ref_by_name("ItemInBatch").unwrap(),

            item_def: defs.predicate_ref_by_name("ItemDef").unwrap(),
            all_items_in_batch: defs.predicate_ref_by_name("AllItemsInBatch").unwrap(),
            all_items_in_batch_empty: defs.predicate_ref_by_name("AllItemsInBatchEmpty").unwrap(),
            all_items_in_batch_recursive: defs
                .predicate_ref_by_name("AllItemsInBatchRecursive")
                .unwrap(),

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
        assert!(commit_preds.defs.batches.len() == 4);
    }
}

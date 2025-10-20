use std::{slice, sync::Arc};

use pod2::middleware::{CustomPredicateBatch, CustomPredicateRef, Params};

pub const CONSUMED_ITEM_EXTERNAL_NULLIFIER: &str = "consumed item external nullifier";

pub struct CraftingPredicates {
    pub batches: Vec<Arc<CustomPredicateBatch>>,

    pub super_sub_set: CustomPredicateRef,
    pub super_sub_set_empty: CustomPredicateRef,
    pub super_sub_set_recursive: CustomPredicateRef,
    pub item_def: CustomPredicateRef,

    pub item_key: CustomPredicateRef,
    pub nullifiers: CustomPredicateRef,
    pub nullifiers_empty: CustomPredicateRef,
    pub nullifiers_recursive: CustomPredicateRef,

    pub commit_crafting: CustomPredicateRef,
    pub is_copper: CustomPredicateRef,
}

pub fn custom_predicates() -> CraftingPredicates {
    // maximum allowed:
    // 4 batches
    // 4 predicates per batch
    // 8 arguments per predicate, at most 5 of which are public
    // 5 statements per predicate
    let params = Params::default();
    // The statements in batch0 are substitutes for introduction statements,
    // for testing purposes
    // MainPodBuilder::priv_op doesn't know how to fill in the wildcards if they are
    // unconstrained, so throw in some Equal statements
    let batch0 = pod2::lang::parse(
        r#"
        // Generic recursive construction confirming subset.  Relies on the Merkle
        // tree already requiring unique keys (so no inserts on super)
        SuperSubSet(super, sub) = OR(
            SuperSubSetEmpty(super, sub)
            SuperSubSetRecursive(super, sub)
        )

        // We should be able to inline the first Equal above, but
        // MainPodBuilder.op_statement doesn't know how to fill in a
        // wildcard which isn't constrained.  The dummy Equal
        // here lets us specify the `super` value when building.
        SuperSubSetEmpty(super, sub) = AND(
            Equal(sub, {})
            Equal(super, super)
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
        "#,
        &params,
        &[],
    )
    .unwrap()
    .custom_batch;
    let super_sub_set =
        CustomPredicateBatch::predicate_ref_by_name(&batch0, "SuperSubSet").unwrap();
    let super_sub_set_empty =
        CustomPredicateBatch::predicate_ref_by_name(&batch0, "SuperSubSetEmpty").unwrap();
    let super_sub_set_recursive =
        CustomPredicateBatch::predicate_ref_by_name(&batch0, "SuperSubSetRecursive").unwrap();
    let item_def = CustomPredicateBatch::predicate_ref_by_name(&batch0, "ItemDef").unwrap();

    let batch1 = pod2::lang::parse(
        &format!(
            r#"
        use batch SuperSubSet, _, _, ItemDef from {:#}

        // Helper to expose just the item and key from ItemId calculation.
        // This is just the CraftedItem pattern with some of inupts private.
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

        NullifiersRecursive(nullifiers, inputs, private: i, n, k, ns, is) = AND(
            ItemKey(i, k)
            HashOf(n, k, "consumed item external nullifier")
            SetInsert(nullifiers, ns, n)
            SetInsert(inputs, is, i)
            Nullifiers(ns, is)
        )
        "#,
            &batch0.id()
        ),
        &params,
        slice::from_ref(&batch0),
    )
    .unwrap()
    .custom_batch;

    let item_key = CustomPredicateBatch::predicate_ref_by_name(&batch1, "ItemKey").unwrap();
    let nullifiers = CustomPredicateBatch::predicate_ref_by_name(&batch1, "Nullifiers").unwrap();
    let nullifiers_empty =
        CustomPredicateBatch::predicate_ref_by_name(&batch1, "NullifiersEmpty").unwrap();
    let nullifiers_recursive =
        CustomPredicateBatch::predicate_ref_by_name(&batch1, "NullifiersRecursive").unwrap();

    let batch2 = pod2::lang::parse(
        &format!(
            r#"
        use batch SuperSubSet, _, _, ItemDef from {:#}
        use batch ItemKey, Nullifiers, _, _ from {:#}

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

        // Example of a mined item with no inputs or sequential work.
        // Copper requires working in a copper mine (blueprint="copper") and
        // 10 leading 0s.
        IsCopper(item, private: ingredients, inputs, key, work) = AND(
            ItemDef(item, ingredients, inputs, key, work)
            Equal(inputs, {{}})
            DictContains(ingredients, "blueprint", "copper")
            // TODO input POD: HashInRange(0, 1<<10, ingredients)
        )
        "#,
            &batch0.id(),
            &batch1.id()
        ),
        &params,
        &[batch0.clone(), batch1.clone()],
    )
    .unwrap()
    .custom_batch;

    let commit_crafting =
        CustomPredicateBatch::predicate_ref_by_name(&batch2, "CommitCrafting").unwrap();
    let is_copper = CustomPredicateBatch::predicate_ref_by_name(&batch2, "IsCopper").unwrap();

    CraftingPredicates {
        batches: vec![batch0, batch1, batch2],

        super_sub_set,
        super_sub_set_empty,
        super_sub_set_recursive,
        item_def,

        item_key,
        nullifiers,
        nullifiers_empty,
        nullifiers_recursive,

        commit_crafting,
        is_copper,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use pod2::{
        backends::plonky2::mock::mainpod::MockProver,
        frontend::{MainPodBuilder, Operation},
        lang::parse,
        middleware::{EMPTY_VALUE, RawValue, Statement, VDSet, Value, hash_value},
    };
    use pod2lib::st_custom;

    use super::*;
    use crate::{
        constants::COPPER_BLUEPRINT,
        item::{IngredientsDef, ItemDef},
        util::set_from_hashes,
    };

    fn mock_vd_set() -> VDSet {
        VDSet::new(6, &[]).unwrap()
    }

    fn check_matched_wildcards(matched: HashMap<String, Value>, expected: HashMap<String, Value>) {
        assert_eq!(matched.len(), expected.len(), "len");
        for name in expected.keys() {
            assert_eq!(matched[name], expected[name], "{name}");
        }
    }

    #[test]
    fn test_compile_custom_predicates() {
        let preds = custom_predicates();
        assert!(preds.batches.len() == 3);
    }

    #[test]
    fn test_build_pod_no_inputs() -> anyhow::Result<()> {
        let preds = custom_predicates();
        let batches = &preds.batches;
        let params = Params::default();

        let mut builder = MainPodBuilder::new(&Default::default(), &mock_vd_set());

        // Item recipe constants
        let seed = 0xA34;
        let key = 0xBADC0DE;
        let work = 0xDEADBEEF;
        // TODO: Real mining and sequential work.

        // Pre-calculate hashes and intermediate values.
        let ingredients_def: IngredientsDef = IngredientsDef {
            blueprint: COPPER_BLUEPRINT.to_string(),
            inputs: HashSet::new(),
            seed: RawValue::from(seed),
            key: RawValue::from(key),
        };
        let ingredients_dict = ingredients_def.dict(&params)?;
        let inputs_set = ingredients_def.inputs_set(&params)?;
        let item_def = ItemDef {
            ingredients: ingredients_def.clone(),
            work: RawValue::from(work),
        };
        let item_hash = item_def.item_hash(&params)?;

        // Sets for on-chain commitment
        let nullifiers = set_from_hashes(&params, &HashSet::new())?;
        let created_items = set_from_hashes(
            &params,
            &HashSet::from([
                hash_value(&Value::from("dummy1").raw()),
                hash_value(&Value::from("dummy2").raw()),
            ]),
        )?;

        // Build ItemDef(item, ingredients, inputs, key, work)
        let st_item_def = st_custom!(
            (builder, batches),
            pub ItemDef(
                DictContains(ingredients_dict, "inputs", inputs_set),
                DictContains(ingredients_dict, "key", ingredients_def.key),
                HashOf(item_hash, ingredients_dict, item_def.work),
            )
        );

        // Build ItemKey(item, key)
        let _st_itemkey = st_custom!((builder, batches), pub ItemKey(st_item_def.clone(),));

        let st_inputs_subset_empty = st_custom!(
            (builder, batches),
            SuperSubSetEmpty(
                Equal(inputs_set, EMPTY_VALUE),
                Equal(created_items, created_items),
            )
        );
        // Build SuperSubSet(created_items, inputs)
        let st_inputs_subset = st_custom!(
            (builder, batches),
            pub SuperSubSet(st_inputs_subset_empty, Statement::None,)
        );

        // Build Nullifiers(nullifiers, inputs)
        let st_nullifiers_empty = st_custom!(
            (builder, batches),
            NullifiersEmpty(
                Equal(inputs_set, EMPTY_VALUE),
                Equal(nullifiers, EMPTY_VALUE),
            )
        );
        let st_nullifiers = st_custom!(
            (builder, batches),
            pub Nullifiers(st_nullifiers_empty, Statement::None,)
        );

        // Build CommitCrafting(item, nullifiers, created_items)
        let _st_commit_crafting = st_custom!(
            (builder, batches),
            pub CommitCrafting(st_item_def.clone(), st_inputs_subset, st_nullifiers,)
        );

        // Build IsCopper(item)
        let _st_is_copper = st_custom!(
            (builder, batches),
            pub IsCopper(
                st_item_def,
                Equal(inputs_set, EMPTY_VALUE),
                DictContains(ingredients_dict, "blueprint", COPPER_BLUEPRINT),
            )
        );

        // Prove MainPOD
        let main_pod = builder.prove(&MockProver {})?;
        main_pod.pod.verify()?;
        println!("POD: {}", main_pod);

        // PODLang query to check the final statements.  There are a lot
        // more public statements than in real crafting, to allow confirming
        // all the values.
        let query = format!(
            r#"
            use batch SuperSubSet, _, _, ItemDef from {:#}
            use batch ItemKey, Nullifiers, _, _ from {:#}
            use batch CommitCrafting, IsCopper from {:#}

            REQUEST(
                ItemDef(item, ingredients, inputs, key, work)
                ItemKey(item, key)
                SuperSubSet(created_items, inputs)
                Nullifiers(nullifiers, inputs)
                CommitCrafting(item, nullifiers, created_items)
                IsCopper(item)
            )
            "#,
            &preds.batches[0].id(),
            &preds.batches[1].id(),
            &preds.batches[2].id(),
        );

        println!("Verification request: {query}");

        let request = parse(&query, &params, &preds.batches)?.request;
        let matched_wildcards = request.exact_match_pod(&*main_pod.pod)?;
        check_matched_wildcards(
            matched_wildcards,
            HashMap::from([
                ("item".to_string(), Value::from(item_hash)),
                ("ingredients".to_string(), Value::from(ingredients_dict)),
                ("inputs".to_string(), Value::from(inputs_set)),
                ("key".to_string(), Value::from(key)),
                ("work".to_string(), Value::from(work)),
                ("created_items".to_string(), Value::from(created_items)),
                ("nullifiers".to_string(), Value::from(nullifiers)),
            ]),
        );

        Ok(())
    }
}

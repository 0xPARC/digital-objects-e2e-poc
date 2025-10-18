use std::{slice, sync::Arc};

use pod2::middleware::{CustomPredicateBatch, CustomPredicateRef, Params};

pub struct CraftingPredicates {
    pub batches: Vec<Arc<CustomPredicateBatch>>,

    pub super_sub_set: CustomPredicateRef,
    pub super_sub_set_recursive: CustomPredicateRef,
    pub ingredients: CustomPredicateRef,
    pub item_seed: CustomPredicateRef,

    pub nullifiers: CustomPredicateRef,
    pub nullifiers_empty: CustomPredicateRef,
    pub nullifiers_recursive: CustomPredicateRef,
    pub commit_crafting: CustomPredicateRef,
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
            Equal(sub, {})
            SuperSubSetRecursive(super, sub)
        )

        SuperSubSetRecursive(super, sub, private: i, smaller) = AND(
            SetContains(super, i)
            SetInsert(sub, smaller, i)
            SuperSubSet(super, smaller)
        )

        // Prove ingredients dict was built properly
        Ingredients(ingredients, blueprint, inputs, seed, private: i1, i2) = AND(
            DictInsert(i1, {}, "blueprint", blueprint)
            DictInsert(i2, i1, "inputs", inputs)
            DictInsert(ingredients, i2, "seed", seed)
        )

        // Fully-public construction of item hash from all of its properties.
        // Note that this predicate is not recursively defined.  The inputs
        // set at this level is a Merkle root without further verification.
        // 
        // This statement is always "inlined" elsewhere, so commented out here.
        //CraftedItem(item, ingredients, blueprint, inputs, seed, work) = AND(
        //    Ingredients(ingredients, blueprint, inputs, seed)
        //    HashOf(item, ingredients, work)
        //)

        // Helper to expose just the item and seed from ItemId calculation.
        // This is just the CraftedItem pattern with some of inupts private.
        ItemSeed(item, seed, private: ingredients, blueprint, inputs, work) = AND(
            Ingredients(ingredients, blueprint, inputs, seed)
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
    let super_sub_set_recursive =
        CustomPredicateBatch::predicate_ref_by_name(&batch0, "SuperSubSet").unwrap();
    let ingredients = CustomPredicateBatch::predicate_ref_by_name(&batch0, "Ingredients").unwrap();
    let item_seed = CustomPredicateBatch::predicate_ref_by_name(&batch0, "ItemSeed").unwrap();

    let batch1 = pod2::lang::parse(
        &format!(r#"
        use SuperSubSet, _, Ingredients, ItemSeed from {:#}

        // Derive nullifiers from items (using a recursive foreach construction)
        // This proves the relationship between an item and its seed before using
        // the seed to calculate a nullifier.
        Nullifiers(nullifiers, inputs) = OR(
            NullifiersEmpty(nullifiers, inputs)
            NullifiersRecursive(nullifiers, inputs)
        )

        NullifiersEmpty(nullifiers, inputs) = AND(
            Equal(nullifiers, {{}})
            Equal(inputs, {{}})
        )

        NullifiersRecursive(nullifiers, inputs, private: i, n, s, ns, is) = AND(
            ItemSeed(i, s)
            HashOf(n, s, "externalNullifier")
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
        CommitCrafting(item, nullifiers, createdItemsRoot, private: ingredients, blueprint, inputs, seed, work) = AND(
            // CraftedItem construction
            Ingredients(ingredients, blueprint, inputs, seed)
            HashOf(item, ingredients, work)

            // Prove all inputs are in the created set
            SuperSubSet(createdItemsRoot, inputs)

            // Expose nullifiers for all inputs
            Nullifiers(nullifiers, inputs)
        )
        "#, &batch0.id()),
        &params,
        slice::from_ref(&batch0),
    )
    .unwrap()
    .custom_batch;

    let nullifiers = CustomPredicateBatch::predicate_ref_by_name(&batch1, "Nullifiers").unwrap();
    let nullifiers_empty =
        CustomPredicateBatch::predicate_ref_by_name(&batch1, "NullifiersEmpty").unwrap();
    let nullifiers_recursive =
        CustomPredicateBatch::predicate_ref_by_name(&batch1, "NullifiersRecursive").unwrap();
    let commit_crafting =
        CustomPredicateBatch::predicate_ref_by_name(&batch1, "CommitCrafting").unwrap();

    CraftingPredicates {
        batches: vec![batch0, batch1],

        super_sub_set,
        super_sub_set_recursive,
        ingredients,
        item_seed,

        nullifiers,
        nullifiers_empty,
        nullifiers_recursive,
        commit_crafting,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use pod2::{
        backends::plonky2::mock::mainpod::MockProver,
        dict,
        frontend::{MainPodBuilder, Operation},
        lang::parse,
        middleware::{VDSet, Value, containers::Set, hash_values},
    };

    use super::*;

    fn mock_vd_set() -> VDSet {
        VDSet::new(6, &[]).unwrap()
    }

    #[test]
    fn test_compile_custom_predicates() {
        let preds = custom_predicates();
        assert!(preds.batches.len() == 2);
    }

    #[test]
    fn test_build_item_pod() -> anyhow::Result<()> {
        let preds = custom_predicates();
        let params = Params::default();

        let mut builder = MainPodBuilder::new(&Default::default(), &mock_vd_set());

        // Item recipe.
        let blueprint = "copper";
        let inputs = Set::new(params.max_depth_mt_containers, HashSet::new())?;
        let seed = 123i64;
        let work = 0i64;

        // Pre-calculate hashes and intermediave values.
        let i0 = dict!(params.max_depth_mt_containers, {})?;
        let i1 = dict!(params.max_depth_mt_containers, {
            "blueprint" => blueprint,
        })?;
        let i2 = dict!(params.max_depth_mt_containers, {
            "blueprint" => blueprint,
            "inputs" => inputs.clone(),
        })?;
        let ingredients = dict!(params.max_depth_mt_containers, {
            "blueprint" => "copper",
            "inputs" => inputs.clone(),
            "seed" => seed,
        })?;
        let item_hash = hash_values(&[Value::from(ingredients.clone()), Value::from(work)]);

        // Build Ingredients statement
        let st_i1 = builder.priv_op(Operation::dict_insert(
            Value::from(i1.clone()),
            Value::from(i0),
            Value::from("blueprint"),
            Value::from(blueprint),
        ))?;
        let st_i2 = builder.priv_op(Operation::dict_insert(
            Value::from(i2.clone()),
            Value::from(i1),
            Value::from("inputs"),
            Value::from(inputs.clone()),
        ))?;
        let st_i3 = builder.priv_op(Operation::dict_insert(
            Value::from(ingredients.clone()),
            Value::from(i2),
            Value::from("seed"),
            Value::from(seed),
        ))?;
        let st_ingredients = builder.pub_op(Operation::custom(
            preds.ingredients.clone(),
            [st_i1, st_i2, st_i3],
        ))?;
        let st_item_hash = builder.priv_op(Operation::hash_of(
            Value::from(item_hash),
            Value::from(ingredients.clone()),
            Value::from(work),
        ))?;
        let _st_itemseed = builder.pub_op(Operation::custom(
            preds.item_seed.clone(),
            [st_ingredients, st_item_hash],
        ))?;

        // Prove MainPOD
        let main_pod = builder.prove(&MockProver {})?;
        main_pod.pod.verify()?;
        println!("POD: {:?}", main_pod.pod);

        // PODLang query to check the final statement
        let query = format!(
            r#"
            use _, _, Ingredients, ItemSeed from {:#}

            REQUEST(
                Ingredients(ingredients, blueprint, inputs, seed)
                ItemSeed(item, seed)
            )
            "#,
            &preds.batches[0].id()
        );

        println!("Verification query: {query}");

        let request = parse(&query, &params, &preds.batches)?.request;
        let matched_wildcards = request.exact_match_pod(&*main_pod.pod)?;
        assert_eq!(
            matched_wildcards,
            HashMap::from([
                ("item".to_string(), Value::from(item_hash)),
                ("ingredients".to_string(), Value::from(ingredients)),
                ("blueprint".to_string(), Value::from(blueprint)),
                ("inputs".to_string(), Value::from(inputs)),
                ("seed".to_string(), Value::from(seed)),
            ]),
        );

        Ok(())
    }
}

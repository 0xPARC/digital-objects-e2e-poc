use std::slice;

use commitlib::predicates::CommitPredicates;
use plonky2::field::types::Field;
use pod2::middleware::{CustomPredicateRef, F, Params};
use pod2utils::PredicateDefs;

use crate::constants::{AXE_MINING_MAX, STONE_MINING_MAX, WOOD_MINING_MAX};

/// Convert a u64 difficulty to RawValue format for use in predicates (little-endian)
fn difficulty_to_raw_string(difficulty: u64) -> String {
    let difficulty_f = F::from_canonical_u64(difficulty);
    format!(
        "Raw(0x{:016x}{:016x}{:016x}{:016x})",
        0u64, 0u64, 0u64, difficulty_f.0
    )
}

pub struct ItemPredicates {
    pub defs: PredicateDefs,

    pub is_stone: CustomPredicateRef,
}

impl ItemPredicates {
    pub fn compile(params: &Params, commit_preds: &CommitPredicates) -> Self {
        // maximum allowed:
        // 4 batches
        // 4 predicates per batch
        // 8 arguments per predicate, at most 5 of which are public
        // 5 statements per predicate

        // Convert mining difficulties to RawValue format for predicates
        let stone_difficulty_raw = difficulty_to_raw_string(STONE_MINING_MAX);
        let wood_difficulty_raw = difficulty_to_raw_string(WOOD_MINING_MAX);
        let axe_difficulty_raw = difficulty_to_raw_string(AXE_MINING_MAX);

        let batch_def_1 = format!(
            r#"
            use intro Vdf(count, input, output) from 0x3493488bc23af15ac5fabe38c3cb6c4b66adb57e3898adf201ae50cc57183f65 // vdfpod vd hash
            use intro PoW(hash, difficulty) from 0x42fed42704533123de144a9e820c9d6bdf4c8616f29664111469bd696b628686 // powpod vd hash

            // Example of a mined item with mining difficulty check and VDF work.
            // Stone requires:
            // - blueprint="stone"
            // - hash(ingredients) meets difficulty (PoW mining)
            // - sequential work via VDF
            IsStone(item, private: ingredients, inputs, key, work) = AND(
                ItemDef(item, ingredients, inputs, key, work)
                Equal(inputs, {{}})
                DictContains(ingredients, "blueprint", "stone")
                PoW(ingredients, {stone_difficulty_raw})  // Proves ingredients <= STONE_MINING_MAX
                Vdf(3, ingredients, work)  // Proves 3 iterations of sequential hashing
            )

            // Example of a mined item with just PoW (no VDF work).
            // Wood requires:
            // - blueprint="wood"
            // - hash(ingredients) meets difficulty (PoW mining)
            IsWood(item, private: ingredients, inputs, key, work) = AND(
                ItemDef(item, ingredients, inputs, key, work)
                Equal(inputs, {{}})
                DictContains(ingredients, "blueprint", "wood")
                PoW(ingredients, {wood_difficulty_raw})  // Proves ingredients <= WOOD_MINING_MAX
                Equal(work, {{}})  // No VDF work required
            )
            "#
        );

        let batch_def_2 = format!(
            r#"
            use intro PoW(hash, difficulty) from 0x42fed42704533123de144a9e820c9d6bdf4c8616f29664111469bd696b628686 // powpod vd hash

            AxeInputs(inputs, private: s1, wood, stone) = AND(
                // 2 ingredients
                SetInsert(s1, {{}}, wood)
                SetInsert(inputs, s1, stone)

                // prove the ingredients are correct.
                IsWood(wood)
                IsStone(stone)
            )

            // Combining Stone and Wood to get Axe requires mining (no sequential work).
            IsAxe(item, private: ingredients, inputs, key, work) = AND(
                ItemDef(item, ingredients, inputs, key, work)
                DictContains(ingredients, "blueprint", "axe")
                PoW(ingredients, {axe_difficulty_raw})  // Proves ingredients <= AXE_MINING_MAX
                Equal(work, {{}})

                AxeInputs(inputs)
            )

            // Wooden Axe:
            WoodenAxeInputs(inputs, private: s1, wood1, wood2) = AND(
                // 2 ingredients
                SetInsert(s1, {{}}, wood1)
                SetInsert(inputs, s1, wood2)

                // prove the ingredients are correct.
                IsWood(wood1)
                IsWood(wood2)
            )

            // Combine Wood and Wod to get WoodenAxe.
            IsWoodenAxe(item, private: ingredients, inputs, key, work) = AND(
                ItemDef(item, ingredients, inputs, key, work)
                DictContains(ingredients, "blueprint", "wooden-axe")
                Equal(work, {{}})

                WoodenAxeInputs(inputs)
            )
            "#
        );

        let batch_defs = [batch_def_1, batch_def_2];
        let batch_defs_refs: Vec<&str> = batch_defs.iter().map(|s| s.as_str()).collect();
        let defs = PredicateDefs::new(
            params,
            &batch_defs_refs,
            slice::from_ref(&commit_preds.defs),
        );

        ItemPredicates {
            is_stone: defs.predicate_ref_by_name("IsStone").unwrap(),
            defs,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use commitlib::{IngredientsDef, ItemDef, util::set_from_hashes};
    use pod2::{
        backends::plonky2::mock::mainpod::MockProver,
        frontend::{MainPod, MainPodBuilder, Operation},
        lang::parse,
        middleware::{EMPTY_VALUE, Pod, RawValue, Statement, Value, hash_value},
    };

    use super::*;
    use crate::{
        constants::STONE_BLUEPRINT,
        test_util::test::{check_matched_wildcards, mock_vd_set},
        vdfpod::VdfPod,
    };

    #[test]
    fn test_compile_custom_predicates() {
        let params = Params::default();
        let commit_preds = CommitPredicates::compile(&params);
        assert!(commit_preds.defs.batches.len() == 2);

        let item_preds = ItemPredicates::compile(&params, &commit_preds);
        assert!(item_preds.defs.batches.len() == 2);
    }

    #[test]
    fn test_build_pod_no_inputs() -> anyhow::Result<()> {
        let params = Params::default();
        let commit_preds = CommitPredicates::compile(&params);
        let item_preds = ItemPredicates::compile(&params, &commit_preds);

        let mut builder = MainPodBuilder::new(&Default::default(), &mock_vd_set());

        // Item recipe constants
        let seed: i64 = 0xA34;
        let key = 0xBADC0DE;

        // Pre-calculate hashes and intermediate values.
        let ingredients_def: IngredientsDef = IngredientsDef {
            inputs: HashSet::new(),
            key: RawValue::from(key),
            app_layer: HashMap::from([
                ("blueprint".to_string(), Value::from(STONE_BLUEPRINT)),
                ("seed".to_string(), Value::from(seed)),
            ]),
        };
        let ingredients_dict = ingredients_def.dict(&params)?;
        let inputs_set = ingredients_def.inputs_set(&params)?;
        // compute the VdfPod
        let vd_set = &mock_vd_set();
        let vdf_pod = VdfPod::new(
            &params,
            vd_set.clone(),
            3,
            RawValue::from(ingredients_def.dict(&params)?.commitment()),
        )?;
        let main_vdf_pod = MainPod {
            pod: Box::new(vdf_pod.clone()),
            public_statements: vdf_pod.pub_statements(),
            params: params.clone(),
        };
        let work: RawValue = vdf_pod.output;
        let st_vdf = main_vdf_pod.public_statements[0].clone();
        builder.add_pod(main_vdf_pod);
        let item_def = ItemDef {
            ingredients: ingredients_def.clone(),
            work,
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
        let st_contains_inputs = builder.priv_op(Operation::dict_contains(
            ingredients_dict.clone(),
            "inputs",
            inputs_set.clone(),
        ))?;
        let st_contains_key = builder.priv_op(Operation::dict_contains(
            ingredients_dict.clone(),
            "key",
            ingredients_def.key,
        ))?;
        let st_item_hash = builder.priv_op(Operation::hash_of(
            item_hash,
            ingredients_dict.clone(),
            item_def.work,
        ))?;
        let st_item_def = builder.pub_op(Operation::custom(
            commit_preds.item_def.clone(),
            [st_contains_inputs, st_contains_key, st_item_hash],
        ))?;

        // Build ItemKey(item, key)
        let _st_itemkey = builder.pub_op(Operation::custom(
            commit_preds.item_key.clone(),
            [st_item_def.clone()],
        ))?;

        // Build SubsetOf(inputs, created_items)
        // We use builder.op() to manually specify the `super` wildcard value
        // because it's otherwise unconstrained.  This is only relevant in
        // the base case where `sub` is empty, which is a subset of anything.
        let st_inputs_eq_empty = builder.priv_op(Operation::eq(inputs_set.clone(), EMPTY_VALUE))?;
        let st_inputs_subset = builder.op(
            true, /*public*/
            vec![(1, Value::from(created_items.clone()))],
            Operation::custom(
                commit_preds.subset_of.clone(),
                [st_inputs_eq_empty.clone(), Statement::None],
            ),
        )?;

        // Build Nullifiers(nullifiers, inputs)
        let st_nullifiers_eq_empty =
            builder.priv_op(Operation::eq(nullifiers.clone(), EMPTY_VALUE))?;
        let st_nullifiers_empty = builder.pub_op(Operation::custom(
            commit_preds.nullifiers_empty.clone(),
            [st_inputs_eq_empty.clone(), st_nullifiers_eq_empty],
        ))?;
        let st_nullifiers = builder.pub_op(Operation::custom(
            commit_preds.nullifiers.clone(),
            [st_nullifiers_empty, Statement::None],
        ))?;

        // Build CommitCreation(item, nullifiers, created_items)
        let _st_commit_crafting = builder.pub_op(Operation::custom(
            commit_preds.commit_creation.clone(),
            [st_item_def.clone(), st_inputs_subset, st_nullifiers],
        ))?;

        // Build IsStone(item)
        let st_contains_blueprint = builder.priv_op(Operation::dict_contains(
            ingredients_dict.clone(),
            "blueprint",
            Value::from(STONE_BLUEPRINT),
        ))?;
        let _st_is_stone = builder.pub_op(Operation::custom(
            item_preds.is_stone.clone(),
            [
                st_item_def,
                st_inputs_eq_empty,
                st_contains_blueprint,
                st_vdf,
            ],
        ))?;

        // Prove MainPOD
        let main_pod = builder.prove(&MockProver {})?;
        main_pod.pod.verify()?;
        println!("POD: {:?}", main_pod.pod);

        // PODLang query to check the final statements.  There are a lot
        // more public statements than in real crafting, to allow confirming
        // all the values.
        let query = format!(
            r#"
            {}
            {}

            REQUEST(
                ItemDef(item, ingredients, inputs, key, work)
                ItemKey(item, key)
                SubsetOf(inputs, created_items)
                Nullifiers(nullifiers, inputs)
                CommitCreation(item, nullifiers, created_items)
                IsStone(item)
            )
            "#,
            &commit_preds.defs.imports, &item_preds.defs.imports,
        );

        println!("Verification request: {query}");

        let request = parse(
            &query,
            &params,
            &[commit_preds.defs.batches, item_preds.defs.batches].concat(),
        )?
        .request;
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

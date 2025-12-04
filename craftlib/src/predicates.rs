use std::slice;

use commitlib::predicates::CommitPredicates;
use pod2::middleware::{CustomPredicateRef, Params};
use pod2utils::PredicateDefs;

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
        let batch_defs = [
            r#"
            use intro Pow(count, input, output) from 0x3493488bc23af15ac5fabe38c3cb6c4b66adb57e3898adf201ae50cc57183f65 // powpod vd hash
        
            // Example of a mined item with no inputs or sequential work.
            // Stone requires working in a stone mine (blueprint="stone") and
            // 10 leading 0s.
            IsStone(item, private: ingredients, inputs, key, work) = AND(
                ItemDef(item, ingredients, inputs, key, work)
                Equal(inputs, {})
                DictContains(ingredients, "blueprint", "stone")
                Pow(3, ingredients, work)
            )
        
            // Example of a mined item which is more common but takes more work to
            // extract.
            IsWood(item, private: ingredients, inputs, key, work) = AND(
                ItemDef(item, ingredients, inputs, key, work)
                Equal(inputs, {})
                DictContains(ingredients, "blueprint", "wood")
                Equal(work, {})
                // TODO input POD: SequentialWork(ingredients, work, 5)
                // TODO input POD: HashInRange(0, 1<<5, ingredients)
            )

            AxeInputs(inputs, private: s1, wood, stone) = AND(
                // 2 ingredients
                SetInsert(s1, {}, wood)
                SetInsert(inputs, s1, stone)
        
                // prove the ingredients are correct.
                IsWood(wood)
                IsStone(stone)
            )
        
            // Combining Stone and Wood to get Axe is easy (no sequential work).
            // TODO: Require a smelter as a tool
            IsAxe(item, private: ingredients, inputs, key, work) = AND(
                ItemDef(item, ingredients, inputs, key, work)
                DictContains(ingredients, "blueprint", "axe")
                Equal(work, {})
        
                AxeInputs(inputs)
            )
            "#,
            r#"
        
            // Wooden Axe:
            WoodenAxeInputs(inputs, private: s1, wood1, wood2) = AND(
                // 2 ingredients
                SetInsert(s1, {}, wood1)
                SetInsert(inputs, s1, wood2)
        
                // prove the ingredients are correct.
                IsWood(wood1)
                IsWood(wood2)
            )

            // Combine Wood and Wod to get WoodenAxe.
            IsWoodenAxe(item, private: ingredients, inputs, key, work) = AND(
                ItemDef(item, ingredients, inputs, key, work)
                DictContains(ingredients, "blueprint", "wooden-axe")
                Equal(work, {})
        
                WoodenAxeInputs(inputs)
            )


            // multi-output related predicates:
            // (simplified version without tools & durability)
            // disassemble 2 Stones into 2 outputs: Dust,Gravel.
            
            // inputs: 2 Stones
            StoneDisassembleInputs(inputs, private: s1, stone1, stone2) = AND(
                SetInsert(s1, {}, stone1)
                SetInsert(inputs, s1, stone2)

                // prove the ingredients are correct
                IsStone(stone1)
                IsStone(stone2)
            )

            // outputs: 1 Dust, 1 Gravel
            StoneDisassembleOutputs(inputs,
                    private: batch, keys, k1, dust, gravel, _dust_key, _gravel_key) = AND(
                HashOf(dust, batch, "dust")
                HashOf(gravel, batch, "gravel")
                DictInsert(k1, {}, "dust", _dust_key)
                DictInsert(keys, k1, "gravel", _gravel_key)
            )
            "#,
            r#"

            // helper to have a single predicate for the inputs & outputs
            StoneDisassembleInputsOutputs(inputs) = AND (
                StoneDisassembleInputs(inputs)
                StoneDisassembleOutputs(inputs)
            )

            StoneDisassemble(inputs,
                    private: batch, keys, ingredients, work) = AND(
                BatchDef(batch, ingredients, inputs, keys, work)
                DictContains(ingredients, "blueprint", "dust")
                DictContains(ingredients, "blueprint", "gravel")

                StoneDisassembleInputsOutputs(inputs)
            )

            // can only obtain Dust from disassembling 2 stones
            IsDust(item, private: ingredients, inputs, key, work) = AND(
                ItemDef(item, ingredients, inputs, key, work)
                DictContains(ingredients, "blueprint", "dust")
                Equal(work, {})
        
                StoneDisassemble(inputs)
            )

            // can only obtain Gravel from disassembling 2 stones
            IsGravel(item, private: ingredients, inputs, key, work, dust, gravel) = AND(
                ItemDef(item, ingredients, inputs, key, work)
                DictContains(ingredients, "blueprint", "gravel")
                Equal(work, {})
        
                StoneDisassemble(inputs)
            )
            "#,
        ];
        let defs = PredicateDefs::new(params, &batch_defs, slice::from_ref(&commit_preds.defs));

        ItemPredicates {
            is_stone: defs.predicate_ref_by_name("IsStone").unwrap(),
            defs,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use commitlib::{BatchDef, IngredientsDef, ItemDef, util::set_from_hashes};
    use pod2::{
        backends::plonky2::mock::mainpod::MockProver,
        frontend::{MainPod, MainPodBuilder, Operation},
        lang::parse,
        middleware::{
            EMPTY_VALUE, Key, Pod, RawValue, Statement, Value,
            containers::{Dictionary, Set},
            hash_value,
        },
    };

    use super::*;
    use crate::{
        constants::STONE_BLUEPRINT,
        powpod::PowPod,
        test_util::test::{check_matched_wildcards, mock_vd_set},
    };

    #[test]
    fn test_compile_custom_predicates() {
        let params = Params::default();
        let commit_preds = CommitPredicates::compile(&params);
        assert!(commit_preds.defs.batches.len() == 4);

        let item_preds = ItemPredicates::compile(&params, &commit_preds);
        assert!(item_preds.defs.batches.len() == 3);
    }

    #[test]
    fn test_build_pod_no_inputs() -> anyhow::Result<()> {
        let params = Params::default();
        let commit_preds = CommitPredicates::compile(&params);
        let item_preds = ItemPredicates::compile(&params, &commit_preds);

        let mut builder = MainPodBuilder::new(&Default::default(), &mock_vd_set());

        // Item recipe constants
        let seed: i64 = 0xA34;
        let index: Key = "0".into();
        let key = 0xBADC0DE;

        // Pre-calculate hashes and intermediate values.
        let ingredients_def: IngredientsDef = IngredientsDef {
            inputs: HashSet::new(),
            keys: [(index.clone(), Value::from(key))].into_iter().collect(),
            app_layer: HashMap::from([
                ("blueprint".to_string(), Value::from(STONE_BLUEPRINT)),
                ("seed".to_string(), Value::from(seed)),
            ]),
        };
        let ingredients_dict = ingredients_def.dict(&params)?;
        let inputs_set = ingredients_def.inputs_set(&params)?;
        // compute the PowPod
        let vd_set = &mock_vd_set();
        let pow_pod = PowPod::new(
            &params,
            vd_set.clone(),
            3,
            RawValue::from(ingredients_def.dict(&params)?.commitment()),
        )?;
        let main_pow_pod = MainPod {
            pod: Box::new(pow_pod.clone()),
            public_statements: pow_pod.pub_statements(),
            params: params.clone(),
        };
        let work: RawValue = pow_pod.output;
        let st_pow = main_pow_pod.public_statements[0].clone();
        builder.add_pod(main_pow_pod);
        let batch_def = BatchDef::new(ingredients_def.clone(), work);
        let batch_hash = batch_def.batch_hash(&params)?;
        let item_def = ItemDef::new(batch_def.clone(), index.clone())?;
        let item_hash = item_def.item_hash(&params)?;
        let key_dict =
            Dictionary::new(params.max_depth_mt_containers, ingredients_def.keys.clone())?;

        // Sets for on-chain commitment
        let nullifiers = set_from_hashes(&params, &HashSet::new())?;
        let created_items = set_from_hashes(
            &params,
            &HashSet::from([
                hash_value(&Value::from("dummy1").raw()),
                hash_value(&Value::from("dummy2").raw()),
            ]),
        )?;

        // Build BatchDef(batch, ingredients, inputs, keys, work)
        let st_contains_inputs = builder.priv_op(Operation::dict_contains(
            ingredients_dict.clone(),
            "inputs",
            inputs_set.clone(),
        ))?;
        let st_contains_keys = builder.priv_op(Operation::dict_contains(
            ingredients_dict.clone(),
            "keys",
            key_dict.clone(),
        ))?;
        let st_batch_hash = builder.priv_op(Operation::hash_of(
            batch_hash,
            ingredients_dict.clone(),
            batch_def.work,
        ))?;
        let st_batch_def = builder.pub_op(Operation::custom(
            commit_preds.batch_def.clone(),
            [st_contains_inputs, st_contains_keys, st_batch_hash],
        ))?;

        // Build ItemInBatch(item, batch, index, keys)
        let st_item_hash =
            builder.priv_op(Operation::hash_of(item_hash, batch_hash, index.hash()))?;
        let st_contains_key = builder.priv_op(Operation::dict_contains(
            key_dict.clone(),
            index.name(),
            ingredients_def.keys[&index].clone(),
        ))?;
        let st_item_in_batch = builder.pub_op(Operation::custom(
            commit_preds.item_in_batch.clone(),
            [st_item_hash.clone(), st_contains_key.clone()],
        ))?;

        // Build ItemDef(item, ingredients, inputs, key, work)
        let st_item_def = builder.priv_op(Operation::custom(
            commit_preds.item_def.clone(),
            [st_batch_def.clone(), st_item_in_batch, st_contains_key],
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

        // Build AllItemsInBatch(items, batch, keys)
        let empty_set = Set::new(params.max_depth_mt_containers, HashSet::new())?;
        let empty_dict = Dictionary::new(params.max_depth_mt_containers, HashMap::new())?;
        let st_all_items_in_batch_empty = builder.op(
            false,
            vec![(1, batch_hash.into())],
            Operation::custom(
                commit_preds.all_items_in_batch_empty.clone(),
                [
                    Statement::Equal(empty_set.clone().into(), EMPTY_VALUE.into()),
                    Statement::Equal(empty_dict.clone().into(), EMPTY_VALUE.into()),
                ],
            ),
        )?;
        let st_all_items_in_batch1 = builder.priv_op(Operation::custom(
            commit_preds.all_items_in_batch.clone(),
            [st_all_items_in_batch_empty, Statement::None],
        ))?;
        let items = {
            let mut items = empty_set.clone();
            items.insert(&item_hash.into())?;
            items
        };
        let st_set_insert =
            builder.priv_op(Operation::set_insert(items.clone(), empty_set, item_hash))?;
        let st_dict_insert = builder.priv_op(Operation::dict_insert(
            key_dict.clone(),
            empty_dict,
            index.name(),
            key,
        ))?;
        let st_all_items_in_batch_recursive = builder.priv_op(Operation::custom(
            commit_preds.all_items_in_batch_recursive.clone(),
            [
                st_all_items_in_batch1,
                st_set_insert,
                st_dict_insert,
                st_item_hash,
            ],
        ))?;
        let st_all_items_in_batch = builder.priv_op(Operation::custom(
            commit_preds.all_items_in_batch.clone(),
            [Statement::None, st_all_items_in_batch_recursive],
        ))?;

        // Build CommitCreation(item, nullifiers, created_items)
        let _st_commit_crafting = builder.pub_op(Operation::custom(
            commit_preds.commit_creation.clone(),
            [
                st_batch_def,
                st_all_items_in_batch,
                st_inputs_subset,
                st_nullifiers,
            ],
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
                st_pow,
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
                BatchDef(batch, ingredients, inputs, keys, work)
                ItemKey(item, key)
                SubsetOf(inputs, created_items)
                Nullifiers(nullifiers, inputs)
                CommitCreation(items, nullifiers, created_items)
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
                ("batch".into(), batch_hash.into()),
                ("item".to_string(), Value::from(item_hash)),
                ("items".into(), items.into()),
                ("ingredients".to_string(), Value::from(ingredients_dict)),
                ("inputs".to_string(), Value::from(inputs_set)),
                ("key".to_string(), Value::from(key)),
                ("keys".into(), key_dict.into()),
                ("work".to_string(), Value::from(work)),
                ("created_items".to_string(), Value::from(created_items)),
                ("nullifiers".to_string(), Value::from(nullifiers)),
            ]),
        );

        Ok(())
    }
}

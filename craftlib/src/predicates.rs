use std::slice;

use commitlib::predicates::CommitPredicates;
use pod2::middleware::{CustomPredicateRef, Params};
use pod2utils::PredicateDefs;

pub struct ItemPredicates {
    pub defs: PredicateDefs,

    pub is_copper: CustomPredicateRef,
}

impl ItemPredicates {
    pub fn compile(params: &Params, commit_preds: &CommitPredicates) -> Self {
        // maximum allowed:
        // 4 batches
        // 4 predicates per batch
        // 8 arguments per predicate, at most 5 of which are public
        // 5 statements per predicate
        let batch_defs = [r#"
            use intro Pow(count, input, output) from 0xa5ddf9efb3b8f9c2e58af8c03f0b1f4e8ce29da41d2bf50e71d613c4e95319dd // powpod vd hash

            // Example of a mined item with no inputs or sequential work.
            // Copper requires working in a copper mine (blueprint="copper") and
            // 10 leading 0s.
            IsCopper(item, private: ingredients, inputs, key, work) = AND(
                ItemDef(item, ingredients, inputs, key, work)
                Equal(inputs, {})
                DictContains(ingredients, "blueprint", "copper")
                Pow(3, ingredients, work)
            )

            // Example of a mined item which is more common but takes more work to
            // extract.
            IsTin(item, private: ingredients, inputs, key, work) = AND(
                ItemDef(item, ingredients, inputs, key, work)
                Equal(inputs, {})
                DictContains(ingredients, "blueprint", "tin")
                // TODO input POD: SequentialWork(ingredients, work, 5)
                // TODO input POD: HashInRange(0, 1<<5, ingredients)
            )

            BronzeInputs(inputs, private: s1, tin, copper) = AND(
                // 2 ingredients
                SetInsert(s1, {}, tin)
                SetInsert(inputs, s1, copper)

                // Recursively prove the ingredients are correct.
                IsTin(tin)
                IsCopper(copper)
            )

            // Combining Copper and Tin to get Bronze is easy (no sequential work).
            // TODO: Require a smelter as a tool
            IsBronze(item, private: ingredients, inputs, key, work) = AND(
                ItemDef(item, ingredients, inputs, key, work)
                DictContains(ingredients, "blueprint", "bronze")

                BronzeInputs(inputs)
            )
            "#];

        let defs = PredicateDefs::new(params, &batch_defs, slice::from_ref(&commit_preds.defs));

        ItemPredicates {
            is_copper: defs.predicate_ref_by_name("IsCopper").unwrap(),
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
        constants::COPPER_BLUEPRINT,
        powpod::PowPod,
        test_util::test::{check_matched_wildcards, mock_vd_set},
    };

    #[test]
    fn test_compile_custom_predicates() {
        let params = Params::default();
        let commit_preds = CommitPredicates::compile(&params);
        assert!(commit_preds.defs.batches.len() == 2);

        let item_preds = ItemPredicates::compile(&params, &commit_preds);
        assert!(item_preds.defs.batches.len() == 1);
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
                ("blueprint".to_string(), Value::from(COPPER_BLUEPRINT)),
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

        // Build IsCopper(item)
        let st_contains_blueprint = builder.priv_op(Operation::dict_contains(
            ingredients_dict.clone(),
            "blueprint",
            Value::from(COPPER_BLUEPRINT),
        ))?;
        let _st_is_copper = builder.pub_op(Operation::custom(
            item_preds.is_copper.clone(),
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
                ItemDef(item, ingredients, inputs, key, work)
                ItemKey(item, key)
                SubsetOf(inputs, created_items)
                Nullifiers(nullifiers, inputs)
                CommitCreation(item, nullifiers, created_items)
                IsCopper(item)
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

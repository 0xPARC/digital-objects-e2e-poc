use std::{slice, sync::Arc};

use itertools::Itertools;
use pod2::middleware::{CustomPredicateBatch, CustomPredicateRef, Hash, Params};

pub const CONSUMED_ITEM_EXTERNAL_NULLIFIER: &str = "consumed item external nullifier";

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
            r#"
            // Derive nullifiers from items (using a recursive foreach construction)
            // This proves the relationship between an item and its key before using
            // the key to calculate a nullifier.
            Nullifiers(nullifiers, inputs) = OR(
                NullifiersEmpty(nullifiers, inputs)
                NullifiersRecursive(nullifiers, inputs)
            )

            NullifiersEmpty(nullifiers, inputs) = AND(
                Equal(nullifiers, {})
                Equal(inputs, {})
            )

            NullifiersRecursive(nullifiers, inputs, private: i, n, k, ns, is) = AND(
                ItemKey(i, k)
                HashOf(n, k, "consumed item external nullifier")
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
            "#,
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
            // Example of a mined item with no inputs or sequential work.
            // Copper requ`ires working in a copper mine (blueprint="copper") and
            // 10 leading 0s.
            IsCopper(item, private: ingredients, inputs, key, work) = AND(
                ItemDef(item, ingredients, inputs, key, work)
                Equal(inputs, {})
                Equal(work, 0)
                DictContains(ingredients, "blueprint", "copper")

                // TODO input POD: HashInRange(0, 1<<10, ingredients)
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

    use pod2::{
        backends::plonky2::mock::mainpod::MockProver,
        frontend::{MainPodBuilder, Operation},
        lang::parse,
        middleware::{EMPTY_VALUE, RawValue, Statement, Value, hash_value},
    };

    use super::*;
    use crate::{
        constants::COPPER_BLUEPRINT,
        item::{IngredientsDef, ItemDef},
        test_util::test::{check_matched_wildcards, mock_vd_set},
        util::set_from_hashes,
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
        let seed = 0xA34;
        let key = 0xBADC0DE;
        let work: RawValue = EMPTY_VALUE;
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

        // Build SuperSubSet(created_items, inputs)
        // We use builder.op() to manually specify the `super` wildcard value
        // because it's otherwise unconstrained.  This is only relevant in
        // the base case where `sub` is empty, which is a subset of anything.
        let st_inputs_eq_empty = builder.priv_op(Operation::eq(inputs_set.clone(), EMPTY_VALUE))?;
        let st_inputs_subset = builder.op(
            true, /*public*/
            vec![(0, Value::from(created_items.clone()))],
            Operation::custom(
                commit_preds.super_sub_set.clone(),
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

        // Build CommitCrafting(item, nullifiers, created_items)
        let _st_commit_crafting = builder.pub_op(Operation::custom(
            commit_preds.commit_crafting.clone(),
            [st_item_def.clone(), st_inputs_subset, st_nullifiers],
        ))?;

        // Build IsCopper(item)
        let st_work_empty = builder.priv_op(Operation::eq(work, EMPTY_VALUE))?;
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
                st_work_empty,
                st_contains_blueprint,
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
                SuperSubSet(created_items, inputs)
                Nullifiers(nullifiers, inputs)
                CommitCrafting(item, nullifiers, created_items)
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

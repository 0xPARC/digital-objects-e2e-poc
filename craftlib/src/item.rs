use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use commitlib::{IngredientsDef, ItemDef, build_st_item_def, build_st_item_key};
use pod2::{
    frontend::{MainPod, MainPodBuilder},
    middleware::{
        CustomPredicateBatch, EMPTY_VALUE, Hash, MainPodProver, Params, RawValue, Statement,
        ToFields, VDSet, Value,
    },
};
use pod2utils::{macros::BuildContext, st_custom};

use crate::constants::COPPER_BLUEPRINT;

// Reusable recipe for an item to be mined, not including the variable
// cryptographic values.
#[derive(Debug, Clone)]
pub struct MiningRecipe {
    pub inputs: HashSet<Hash>,
    pub blueprint: String,
}

impl MiningRecipe {
    pub fn prep_ingredients(&self, key: RawValue, seed: i64) -> IngredientsDef {
        let app_layer = HashMap::from([
            ("blueprint".to_string(), Value::from(self.blueprint.clone())),
            ("seed".to_string(), Value::from(seed)),
        ]);
        IngredientsDef {
            inputs: self.inputs.clone(),
            key,
            app_layer,
        }
    }

    pub fn do_mining(
        &self,
        params: &Params,
        key: RawValue,
        start_seed: i64,
        mine_max: u64,
    ) -> pod2::middleware::Result<Option<IngredientsDef>> {
        for seed in start_seed..=i64::MAX {
            let ingredients = self.prep_ingredients(key, seed);
            let ingredients_hash = ingredients.hash(params)?;
            let mining_val = ingredients_hash.to_fields(params)[0];
            if mining_val.0 <= mine_max {
                return Ok(Some(ingredients));
            }
        }

        Ok(None)
    }

    pub fn new_no_inputs(blueprint: String) -> Self {
        MiningRecipe {
            inputs: HashSet::new(),
            blueprint,
        }
    }
}

// Adds statements to MainPodBilder to represent Copper as additions to
// already-existing generic item statements.
// Builds the following public predicates: IsCopper
// Returns the Statement object for IsCopper for use in further statements.
fn build_st_is_copper(
    ctx: &mut BuildContext,
    params: &Params,
    item_def: ItemDef,
    st_item_def: Statement,
) -> anyhow::Result<Statement> {
    // Build IsCopper(item)
    let st_is_copper = st_custom!(ctx,
        IsCopper() = (
            st_item_def,
            Equal(item_def.work, EMPTY_VALUE),
            Equal(item_def.ingredients.inputs_set(params)?, EMPTY_VALUE),
            DictContains(item_def.ingredients.dict(params)?, "blueprint", COPPER_BLUEPRINT)
        ))?;

    Ok(st_is_copper)
}

// Builds the private POD to store locally for use in further crafting.
// Contains the following public predicates: ItemDef, ItemKey, IsCopper
pub fn prove_copper(
    item_def: ItemDef,

    // TODO: All the args below might belong in a ItemBuilder object
    batches: &[Arc<CustomPredicateBatch>],
    params: &Params,
    prover: &dyn MainPodProver,
    vd_set: &VDSet,
) -> anyhow::Result<MainPod> {
    let mut builder = MainPodBuilder::new(&Default::default(), vd_set);

    let mut ctx = BuildContext {
        builder: &mut builder,
        batches,
    };
    let st_item_def = build_st_item_def(&mut ctx, params, item_def.clone())?;
    ctx.builder.reveal(&st_item_def);
    let st_item_key = build_st_item_key(&mut ctx, st_item_def.clone())?;
    ctx.builder.reveal(&st_item_key);
    let st_is_copper = build_st_is_copper(&mut ctx, params, item_def, st_item_def)?;
    ctx.builder.reveal(&st_is_copper);

    // Prove MainPOD
    Ok(builder.prove(prover)?)
}

#[cfg(test)]
mod tests {

    use std::collections::HashMap;

    use commitlib::{
        predicates::CommitPredicates, prove_st_commit_creation, util::set_from_hashes,
    };
    use pod2::{
        backends::plonky2::mock::mainpod::MockProver,
        lang::parse,
        middleware::{RawValue, hash_value},
    };

    use super::*;
    use crate::{
        constants::{COPPER_BLUEPRINT, COPPER_MINING_MAX, COPPER_WORK},
        predicates::ItemPredicates,
        test_util::test::mock_vd_set,
    };

    // Seed of 2612=0xA34 is a match with hash 6647892930992163=0x000A7EE9D427E832.
    const COPPER_START_SEED: i64 = 0x9C4;

    fn check_matched_wildcards(matched: HashMap<String, Value>, expected: HashMap<String, Value>) {
        assert_eq!(matched.len(), expected.len(), "len");
        for name in expected.keys() {
            assert_eq!(matched[name], expected[name], "{name}");
        }
    }

    #[test]
    fn test_mine_copper() -> anyhow::Result<()> {
        let params = Params::default();
        let mining_recipe = MiningRecipe::new_no_inputs(COPPER_BLUEPRINT.to_string());
        let key = RawValue::from(0xBADC0DE);

        // Seed of 2612=0xA34 is a match with hash 6647892930992163=0x000A7EE9D427E832.
        // TODO: This test is going to get slower (~2s) whenever the ingredient
        // dict definition changes.  Need a better approach to testing mining.
        let mine_success =
            mining_recipe.do_mining(&params, key, COPPER_START_SEED, COPPER_MINING_MAX)?;
        assert!(mine_success.is_some());

        let ingredients_def = mine_success.unwrap();
        let item_def = ItemDef::new(ingredients_def.clone(), COPPER_WORK);
        let item_hash = item_def.item_hash(&params)?;
        println!(
            "Mined copper {:?} from ingredients {:?}",
            item_hash,
            ingredients_def.hash(&params)?
        );

        Ok(())
    }

    #[test]
    fn test_mine_and_prove_copper() -> anyhow::Result<()> {
        let params = Params::default();
        let commit_preds = CommitPredicates::compile(&params);
        let mut batches = commit_preds.defs.batches.clone();
        let item_preds = ItemPredicates::compile(&params, &commit_preds);
        batches.extend_from_slice(&item_preds.defs.batches);

        let prover = &MockProver {};
        let vd_set = &mock_vd_set();

        // Mine copper with a selected key.
        let key = RawValue::from(0xBADC0DE);
        let mining_recipe = MiningRecipe::new_no_inputs(COPPER_BLUEPRINT.to_string());
        let ingredients_def = mining_recipe
            .do_mining(&params, key, COPPER_START_SEED, COPPER_MINING_MAX)?
            .unwrap();

        // Pre-calculate hashes and intermediate values.
        let ingredients_dict = ingredients_def.dict(&params)?;
        let inputs_set = ingredients_def.inputs_set(&params)?;
        let item_def = ItemDef {
            ingredients: ingredients_def.clone(),
            work: COPPER_WORK,
        };
        let item_hash = item_def.item_hash(&params)?;

        // Prove a copper POD.  This is the private POD for the player to store
        // locally for future crafting.
        let copper_main_pod = prove_copper(item_def.clone(), &batches, &params, prover, vd_set)?;

        copper_main_pod.pod.verify()?;
        assert_eq!(copper_main_pod.public_statements.len(), 3);
        //println!("Copper POD: {:?}", copper_main_pod.pod);

        // PODLang query to check the final statements.
        let copper_query = format!(
            r#"
            {}
            {}

            REQUEST(
                ItemDef(item, ingredients, inputs, key, work)
                ItemKey(item, key)
                IsCopper(item)
            )
            "#,
            &commit_preds.defs.imports, &item_preds.defs.imports,
        );

        println!("Copper verification request: {copper_query}");

        let copper_request = parse(
            &copper_query,
            &params,
            &[
                commit_preds.defs.batches.clone(),
                item_preds.defs.batches.clone(),
            ]
            .concat(),
        )?
        .request;
        let matched_wildcards = copper_request.exact_match_pod(&*copper_main_pod.pod)?;
        check_matched_wildcards(
            matched_wildcards,
            HashMap::from([
                ("item".to_string(), Value::from(item_hash)),
                ("ingredients".to_string(), Value::from(ingredients_dict)),
                ("inputs".to_string(), Value::from(inputs_set)),
                ("key".to_string(), Value::from(key)),
                ("work".to_string(), Value::from(EMPTY_VALUE)),
            ]),
        );

        // Dummy (non-empty) created items set which works for 0 inputs.
        let created_items = set_from_hashes(
            &params,
            &HashSet::from([
                hash_value(&Value::from("dummy1").raw()),
                hash_value(&Value::from("dummy2").raw()),
            ]),
        )?;

        // TODO Prove a commitment POD to send on-chain.  This intentionally doesn't
        // expose any public statements other than CommitCreation.
        let commit_main_pod = prove_st_commit_creation(
            item_def,
            created_items.clone(),
            copper_main_pod,
            &batches,
            &params,
            prover,
            vd_set,
        )?;

        commit_main_pod.pod.verify()?;
        assert_eq!(commit_main_pod.public_statements.len(), 1);
        //println!("Commit POD: {:?}", copper_main_pod.pod);

        // PODLang query to check the final statement.
        let commit_query = format!(
            r#"
            {}

            REQUEST(
                CommitCreation(item, nullifiers, created_items)
            )
            "#,
            &commit_preds.defs.imports,
        );

        println!("Commit verification request: {commit_query}");

        let commit_request = parse(
            &commit_query,
            &params,
            &[
                commit_preds.defs.batches.clone(),
                item_preds.defs.batches.clone(),
            ]
            .concat(),
        )?
        .request;
        let matched_wildcards = commit_request.exact_match_pod(&*commit_main_pod.pod)?;
        check_matched_wildcards(
            matched_wildcards,
            HashMap::from([
                ("item".to_string(), Value::from(item_hash)),
                ("created_items".to_string(), Value::from(created_items)),
                ("nullifiers".to_string(), Value::from(EMPTY_VALUE)),
            ]),
        );

        Ok(())
    }
}

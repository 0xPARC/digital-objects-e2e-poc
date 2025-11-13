use std::collections::{HashMap, HashSet};

use commitlib::{IngredientsDef, ItemDef};
use log;
use pod2::middleware::{EMPTY_VALUE, Hash, Params, RawValue, Statement, ToFields, Value};
use pod2utils::{macros::BuildContext, set, st_custom};

use crate::constants::{AXE_BLUEPRINT, STONE_BLUEPRINT, WOOD_BLUEPRINT, WOODEN_AXE_BLUEPRINT};

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
        log::info!("Mining...");
        for seed in start_seed..=i64::MAX {
            let ingredients = self.prep_ingredients(key, seed);
            let ingredients_hash = ingredients.hash(params)?;
            let mining_val = ingredients_hash.to_fields(params)[0];
            if mining_val.0 <= mine_max {
                log::info!("Mining complete!");
                return Ok(Some(ingredients));
            }
        }

        Ok(None)
    }

    pub fn new(blueprint: String, inputs: &[Hash]) -> Self {
        MiningRecipe {
            inputs: HashSet::from_iter(inputs.iter().cloned()),
            blueprint,
        }
    }
}

pub struct CraftBuilder<'a> {
    pub ctx: BuildContext<'a>,
    pub params: &'a Params,
}

impl<'a> CraftBuilder<'a> {
    pub fn new(ctx: BuildContext<'a>, params: &'a Params) -> Self {
        Self { ctx, params }
    }

    // Adds statements to MainPodBilder to represent Stone as additions to
    // already-existing generic item statements.
    // Builds the following public predicates: IsStone
    // Returns the Statement object for IsStone for use in further statements.
    pub fn st_is_stone(
        &mut self,
        item_def: ItemDef,
        st_item_def: Statement,
        st_pow: Statement,
    ) -> anyhow::Result<Statement> {
        // Build IsStone(item)
        Ok(st_custom!(self.ctx,
            IsStone() = (
                st_item_def,
                Equal(item_def.ingredients.inputs_set(self.params)?, EMPTY_VALUE),
                DictContains(item_def.ingredients.dict(self.params)?, "blueprint", STONE_BLUEPRINT),
                st_pow
            ))?)
    }

    pub fn st_is_wood(
        &mut self,
        item_def: ItemDef,
        st_item_def: Statement,
    ) -> anyhow::Result<Statement> {
        // Build IsWood(item)
        Ok(st_custom!(self.ctx,
            IsWood() = (
                st_item_def,
                Equal(item_def.ingredients.inputs_set(self.params)?, EMPTY_VALUE),
                DictContains(item_def.ingredients.dict(self.params)?, "blueprint", WOOD_BLUEPRINT)
            ))?)
    }

    fn st_axe_inputs(
        &mut self,
        st_is_wood: Statement,
        st_is_stone: Statement,
    ) -> anyhow::Result<Statement> {
        let wood = st_is_wood.args()[0].literal().unwrap();
        let stone = st_is_stone.args()[0].literal().unwrap();
        let empty_set = set!(self.params.max_depth_mt_containers).unwrap();
        let mut s1 = empty_set.clone();
        s1.insert(&wood).unwrap();
        let mut inputs = s1.clone();
        inputs.insert(&stone).unwrap();
        Ok(st_custom!(self.ctx,
            AxeInputs() = (
                SetInsert(s1, empty_set, wood),
                SetInsert(inputs, s1, stone),
                st_is_wood,
                st_is_stone
            ))?)
    }

    pub fn st_is_axe(
        &mut self,
        item_def: ItemDef,
        st_item_def: Statement,
        st_is_wood: Statement,
        st_is_stone: Statement,
    ) -> anyhow::Result<Statement> {
        let st_axe_inputs = self.st_axe_inputs(st_is_wood, st_is_stone)?;
        // Build IsAxe(item)
        Ok(st_custom!(self.ctx,
            IsAxe() = (
                st_item_def,
                DictContains(item_def.ingredients.dict(self.params)?, "blueprint", AXE_BLUEPRINT),
                st_axe_inputs
            ))?)
    }

    fn st_wooden_axe_inputs(
        &mut self,
        st_is_wood1: Statement,
        st_is_wood2: Statement,
    ) -> anyhow::Result<Statement> {
        let wood1 = st_is_wood1.args()[0].literal().unwrap();
        let wood2 = st_is_wood2.args()[0].literal().unwrap();
        let empty_set = set!(self.params.max_depth_mt_containers).unwrap();
        let mut s1 = empty_set.clone();
        s1.insert(&wood1).unwrap();
        let mut inputs = s1.clone();
        inputs.insert(&wood2).unwrap();
        Ok(st_custom!(self.ctx,
            WoodenAxeInputs() = (
                SetInsert(s1, empty_set, wood1),
                SetInsert(inputs, s1, wood2),
                st_is_wood1,
                st_is_wood2
            ))?)
    }

    pub fn st_is_wooden_axe(
        &mut self,
        item_def: ItemDef,
        st_item_def: Statement,
        st_is_wood1: Statement,
        st_is_wood2: Statement,
    ) -> anyhow::Result<Statement> {
        let st_wooden_axe_inputs = self.st_wooden_axe_inputs(st_is_wood1, st_is_wood2)?;
        // Build IsWoodenAxe(item)
        Ok(st_custom!(self.ctx,
            IsWoodenAxe() = (
                st_item_def,
                DictContains(item_def.ingredients.dict(self.params)?, "blueprint", WOODEN_AXE_BLUEPRINT),
                st_wooden_axe_inputs
            ))?)
    }
}

#[cfg(test)]
mod tests {

    use std::{collections::HashMap, sync::Arc};

    use commitlib::{ItemBuilder, ItemDef, predicates::CommitPredicates, util::set_from_hashes};
    use pod2::{
        backends::plonky2::mock::mainpod::MockProver,
        frontend::{MainPod, MainPodBuilder},
        lang::parse,
        middleware::{
            CustomPredicateBatch, EMPTY_VALUE, MainPodProver, Params, Pod, RawValue, VDSet, Value,
            containers::Set, hash_value,
        },
    };

    use super::*;
    use crate::{
        constants::{STONE_BLUEPRINT, STONE_MINING_MAX, STONE_WORK},
        powpod::PowPod,
        predicates::ItemPredicates,
        test_util::test::mock_vd_set,
    };

    // Seed of 2612=0xA34 is a match with hash 6647892930992163=0x000A7EE9D427E832.
    const STONE_START_SEED: i64 = 0x9C4;

    // Builds the private POD to store locally for use in further crafting.
    // Contains the following public predicates: ItemDef, ItemKey, IsStone
    fn prove_stone(
        item_def: ItemDef,
        pow_pod: MainPod,

        // TODO: All the args below might belong in a ItemBuilder object
        batches: &[Arc<CustomPredicateBatch>],
        params: &Params,
        prover: &dyn MainPodProver,
        vd_set: &VDSet,
    ) -> anyhow::Result<MainPod> {
        let mut builder = MainPodBuilder::new(&Default::default(), vd_set);
        let mut item_builder = ItemBuilder::new(BuildContext::new(&mut builder, batches), params);
        let st_item_def = item_builder.st_item_def(item_def.clone())?;
        item_builder.ctx.builder.reveal(&st_item_def);
        let st_item_key = item_builder.st_item_key(st_item_def.clone())?;
        item_builder.ctx.builder.reveal(&st_item_key);

        let st_pow = pow_pod.public_statements[0].clone();

        let mut craft_builder = CraftBuilder::new(BuildContext::new(&mut builder, batches), params);
        craft_builder.ctx.builder.add_pod(pow_pod);
        let st_is_stone = craft_builder.st_is_stone(item_def, st_item_def, st_pow)?;
        craft_builder.ctx.builder.reveal(&st_is_stone);

        // Prove MainPOD
        Ok(builder.prove(prover)?)
    }

    // Builds the public POD to commit a creation operation on-chain, with the only
    // public predicate being CommitCreation.  Uses a given created_items_set as
    // the root to prove that inputs were previously created.
    fn prove_st_commit_creation(
        item_def: ItemDef,
        created_items: Set,
        item_main_pod: MainPod,

        // TODO: All the args below might belong in a ItemBuilder object
        batches: &[Arc<CustomPredicateBatch>],
        params: &Params,
        prover: &dyn MainPodProver,
        vd_set: &VDSet,
    ) -> anyhow::Result<MainPod> {
        let mut builder = MainPodBuilder::new(&Default::default(), vd_set);

        // TODO: Consider a more robust lookup for this which doesn't depend on index.
        let st_item_def = item_main_pod.public_statements[0].clone();
        builder.add_pod(item_main_pod);

        let mut item_builder = ItemBuilder::new(BuildContext::new(&mut builder, batches), params);
        let (st_nullifier, _) = item_builder.st_nullifiers(vec![])?;
        let st_commit_creation =
            item_builder.st_commit_creation(item_def, st_nullifier, created_items, st_item_def)?;
        builder.reveal(&st_commit_creation);

        // Prove MainPOD
        Ok(builder.prove(prover)?)
    }

    fn check_matched_wildcards(matched: HashMap<String, Value>, expected: HashMap<String, Value>) {
        assert_eq!(matched.len(), expected.len(), "len");
        for name in expected.keys() {
            assert_eq!(matched[name], expected[name], "{name}");
        }
    }

    #[test]
    fn test_mine_stone() -> anyhow::Result<()> {
        let params = Params::default();
        let mining_recipe = MiningRecipe::new(STONE_BLUEPRINT.to_string(), &[]);
        let key = RawValue::from(0xBADC0DE);

        // Seed of 2612=0xA34 is a match with hash 6647892930992163=0x000A7EE9D427E832.
        // TODO: This test is going to get slower (~2s) whenever the ingredient
        // dict definition changes.  Need a better approach to testing mining.
        let mine_success =
            mining_recipe.do_mining(&params, key, STONE_START_SEED, STONE_MINING_MAX)?;
        assert!(mine_success.is_some());

        let ingredients_def = mine_success.unwrap();
        let item_def = ItemDef::new(ingredients_def.clone(), STONE_WORK);
        let item_hash = item_def.item_hash(&params)?;
        println!(
            "Mined stone {:?} from ingredients {:?}",
            item_hash,
            ingredients_def.hash(&params)?
        );

        Ok(())
    }

    #[test]
    fn test_mine_and_prove_stone() -> anyhow::Result<()> {
        let params = Params::default();
        let commit_preds = CommitPredicates::compile(&params);
        let mut batches = commit_preds.defs.batches.clone();
        let item_preds = ItemPredicates::compile(&params, &commit_preds);
        batches.extend_from_slice(&item_preds.defs.batches);

        let prover = &MockProver {};
        let vd_set = &mock_vd_set();

        // Mine stone with a selected key.
        let key = RawValue::from(0xBADC0DE);
        let mining_recipe = MiningRecipe::new(STONE_BLUEPRINT.to_string(), &[]);
        let ingredients_def = mining_recipe
            .do_mining(&params, key, STONE_START_SEED, STONE_MINING_MAX)?
            .unwrap();

        let pow_pod = PowPod::new(
            &params,
            vd_set.clone(),
            3, // num_iters
            RawValue::from(ingredients_def.dict(&params)?.commitment()),
        )?;
        let main_pow_pod = MainPod {
            pod: Box::new(pow_pod.clone()),
            public_statements: pow_pod.pub_statements(),
            params: params.clone(),
        };

        // Pre-calculate hashes and intermediate values.
        let ingredients_dict = ingredients_def.dict(&params)?;
        let inputs_set = ingredients_def.inputs_set(&params)?;
        let item_def = ItemDef {
            ingredients: ingredients_def.clone(),
            work: pow_pod.output,
        };
        let item_hash = item_def.item_hash(&params)?;

        // Prove a stone POD.  This is the private POD for the player to store
        // locally for future crafting.
        let stone_main_pod = prove_stone(
            item_def.clone(),
            main_pow_pod,
            &batches,
            &params,
            prover,
            vd_set,
        )?;

        stone_main_pod.pod.verify()?;
        assert_eq!(stone_main_pod.public_statements.len(), 3);
        //println!("Stone POD: {:?}", stone_main_pod.pod);

        // PODLang query to check the final statements.
        let stone_query = format!(
            r#"
            {}
            {}

            REQUEST(
                ItemDef(item, ingredients, inputs, key, work)
                ItemKey(item, key)
                IsStone(item)
            )
            "#,
            &commit_preds.defs.imports, &item_preds.defs.imports,
        );

        println!("Stone verification request: {stone_query}");

        let stone_request = parse(
            &stone_query,
            &params,
            &[
                commit_preds.defs.batches.clone(),
                item_preds.defs.batches.clone(),
            ]
            .concat(),
        )?
        .request;
        let matched_wildcards = stone_request.exact_match_pod(&*stone_main_pod.pod)?;
        check_matched_wildcards(
            matched_wildcards,
            HashMap::from([
                ("item".to_string(), Value::from(item_hash)),
                ("ingredients".to_string(), Value::from(ingredients_dict)),
                ("inputs".to_string(), Value::from(inputs_set)),
                ("key".to_string(), Value::from(key)),
                ("work".to_string(), Value::from(pow_pod.output)),
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
            stone_main_pod,
            &batches,
            &params,
            prover,
            vd_set,
        )?;

        commit_main_pod.pod.verify()?;
        assert_eq!(commit_main_pod.public_statements.len(), 1);
        //println!("Commit POD: {:?}", stone_main_pod.pod);

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

pub mod predicates;
pub mod util;

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use pod2::{
    frontend::{MainPod, MainPodBuilder},
    middleware::{
        CustomPredicateBatch, EMPTY_HASH, EMPTY_VALUE, Hash, Key, MainPodProver, Params, RawValue,
        Statement, VDSet, Value,
        containers::{Dictionary, Set},
        hash_values,
    },
};
use pod2utils::{macros::BuildContext, pub_st_custom, st_custom};

use crate::util::set_from_hashes;

pub const CONSUMED_ITEM_EXTERNAL_NULLIFIER: &str = "consumed item external nullifier";

// Rust-level definition of the ingredients of an item, used to derive the
// ingredients hash (dict root) before doing sequential work on it.
#[derive(Debug, Clone)]
pub struct IngredientsDef {
    // These properties are committed on-chain
    pub inputs: HashSet<Hash>,
    pub key: RawValue,

    // These properties are used only by the application layer
    pub app_layer: HashMap<String, Value>,
}

impl IngredientsDef {
    pub fn dict(&self, params: &Params) -> pod2::middleware::Result<Dictionary> {
        let mut map = HashMap::new();
        map.insert(Key::from("inputs"), Value::from(self.inputs_set(params)?));
        map.insert(Key::from("key"), Value::from(self.key));
        for (key, value) in &self.app_layer {
            map.insert(Key::from(key), value.clone());
        }
        Dictionary::new(params.max_depth_mt_containers, map)
    }

    pub fn hash(&self, params: &Params) -> pod2::middleware::Result<Hash> {
        Ok(self.dict(params)?.commitment())
    }

    pub fn inputs_set(&self, params: &Params) -> pod2::middleware::Result<Set> {
        set_from_hashes(params, &self.inputs)
    }
}

// Rust-level definition of an item, used to derive its ID (hash).
#[derive(Debug, Clone)]
pub struct ItemDef {
    pub ingredients: IngredientsDef,
    pub work: RawValue,
}

impl ItemDef {
    pub fn item_hash(&self, params: &Params) -> pod2::middleware::Result<Hash> {
        Ok(hash_values(&[
            Value::from(self.ingredients.dict(params)?),
            Value::from(self.work),
        ]))
    }

    pub fn nullifier(&self, params: &Params) -> pod2::middleware::Result<Hash> {
        Ok(hash_values(&[
            Value::from(self.item_hash(params)?),
            Value::from(CONSUMED_ITEM_EXTERNAL_NULLIFIER),
        ]))
    }

    pub fn new(ingredients: IngredientsDef, work: RawValue) -> Self {
        Self { ingredients, work }
    }
}

// Adds statements to MainPodBilder to represent a generic item based on the
// ItemDef.  Includes the following public predicates: ItemDef, ItemKey
// Returns the Statement object for ItemDef for use in further statements.
pub fn build_st_item_def(
    ctx: &mut BuildContext,
    params: &Params,
    item_def: ItemDef,
) -> anyhow::Result<Statement> {
    let ingredients_dict = item_def.ingredients.dict(params)?;
    let inputs_set = item_def.ingredients.inputs_set(params)?;
    let item_hash = item_def.item_hash(params)?;

    // Build ItemDef(item, ingredients, inputs, key, work)
    let st_item_def = pub_st_custom!(ctx,
        ItemDef() = (
            DictContains(ingredients_dict, "inputs", inputs_set),
            DictContains(ingredients_dict, "key", item_def.ingredients.key),
            HashOf(item_hash, ingredients_dict, item_def.work)
        ))?;

    // Build ItemKey(item, key)
    let _st_itemkey = pub_st_custom!(ctx,
        ItemKey() = (
            st_item_def.clone()
        ))?;

    Ok(st_item_def)
}

// Adds statements to MainPodBuilder to prove inclusion of input_set in
// created_items_set.  Returns the private SuperSubSet statement.
fn build_st_super_sub_set(
    ctx: &mut BuildContext,
    inputs_set: Set,
    created_items: Set,
) -> anyhow::Result<Statement> {
    // TODO: Needs a real impl.  This only works for 0 inputs.
    assert!(inputs_set.commitment() == EMPTY_HASH);

    // Build SuperSubSet(created_items, inputs)
    // We manually specify the `super` wildcard value because it's otherwise unconstrained.  This
    // is only relevant in the base case where `sub` is empty, which is a subset of anything.
    let st_inputs_subset = st_custom!(ctx,
        SuperSubSet(super=created_items) = (
            Equal(inputs_set, EMPTY_VALUE),
            Statement::None
        ))?;

    Ok(st_inputs_subset)
}

// Adds statements to MainPodBilder to prove correct nullifiers for a set of
// inputs.  Returns the private Nullifiers.
fn build_st_nullifiers(
    ctx: &mut BuildContext,
    inputs_set: Set,
    nullifiers: Set,
) -> anyhow::Result<Statement> {
    // TODO: Needs a real impl.  This only works for 0 inputs.
    assert!(inputs_set.commitment() == EMPTY_HASH);
    assert!(nullifiers.commitment() == EMPTY_HASH);

    // Build Nullifiers(nullifiers, inputs)
    let st_nullifiers_empty = st_custom!(ctx,
        NullifiersEmpty() = (
            Equal(inputs_set, EMPTY_VALUE),
            Equal(nullifiers, EMPTY_VALUE)
        ))?;
    let st_nullifiers = st_custom!(ctx,
        Nullifiers() = (
            st_nullifiers_empty,
            Statement::None
        ))?;

    Ok(st_nullifiers)
}

// Builds the public POD to commit a crafting operation on-chain, with the only
// public predicate being CommitCrafting.  Uses a given created_items_set as
// the root to prove that inputs were previously crafted.
pub fn build_st_commit_crafting(
    ctx: &mut BuildContext,
    params: &Params, // TODO: This argument might belong in a ItemBuilder object
    item_def: ItemDef,
    created_items: Set,
    st_item_def: Statement,
) -> anyhow::Result<Statement> {
    let st_inputs_subset = build_st_super_sub_set(
        ctx,
        item_def.ingredients.inputs_set(params)?,
        created_items.clone(),
    )?;

    // TODO: Calculate real nullifiers for non-empty inputs.
    let nullifiers = set_from_hashes(params, &HashSet::new())?;
    let st_nullifiers =
        build_st_nullifiers(ctx, item_def.ingredients.inputs_set(params)?, nullifiers)?;

    // Build CommitCrafting(item, nullifiers, created_items)
    let st_commit_crafting = pub_st_custom!(ctx,
        CommitCrafting() = (
            st_item_def.clone(),
            st_inputs_subset,
            st_nullifiers
        ))?;

    Ok(st_commit_crafting)
}

// Builds the public POD to commit a crafting operation on-chain, with the only
// public predicate being CommitCrafting.  Uses a given created_items_set as
// the root to prove that inputs were previously crafted.
pub fn prove_st_commit_crafting(
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

    let mut ctx = BuildContext {
        builder: &mut builder,
        batches,
    };
    let _st_commit_crafting =
        build_st_commit_crafting(&mut ctx, params, item_def, created_items, st_item_def)?;

    // Prove MainPOD
    Ok(builder.prove(prover)?)
}

#[cfg(test)]
mod tests {
    use pod2::{
        backends::plonky2::{
            basetypes::DEFAULT_VD_SET, mainpod::Prover, mock::mainpod::MockProver,
        },
        middleware::hash_value,
    };

    use super::*;
    use crate::predicates::CommitPredicates;

    #[test]
    fn test_prove_st_commit_crafting() {
        let mock = true;

        let mock_prover = MockProver {};
        let real_prover = Prover {};
        let (vd_set, prover): (_, &dyn MainPodProver) = if mock {
            (&VDSet::new(6, &[]).unwrap(), &mock_prover)
        } else {
            let vd_set = &*DEFAULT_VD_SET;
            (vd_set, &real_prover)
        };

        let params = Params::default();

        let commit_preds = CommitPredicates::compile(&params);
        let batches = &commit_preds.defs.batches;

        let mut builder = MainPodBuilder::new(&Default::default(), vd_set);
        let mut ctx = BuildContext {
            builder: &mut builder,
            batches,
        };

        let ingredients_def = IngredientsDef {
            inputs: HashSet::new(),
            key: Value::from(33).raw(),
            app_layer: HashMap::from([("foo".to_string(), Value::from("bar"))]),
        };
        let item_def = ItemDef {
            ingredients: ingredients_def,
            work: Value::from(42).raw(),
        };
        let st_item_def = build_st_item_def(&mut ctx, &params, item_def.clone()).unwrap();

        let created_items = set_from_hashes(
            &params,
            &HashSet::from([
                hash_value(&Value::from("dummy1").raw()),
                hash_value(&Value::from("dummy2").raw()),
            ]),
        )
        .unwrap();

        let _st_commit_crafting =
            build_st_commit_crafting(&mut ctx, &params, item_def, created_items, st_item_def)
                .unwrap();

        let main_pod = builder.prove(prover).unwrap();

        main_pod.pod.verify().unwrap();
    }
}

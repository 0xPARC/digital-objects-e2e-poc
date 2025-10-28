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
use pod2utils::{macros::BuildContext, st_custom};
use serde::{Deserialize, Serialize};

use crate::util::set_from_hashes;

pub const CONSUMED_ITEM_EXTERNAL_NULLIFIER: &str = "consumed item external nullifier";

// Rust-level definition of the ingredients of an item, used to derive the
// ingredients hash (dict root) before doing sequential work on it.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

pub struct ItemBuilder<'a> {
    pub ctx: BuildContext<'a>,
    pub params: &'a Params,
}

impl<'a> ItemBuilder<'a> {
    pub fn new(ctx: BuildContext<'a>, params: &'a Params) -> Self {
        Self { ctx, params }
    }

    // Adds statements to MainPodBilder to represent a generic item based on the
    // ItemDef.  Includes the following public predicates: ItemDef, ItemKey
    // Returns the Statement object for ItemDef for use in further statements.
    pub fn st_item_def(&mut self, item_def: ItemDef) -> anyhow::Result<Statement> {
        let ingredients_dict = item_def.ingredients.dict(self.params)?;
        let inputs_set = item_def.ingredients.inputs_set(self.params)?;
        let item_hash = item_def.item_hash(self.params)?;

        // Build ItemDef(item, ingredients, inputs, key, work)
        Ok(st_custom!(self.ctx,
        ItemDef() = (
            DictContains(ingredients_dict, "inputs", inputs_set),
            DictContains(ingredients_dict, "key", item_def.ingredients.key),
            HashOf(item_hash, ingredients_dict, item_def.work)
        ))?)
    }

    pub fn st_item_key(&mut self, st_item_def: Statement) -> anyhow::Result<Statement> {
        // Build ItemKey(item, key)
        Ok(st_custom!(self.ctx,
        ItemKey() = (
            st_item_def
        ))?)
    }

    fn st_super_sub_set_recursive(
        &mut self,
        inputs_set: &Set,
        created_items: &Set,
    ) -> anyhow::Result<Statement> {
        let mut smaller = inputs_set.clone();
        let i = smaller
            .set()
            .iter()
            .next()
            .expect("Should be nonempty.")
            .clone();
        smaller.delete(&i)?;
        let st_prev = self.st_super_sub_set(&smaller, created_items)?;

        // Build SuperSubSetRecursive(super, sub)
        Ok(st_custom!(self.ctx,
            SuperSubSetRecursive() = (
                SetContains(created_items, i),
                SetInsert(inputs_set, smaller, i),
                st_prev
            ))?)
    }

    // Adds statements to MainPodBuilder to prove inclusion of input_set in
    // created_items_set.  Returns the private SuperSubSet statement.
    fn st_super_sub_set(
        &mut self,
        inputs_set: &Set,
        created_items: &Set,
    ) -> anyhow::Result<Statement> {
        // Build SuperSubSet(created_items, inputs)
        if inputs_set.commitment() == EMPTY_HASH {
            // We manually specify the `super` wildcard value because it's otherwise unconstrained.  This
            // is only relevant in the base case where `sub` is empty, which is a subset of anything.
            Ok(st_custom!(self.ctx,
                SuperSubSet(super=created_items) = (
                    Equal(inputs_set, EMPTY_VALUE),
                    Statement::None
                ))?)
        } else {
            let st_recursive = self.st_super_sub_set_recursive(inputs_set, created_items)?;
            Ok(st_custom!(self.ctx,
                SuperSubSet() = (
                    Statement::None,
                    st_recursive
                ))?)
        }
    }

    // Adds statements to MainPodBilder to prove correct nullifiers for a set of
    // inputs.  Returns the private Nullifiers.
    fn st_nullifiers(
        &mut self,
        // inputs + keys + ItemKey statements
        inputs_with_keys: Vec<(Hash, RawValue, Statement)>,
        nullifiers: Set,
    ) -> anyhow::Result<Statement> {
        let empty_set = Set::new(self.params.max_depth_mt_containers, HashSet::new())?;
        // Build Nullifiers(nullifiers, inputs)
        let st_nullifiers_empty = st_custom!(self.ctx,
            NullifiersEmpty() = (
                Equal(&empty_set, EMPTY_VALUE),
                Equal(&empty_set, EMPTY_VALUE)
            ))?;
        let init_st = st_custom!(self.ctx,
            Nullifiers() = (
                st_nullifiers_empty,
                Statement::None
            ))?;

        let (st, _, ns)= inputs_with_keys.into_iter().try_fold::<_,_,anyhow::Result<_>>((init_st, empty_set.clone(), empty_set), |(st, is, ns), (i,k, ik_st)| {
            let n = hash_values(&[k.into(), CONSUMED_ITEM_EXTERNAL_NULLIFIER.into()]);
            let mut new_ns = ns.clone();
            new_ns.insert(&n.into())?;
            let mut new_is = is.clone();
            new_is.insert(&i.into())?;
            let st_nullifiers_recursive = st_custom!(self.ctx,
                                                     NullifiersRecursive() = (
                                                         ik_st,
                                                         HashOf(n, k, CONSUMED_ITEM_EXTERNAL_NULLIFIER),
                                                         SetInsert(new_ns, ns, n),
                                                         SetInsert(new_is, is, i),
                                                         st
            ))?;
        let st_nullifiers = st_custom!(self.ctx,
                                      Nullifiers() = (
                                                          Statement::None,
                st_nullifiers_recursive
            ))?;
            Ok((st_nullifiers, new_is, new_ns))
        })?;

        // Sanity check
        assert_eq!(ns, nullifiers);

        Ok(st)
    }

    // Builds the public POD to commit a creation operation on-chain, with the only
    // public predicate being CommitCreation.  Uses a given created_items_set as
    // the root to prove that inputs were previously created.
    pub fn st_commit_creation(
        &mut self,
        item_def: ItemDef,
        created_items: Set,
        st_item_def: Statement,
    ) -> anyhow::Result<Statement> {
        let st_inputs_subset = self.st_super_sub_set(
            &item_def.ingredients.inputs_set(self.params)?,
            &created_items,
        )?;

        // TODO: Calculate real nullifiers for non-empty inputs.
        let nullifiers = set_from_hashes(self.params, &HashSet::new())?;
        let st_nullifiers = self.st_nullifiers(vec![], nullifiers)?;

        // Build CommitCreation(item, nullifiers, created_items)
        Ok(st_custom!(self.ctx,
            CommitCreation() = (
                st_item_def.clone(),
                st_inputs_subset,
                st_nullifiers
            ))?)
    }
}

// Builds the public POD to commit a creation operation on-chain, with the only
// public predicate being CommitCreation.  Uses a given created_items_set as
// the root to prove that inputs were previously created.
pub fn prove_st_commit_creation(
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

    let ctx = BuildContext {
        builder: &mut builder,
        batches,
    };
    let mut item_builder = ItemBuilder::new(ctx, params);
    let st_commit_creation =
        item_builder.st_commit_creation(item_def, created_items, st_item_def)?;
    let ItemBuilder { ctx, .. } = item_builder;
    ctx.builder.reveal(&st_commit_creation);

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
    fn test_prove_st_commit_creation() {
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
        let ctx = BuildContext {
            builder: &mut builder,
            batches,
        };

        let mut item_builder = ItemBuilder::new(ctx, &params);
        let ingredients_def = IngredientsDef {
            inputs: HashSet::new(),
            key: Value::from(33).raw(),
            app_layer: HashMap::from([("foo".to_string(), Value::from("bar"))]),
        };
        let item_def = ItemDef {
            ingredients: ingredients_def,
            work: Value::from(42).raw(),
        };
        let st_item_def = item_builder.st_item_def(item_def.clone()).unwrap();

        let created_items = set_from_hashes(
            &params,
            &HashSet::from([
                hash_value(&Value::from("dummy1").raw()),
                hash_value(&Value::from("dummy2").raw()),
            ]),
        )
        .unwrap();

        let _st_commit_creation = item_builder
            .st_commit_creation(item_def, created_items, st_item_def)
            .unwrap();

        let main_pod = builder.prove(prover).unwrap();

        main_pod.pod.verify().unwrap();
    }
}

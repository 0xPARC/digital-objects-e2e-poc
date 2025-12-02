pub mod predicates;
pub mod util;

use std::collections::{HashMap, HashSet};

use pod2::middleware::{
    EMPTY_HASH, EMPTY_VALUE, Hash, Key, Params, RawValue, Statement, Value,
    containers::{Dictionary, Set},
    hash_values,
};
use pod2utils::{macros::BuildContext, set, st_custom};
use serde::{Deserialize, Serialize};

use crate::util::set_from_hashes;

pub const CONSUMED_ITEM_EXTERNAL_NULLIFIER: &str = "consumed item external nullifier";

// Rust-level definition of the ingredients of an item, used to derive the
// ingredients hash (dict root) before doing sequential work on it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngredientsDef {
    // These properties are committed on-chain
    pub inputs: HashSet<Hash>,
    // TODO: Maybe replace this with a Value -> Value map?
    pub keys: HashMap<Key, Value>,

    // These properties are used only by the application layer
    pub app_layer: HashMap<String, Value>,
}

impl IngredientsDef {
    pub fn dict(&self, params: &Params) -> pod2::middleware::Result<Dictionary> {
        let mut map = HashMap::new();
        map.insert(Key::from("inputs"), Value::from(self.inputs_set(params)?));
        map.insert(
            Key::from("keys"),
            Value::from(Dictionary::new(
                params.max_depth_mt_containers,
                self.keys.clone(),
            )?),
        );
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

// Rust-level definition of a batch, used to derive its ID (hash).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchDef {
    pub ingredients: IngredientsDef,
    pub work: RawValue,
}

impl BatchDef {
    pub fn batch_hash(&self, params: &Params) -> pod2::middleware::Result<Hash> {
        Ok(hash_values(&[
            Value::from(self.ingredients.dict(params)?),
            Value::from(self.work),
        ]))
    }

    pub fn new(ingredients: IngredientsDef, work: RawValue) -> Self {
        Self { ingredients, work }
    }
}

// Rust-level definition of an item, used to derive its ID (hash).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemDef {
    pub batch: BatchDef,
    pub index: Key,
}

impl ItemDef {
    pub fn item_key(&self) -> Value {
        self.batch.ingredients.keys[&self.index].clone()
    }

    pub fn item_hash(&self, params: &Params) -> pod2::middleware::Result<Hash> {
        Ok(hash_values(&[
            Value::from(self.batch.batch_hash(params)?),
            Value::from(self.index.hash()),
        ]))
    }

    pub fn nullifier(&self, params: &Params) -> pod2::middleware::Result<Hash> {
        Ok(hash_values(&[
            Value::from(self.item_hash(params)?),
            Value::from(CONSUMED_ITEM_EXTERNAL_NULLIFIER),
        ]))
    }

    pub fn new(batch: BatchDef, index: Key) -> Self {
        Self { batch, index }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsumableItem {
    pub item: Hash,
    pub key: RawValue,
    pub st_item_key: Statement,
}

pub struct ItemBuilder<'a> {
    pub ctx: BuildContext<'a>,
    pub params: &'a Params,
}

impl<'a> ItemBuilder<'a> {
    pub fn new(ctx: BuildContext<'a>, params: &'a Params) -> Self {
        Self { ctx, params }
    }

    fn st_super_sub_set_recursive(
        &mut self,
        inputs_set: Set,
        created_items: Set,
    ) -> anyhow::Result<Statement> {
        let mut smaller = inputs_set.clone();
        let i = smaller
            .set()
            .iter()
            .next()
            .expect("Should be nonempty.")
            .clone();
        smaller.delete(&i)?;
        let st_prev = self.st_super_sub_set(smaller.clone(), created_items.clone())?;

        // Build SubsetOfRecursive(sub, super)
        Ok(st_custom!(self.ctx,
            SubsetOfRecursive() = (
                SetContains(created_items, i),
                SetInsert(inputs_set, smaller, i),
                st_prev
            ))?)
    }

    // Adds statements to MainPodBuilder to prove inclusion of input_set in
    // created_items_set.  Returns the private SubsetOf statement.
    fn st_super_sub_set(
        &mut self,
        inputs_set: Set,
        created_items: Set,
    ) -> anyhow::Result<Statement> {
        // Build SubsetOf(inputs, created_items)
        if inputs_set.commitment() == EMPTY_HASH {
            // We manually specify the `super` wildcard value because it's otherwise unconstrained.  This
            // is only relevant in the base case where `sub` is empty, which is a subset of anything.
            Ok(st_custom!(self.ctx,
                SubsetOf(super=created_items) = (
                    Equal(inputs_set, EMPTY_VALUE),
                    Statement::None
                ))?)
        } else {
            let st_recursive = self.st_super_sub_set_recursive(inputs_set, created_items)?;
            Ok(st_custom!(self.ctx,
                SubsetOf() = (
                    Statement::None,
                    st_recursive
                ))?)
        }
    }

    pub fn st_batch_def(&mut self, batch: BatchDef) -> anyhow::Result<Statement> {
        let ingredients_dict = batch.ingredients.dict(self.params)?;
        let inputs_set = batch.ingredients.inputs_set(self.params)?;
        let batch_hash = batch.batch_hash(self.params)?;
        let keys_dict = Dictionary::new(
            self.params.max_depth_mt_containers,
            batch.ingredients.keys.clone(),
        )?;

        // Build BatchDef(item, ingredients, inputs, key, work)
        Ok(st_custom!(self.ctx,
        BatchDef() = (
            DictContains(ingredients_dict, "inputs", inputs_set),
            DictContains(ingredients_dict, "keys", keys_dict),
            HashOf(batch_hash, ingredients_dict, batch.work)
        ))?)
    }

    pub fn st_item_in_batch(&mut self, item_def: ItemDef) -> anyhow::Result<Statement> {
        let item_hash = item_def.item_hash(self.params)?;
        let batch_hash = item_def.batch.batch_hash(self.params)?;
        let keys_dict = Dictionary::new(
            self.params.max_depth_mt_containers,
            item_def.batch.ingredients.keys.clone(),
        )?;

        // Build ItemInBatch(item, batch)
        Ok(st_custom!(self.ctx,
        ItemInBatch() = (
            HashOf(item_hash, batch_hash, item_def.index.hash()),
            DictContains(keys_dict, item_def.index.name(), item_def.item_key())
        ))?)
    }

    // Adds statements to MainPodBilder to represent a generic item based on the
    // ItemDef.  Includes the following public predicates: ItemDef, ItemKey
    // Returns the Statement object for ItemDef for use in further statements.
    pub fn st_item_def(
        &mut self,
        item_def: ItemDef,
        st_batch_def: Statement,
    ) -> anyhow::Result<Statement> {
        let item_in_batch = self.st_item_in_batch(item_def.clone())?;

        let keys_dict = Dictionary::new(
            self.params.max_depth_mt_containers,
            item_def.batch.ingredients.keys.clone(),
        )?;

        // Build ItemDef(item, ingredients, inputs, key, work)
        Ok(st_custom!(self.ctx,
                      ItemDef() = (
            st_batch_def,
                          item_in_batch,
                                      DictContains(keys_dict, item_def.index.name(), item_def.item_key())
        ))?)
    }

    pub fn st_all_items_in_batch(&mut self, batch_def: BatchDef) -> anyhow::Result<Statement> {
        let batch_hash = batch_def.batch_hash(self.params)?;

        let empty_set = set!(self.params.max_depth_mt_containers)?;
        let empty_dict = Dictionary::new(self.params.max_depth_mt_containers, HashMap::new())?;

        // Build AllItemsInBatch(items, batch, keys)
        let st_all_items_in_batch_empty = st_custom!(self.ctx,
            AllItemsInBatchEmpty(batch = batch_hash) = (
                Equal(&empty_set, EMPTY_VALUE),
                Equal(&empty_dict, EMPTY_VALUE)
            ))?;
        let init_st = st_custom!(self.ctx,
            AllItemsInBatch() = (
                st_all_items_in_batch_empty,
                Statement::None
            ))?;

        let (st_all_items_in_batch, _, _) = batch_def
            .ingredients
            .keys
            .iter()
            .try_fold::<_, _, anyhow::Result<_>>(
                (init_st, empty_set.clone(), empty_dict.clone()),
                |(st_all_items_in_batch_prev, items_prev, keys_prev), (index, key)| {
                    let item_hash = hash_values(&[batch_hash.into(), index.raw().into()]);

                    let mut keys = keys_prev.clone();
                    keys.insert(index, key)?;

                    let mut items = items_prev.clone();
                    items.insert(&item_hash.into())?;

                    let st_all_items_in_batch_recursive = st_custom!(self.ctx,
                        AllItemsInBatchRecursive() = (
                            st_all_items_in_batch_prev,
                            SetInsert(items, items_prev, item_hash),
                            DictInsert(keys, keys_prev, index.name(), key),
                            HashOf(item_hash, batch_hash, index.hash())
                        ))?;

                    let st_all_items_in_batch = st_custom!(self.ctx,
                        AllItemsInBatch() = (
                            Statement::None,
                            st_all_items_in_batch_recursive
                        ))?;

                    Ok((st_all_items_in_batch, items, keys))
                },
            )?;

        Ok(st_all_items_in_batch)
    }

    pub fn st_item_key(&mut self, st_item_def: Statement) -> anyhow::Result<Statement> {
        // Build ItemKey(item, key)
        Ok(st_custom!(self.ctx,
        ItemKey() = (
            st_item_def
        ))?)
    }

    // Adds statements to MainPodBilder to prove correct nullifiers for a set of
    // inputs.  Returns the private Nullifiers.
    pub fn st_nullifiers(
        &mut self,
        // Vector of {input + key + ItemKey statements}
        sts_item_key: Vec<Statement>,
    ) -> anyhow::Result<(Statement, Set)> {
        let empty_set = set!(self.params.max_depth_mt_containers)?;
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

        let (st_nullifiers, _, nullifiers) = sts_item_key
            .into_iter()
            .try_fold::<_, _, anyhow::Result<_>>(
                (init_st, empty_set.clone(), empty_set),
                |(st_nullifiers_prev, inputs_prev, nullifiers_prev), st_item_key| {
                    let args = st_item_key.args();
                    let item = args[0].literal().unwrap().raw();
                    let key = args[1].literal().unwrap().raw();

                    let nullifier =
                        hash_values(&[key.into(), CONSUMED_ITEM_EXTERNAL_NULLIFIER.into()]);
                    let mut nullifiers = nullifiers_prev.clone();
                    nullifiers.insert(&nullifier.into())?;
                    let mut inputs = inputs_prev.clone();
                    inputs.insert(&item.into())?;
                    let st_nullifiers_recursive = st_custom!(self.ctx,
                        NullifiersRecursive() = (
                            st_item_key,
                            HashOf(nullifier, key, CONSUMED_ITEM_EXTERNAL_NULLIFIER),
                            SetInsert(nullifiers, nullifiers_prev, nullifier),
                            SetInsert(inputs, inputs_prev, item),
                            st_nullifiers_prev
                        ))?;
                    let st_nullifiers = st_custom!(self.ctx,
                        Nullifiers() = (
                            Statement::None,
                            st_nullifiers_recursive
                        ))?;
                    Ok((st_nullifiers, inputs, nullifiers))
                },
            )?;

        Ok((st_nullifiers, nullifiers))
    }

    // Builds the public POD to commit a creation operation on-chain, with the only
    // public predicate being CommitCreation.  Uses a given created_items_set as
    // the root to prove that inputs were previously created.
    pub fn st_commit_creation(
        &mut self,
        batch_def: BatchDef,
        st_nullifiers: Statement,
        created_items: Set,
        st_batch_def: Statement,
    ) -> anyhow::Result<Statement> {
        let st_inputs_subset = self.st_super_sub_set(
            batch_def.ingredients.inputs_set(self.params)?,
            created_items,
        )?;

        let st_all_items_in_batch = self.st_all_items_in_batch(batch_def)?;

        // Build CommitCreation(item, nullifiers, created_items)
        let st_commit_creation = st_custom!(self.ctx,
                                            CommitCreation() = (
                                                st_batch_def,
                                                st_all_items_in_batch,
                st_inputs_subset,
                st_nullifiers
            ))?;
        Ok(st_commit_creation)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pod2::{
        backends::plonky2::{
            basetypes::DEFAULT_VD_SET, mainpod::Prover, mock::mainpod::MockProver,
        },
        frontend::{MainPod, MainPodBuilder},
        middleware::{CustomPredicateBatch, MainPodProver, VDSet},
    };

    use super::*;
    use crate::predicates::CommitPredicates;

    #[allow(clippy::too_many_arguments)]
    fn build_item(
        params: &Params,
        vd_set: &VDSet,
        prover: &dyn MainPodProver,
        batches: &[Arc<CustomPredicateBatch>],
        created_items: &mut Set,
        blueprint: &str,
        key: i64,
        input_item_key_pods: Vec<MainPod>,
    ) -> MainPod {
        let mut builder = MainPodBuilder::new(params, vd_set);
        let mut item_builder = ItemBuilder::new(BuildContext::new(&mut builder, batches), params);

        let mut input_item_hashes = HashSet::new();
        let mut sts_item_key = Vec::new();
        for input_item_key_pod in input_item_key_pods {
            let st_item_key = input_item_key_pod.pod.pub_statements()[0].clone();
            let item_hash = Hash::from(st_item_key.args()[0].literal().unwrap().raw());
            input_item_hashes.insert(item_hash);
            sts_item_key.push(st_item_key);
            item_builder.ctx.builder.add_pod(input_item_key_pod);
        }

        let index: Key = "0".into();
        let key = Value::from(key);
        let ingredients_def = IngredientsDef {
            inputs: input_item_hashes,
            keys: [(index.clone(), key)].into_iter().collect(),
            app_layer: HashMap::from([("blueprint".to_string(), Value::from(blueprint))]),
        };

        let batch_def = BatchDef::new(ingredients_def, Value::from(42).raw());
        let item_def = ItemDef::new(batch_def.clone(), index);

        let (st_nullifiers, _nullifiers) = if sts_item_key.is_empty() {
            item_builder.st_nullifiers(sts_item_key).unwrap()
        } else {
            // The default params don't have enough custom statement verifications to fit
            // everything in a single pod, so we split it in two.
            let (st_nullifiers, nullifiers) = item_builder.st_nullifiers(sts_item_key).unwrap();
            item_builder.ctx.builder.reveal(&st_nullifiers);

            println!("Proving nullifiers_pod for {blueprint}...");
            let nullifiers_pod = builder.prove(prover).unwrap();
            nullifiers_pod.pod.verify().unwrap();
            builder = MainPodBuilder::new(params, vd_set);
            item_builder = ItemBuilder::new(BuildContext::new(&mut builder, batches), params);
            item_builder.ctx.builder.add_pod(nullifiers_pod);
            (st_nullifiers, nullifiers)
        };

        let item_hash = item_def.item_hash(params).unwrap();
        created_items.insert(&Value::from(item_hash)).unwrap();
        let st_batch_def = item_builder.st_batch_def(batch_def.clone()).unwrap();

        let _st_commit_creation = item_builder
            .st_commit_creation(
                batch_def.clone(),
                st_nullifiers,
                created_items.clone(),
                st_batch_def,
            )
            .unwrap();

        println!("Proving commit_pod for {blueprint}...");
        let commit_pod = builder.prove(prover).unwrap();
        commit_pod.pod.verify().unwrap();

        let mut builder = MainPodBuilder::new(params, vd_set);
        let mut item_builder = ItemBuilder::new(BuildContext::new(&mut builder, batches), params);
        let st_batch_def = item_builder.st_batch_def(batch_def.clone()).unwrap();
        let st_item_def = item_builder.st_item_def(item_def, st_batch_def).unwrap();
        let st_item_key = item_builder.st_item_key(st_item_def).unwrap();
        item_builder.ctx.builder.reveal(&st_item_key);

        println!("Proving item_key_pod for {blueprint}...");
        let item_key_pod = builder.prove(prover).unwrap();
        item_key_pod.pod.verify().unwrap();

        item_key_pod
    }

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

        let mut created_items = set_from_hashes(&params, &HashSet::new()).unwrap();

        // Sodium
        let item_key_pod_na = build_item(
            &params,
            vd_set,
            prover,
            batches,
            &mut created_items,
            "na",
            1,
            vec![],
        );

        // Chlorine
        let item_key_pod_cl = build_item(
            &params,
            vd_set,
            prover,
            batches,
            &mut created_items,
            "cl",
            2,
            vec![],
        );

        // Sodium Chloride
        let _item_key_pod_na_cl = build_item(
            &params,
            vd_set,
            prover,
            batches,
            &mut created_items,
            "na_cl",
            3,
            vec![item_key_pod_na, item_key_pod_cl],
        );
    }
}

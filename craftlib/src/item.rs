use std::collections::HashSet;

use pod2::{
    dict,
    middleware::{
        Hash, Params, RawValue, Value,
        containers::{Dictionary, Set},
        hash_values,
    },
};

use crate::{predicates::CONSUMED_ITEM_EXTERNAL_NULLIFIER, util::set_from_hashes};

// Rust-level definition of the ingredients of an item, used to derive the
// ingredients hash (dict root) before doing sequential work on it.
#[derive(Debug, Clone)]
pub struct IngredientsDef {
    // These properties are committed on-chain
    pub inputs: HashSet<Hash>,
    pub key: RawValue,

    // These properties are used only by item predicates
    pub blueprint: String,
    pub seed: RawValue,
}

impl IngredientsDef {
    pub fn dict(&self, params: &Params) -> pod2::middleware::Result<Dictionary> {
        dict!(params.max_depth_mt_containers, {
            "inputs" => self.inputs_set(params)?,
            "key" => self.key,
            "blueprint" => self.blueprint.clone(),
            "seed" => self.seed,
        })
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
}

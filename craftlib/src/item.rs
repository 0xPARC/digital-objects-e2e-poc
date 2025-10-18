use std::{collections::HashSet, ops::Range};

use pod2::{
    dict,
    middleware::{
        Hash, Params, RawValue, ToFields, Value,
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

    pub fn new(ingredients: IngredientsDef, work: RawValue) -> Self {
        Self { ingredients, work }
    }
}

// Reusable recipe for an item to be mined, not including the variable
// cryptographic values.
#[derive(Debug, Clone)]
pub struct MiningRecipe {
    pub inputs: HashSet<Hash>,
    pub blueprint: String,
}

impl MiningRecipe {
    pub fn prep_ingredients(&self, key: RawValue, seed: i64) -> IngredientsDef {
        IngredientsDef {
            inputs: self.inputs.clone(),
            key,
            blueprint: self.blueprint.clone(),
            seed: RawValue::from(seed),
        }
    }

    pub fn do_mining(
        &self,
        params: &Params,
        key: RawValue,
        seed_range: Range<i64>,
        mine_range: Range<u64>,
    ) -> pod2::middleware::Result<Option<IngredientsDef>> {
        for seed in seed_range {
            let ingredients = self.prep_ingredients(key, seed);
            let ingredients_hash = ingredients.hash(params)?;
            let mining_val = ingredients_hash.to_fields(params)[0];
            if mine_range.contains(&mining_val.0) {
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

#[cfg(test)]
mod tests {

    use pod2::middleware::RawValue;

    use super::*;
    use crate::constants::{COPPER_BLUEPRINT, COPPER_MINING_RANGE};

    #[test]
    fn test_mine_copper() -> anyhow::Result<()> {
        let params = Params::default();
        let mining_recipe = MiningRecipe::new_no_inputs(COPPER_BLUEPRINT.to_string());
        let key = RawValue::from(0xBADC0DE);
        let work = RawValue::from(0xDEADBEEF);

        let mine_nothing = mining_recipe.do_mining(&params, key, 0..0, COPPER_MINING_RANGE)?;
        assert!(mine_nothing.is_none());

        let mine_fail = mining_recipe.do_mining(&params, key, 0..3, COPPER_MINING_RANGE)?;
        assert!(mine_fail.is_none());

        // Seed of 2612=0xA34 is a match with hash 6647892930992163=0x000A7EE9D427E832.
        // TODO: This test is going to get slower (~2s) whenever the ingredient
        // dict definition changes.  Need a better approach to testing mining.
        let mine_success =
            mining_recipe.do_mining(&params, key, 0x9C4..0x19C4, COPPER_MINING_RANGE)?;
        assert!(mine_success.is_some());

        let ingredients_def = mine_success.unwrap();
        let item_def = ItemDef::new(ingredients_def.clone(), work);
        let item_hash = item_def.item_hash(&params)?;
        println!(
            "Mined copper {:?} from ingredients {:?}",
            item_hash,
            ingredients_def.hash(&params)?
        );

        Ok(())
    }
}

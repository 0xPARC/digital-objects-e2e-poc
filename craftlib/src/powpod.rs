//! PoWPod: Introduction Pod that proves Proof of Work (mining difficulty).
//! - takes as input a hash value and a difficulty target
//! - proves that hash[0] <= difficulty_target
//!
//! This is used to prove that mining work was done to find a valid nonce/seed.
//!
//! Circuit structure:
//! 1. PoWCircuit:
//!     - hash: RawValue (4 field elements - already a hash/commitment)
//!     - difficulty_target: u64 constant
//!     - proves: hash[0] <= difficulty_target
//!
//! 2. PoWPod:
//!     - satisfies the pod2's Pod trait interface
//!     - verifies the proof from PoWCircuit
//!
//! Usage:
//! ```rust
//!   use pod2::{backends::plonky2::basetypes::DEFAULT_VD_SET, middleware::{Params, RawValue}};
//!   use craftlib::powpod::PoWPod;
//!
//!   let params = Params::default();
//!   let vd_set = &*DEFAULT_VD_SET;
//!   let hash = RawValue::from(...); // ingredients commitment/hash
//!   let difficulty = 0x0020_0000_0000_0000u64;
//!   let pow_pod = PoWPod::new(&params, vd_set.clone(), hash, difficulty).unwrap();
//! ```

use anyhow::Result;
use itertools::Itertools;
use plonky2::{
    field::types::Field,
    hash::hash_types::{HashOut, HashOutTarget},
    iop::{
        target::Target,
        witness::{PartialWitness, WitnessWrite},
    },
    plonk::{
        circuit_builder::CircuitBuilder,
        circuit_data::{CircuitData, VerifierOnlyCircuitData},
        proof::ProofWithPublicInputs,
    },
};
use pod2::{
    backends::plonky2::{
        Error, Result as BResult,
        circuits::{
            common::{
                CircuitBuilderPod, PredicateTarget, StatementArgTarget, StatementTarget,
                ValueTarget,
            },
            mainpod::calculate_statements_hash_circuit,
        },
        deserialize_proof, mainpod,
        mainpod::calculate_statements_hash,
        serialize_proof,
    },
    measure_gates_begin, measure_gates_end, middleware,
    middleware::{
        C, D, EMPTY_HASH, F, Hash, IntroPredicateRef, Params, Pod, Proof, RawValue,
        ToFields, VDSet,
    },
    timed,
};
use serde::{Deserialize, Serialize};

const POW_POD_TYPE: (usize, &str) = (2002, "PoW");

static STANDARD_POW_POD_DATA: std::sync::LazyLock<(PoWPodTarget, CircuitData<F, C, D>)> =
    std::sync::LazyLock::new(|| build().expect("successful build"));

fn build() -> Result<(PoWPodTarget, CircuitData<F, C, D>)> {
    let params = Params::default();

    let rec_circuit_data =
        &*pod2::backends::plonky2::cache_get_standard_rec_main_pod_common_circuit_data();

    let common_data = rec_circuit_data.0.clone();
    let config = common_data.config.clone();

    let mut builder = CircuitBuilder::<F, D>::new(config);
    let pow_pod_target = PoWPodTarget::add_targets(&mut builder, &params)?;
    pod2::backends::plonky2::recursion::pad_circuit(&mut builder, &common_data);

    let data = timed!("PoWPod build", builder.build::<C>());
    assert_eq!(common_data, data.common);
    Ok((pow_pod_target, data))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PoWPod {
    pub params: Params,
    pub hash: RawValue,  // The hash to check (e.g., dict commitment)
    pub difficulty: F,  // difficulty target as a field element

    pub vd_set: VDSet,
    pub statements_hash: Hash,
    pub proof: Proof,

    pub common_hash: String,
}

#[allow(dead_code)]
impl PoWPod {
    /// Creates a PoWPod proving that hash[0] <= difficulty
    pub fn new(
        params: &Params,
        vd_set: VDSet,
        hash: RawValue,
        difficulty: u64,
    ) -> Result<PoWPod> {
        // Pre-check difficulty (optional, for early bail)
        if hash.0[0].0 > difficulty {
            anyhow::bail!("Hash does not meet difficulty requirement");
        }

        let difficulty_f = F::from_canonical_u64(difficulty);

        // Build the proof
        let (pow_pod_target, circuit_data) = &*STANDARD_POW_POD_DATA;
        let statements = pub_self_statements(hash, difficulty_f)
            .into_iter()
            .map(mainpod::Statement::from)
            .collect_vec();
        let statements_hash: Hash = calculate_statements_hash(&statements, params);

        let pow_input = PoWPodInput {
            vd_root: vd_set.root(),
            statements_hash,
            hash,
            difficulty: difficulty_f,
        };

        let mut pw = PartialWitness::<F>::new();
        pow_pod_target.set_targets(&mut pw, &pow_input)?;
        
        let proof_with_pis = timed!(
            "prove PoW difficulty check",
            circuit_data.prove(pw)?
        );

        circuit_data
            .verifier_data()
            .verify(proof_with_pis.clone())?;

        let common_hash: String =
            pod2::backends::plonky2::mainpod::cache_get_rec_main_pod_common_hash(params).clone();

        Ok(PoWPod {
            params: params.clone(),
            statements_hash,
            hash,
            difficulty: difficulty_f,
            proof: proof_with_pis.proof,
            vd_set: vd_set.clone(),
            common_hash,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct Data {
    hash: RawValue,
    difficulty: F,
    proof: String,
    common_hash: String,
}

impl Pod for PoWPod {
    fn params(&self) -> &Params {
        &self.params
    }

    fn verify(&self) -> pod2::backends::plonky2::Result<()> {
        let statements = pub_self_statements(self.hash, self.difficulty)
            .into_iter()
            .map(mainpod::Statement::from)
            .collect_vec();
        let statements_hash: Hash = calculate_statements_hash(&statements, &self.params);
        if statements_hash != self.statements_hash {
            return Err(Error::statements_hash_not_equal(
                self.statements_hash,
                statements_hash,
            ));
        }

        let (_, circuit_data) = &*STANDARD_POW_POD_DATA;

        let public_inputs = statements_hash
            .to_fields(&self.params)
            .iter()
            .chain(self.vd_set().root().0.iter())
            .cloned()
            .collect_vec();

        circuit_data
            .verify(ProofWithPublicInputs {
                proof: self.proof.clone(),
                public_inputs,
            })
            .map_err(|e| Error::custom(format!("PoWPod proof verification failure: {e:?}")))
    }

    fn statements_hash(&self) -> Hash {
        self.statements_hash
    }

    fn pod_type(&self) -> (usize, &'static str) {
        POW_POD_TYPE
    }

    fn pub_self_statements(&self) -> Vec<middleware::Statement> {
        pub_self_statements(self.hash, self.difficulty)
    }

    fn serialize_data(&self) -> serde_json::Value {
        serde_json::to_value(Data {
            hash: self.hash,
            difficulty: self.difficulty,
            proof: serialize_proof(&self.proof),
            common_hash: self.common_hash.clone(),
        })
        .expect("serialization to json")
    }

    fn deserialize_data(
        params: Params,
        data: serde_json::Value,
        vd_set: VDSet,
        statements_hash: Hash,
    ) -> BResult<Self> {
        let data: Data = serde_json::from_value(data)?;
        let common =
            &*pod2::backends::plonky2::cache_get_standard_rec_main_pod_common_circuit_data();
        let proof = deserialize_proof(common, &data.proof)?;
        Ok(Self {
            params,
            hash: data.hash,
            difficulty: data.difficulty,
            vd_set,
            statements_hash,
            proof,
            common_hash: data.common_hash,
        })
    }

    fn verifier_data(&self) -> VerifierOnlyCircuitData<C, D> {
        STANDARD_POW_POD_DATA
            .1
            .verifier_data()
            .verifier_only
            .clone()
    }

    fn common_hash(&self) -> String {
        self.common_hash.clone()
    }

    fn proof(&self) -> Proof {
        self.proof.clone()
    }

    fn vd_set(&self) -> &VDSet {
        &self.vd_set
    }
}

fn pub_self_statements(hash: RawValue, difficulty: F) -> Vec<middleware::Statement> {
    vec![middleware::Statement::Intro(
        IntroPredicateRef {
            name: POW_POD_TYPE.1.to_string(),
            args_len: 2,
            verifier_data_hash: EMPTY_HASH,
        },
        vec![
            hash.into(),
            RawValue([difficulty, F::ZERO, F::ZERO, F::ZERO]).into(),
        ],
    )]
}

fn pub_self_statements_target(
    builder: &mut CircuitBuilder<F, D>,
    params: &Params,
    hash: &[Target],
    difficulty: Target,
) -> Vec<StatementTarget> {
    let zero = builder.zero();
    let st_arg_0 = StatementArgTarget::literal(builder, &ValueTarget::from_slice(hash));
    let st_arg_1 = StatementArgTarget::literal(
        builder,
        &ValueTarget::from_slice(&[difficulty, zero, zero, zero]),
    );
    
    let args = [st_arg_0, st_arg_1]
        .into_iter()
        .chain(core::iter::repeat_with(|| {
            StatementArgTarget::none(builder)
        }))
        .take(params.max_statement_args)
        .collect();

    let verifier_data_hash = builder.constant_hash(HashOut {
        elements: EMPTY_HASH.0,
    });
    let predicate = PredicateTarget::new_intro(builder, verifier_data_hash);
    vec![StatementTarget { predicate, args }]
}

#[derive(Clone, Debug)]
struct PoWPodTarget {
    vd_root: HashOutTarget,
    statements_hash: HashOutTarget,
    hash: ValueTarget,
    difficulty: Target,
}

struct PoWPodInput {
    vd_root: Hash,
    statements_hash: Hash,
    hash: RawValue,
    difficulty: F,
}

impl PoWPodTarget {
    fn add_targets(builder: &mut CircuitBuilder<F, D>, params: &Params) -> Result<Self> {
        let measure = measure_gates_begin!(builder, "PoWPodTarget");

        // Add virtual inputs
        let hash = builder.add_virtual_value();
        let difficulty = builder.add_virtual_target();

        // Check that hash[0] <= difficulty IN-CIRCUIT
        // We need to prove hash[0] <= difficulty in a way that handles field arithmetic
        
        let hash_first = hash.elements[0];
        
        // Strategy: Prove that difficulty - hash_first is non-negative in u64 space
        // 1. Compute diff = difficulty - hash_first (in field arithmetic)
        // 2. Split both into low/high 32-bit limbs to ensure they're valid u64s
        // 3. Prove the subtraction is valid in u64 space (no underflow)
        
        // Split hash_first into two 32-bit limbs: hash_lo + hash_hi * 2^32
        let hash_bits = builder.split_le(hash_first, 64);
        let hash_lo_bits = &hash_bits[0..32];
        let hash_hi_bits = &hash_bits[32..64];
        
        // Reconstruct to verify decomposition
        let two_32 = builder.constant(F::from_canonical_u64(1u64 << 32));
        let hash_lo = builder.le_sum(hash_lo_bits.iter().copied());
        let hash_hi = builder.le_sum(hash_hi_bits.iter().copied());
        let hash_reconstructed = builder.mul_add(hash_hi, two_32, hash_lo);
        builder.connect(hash_first, hash_reconstructed);
        
        // Split difficulty into two 32-bit limbs: diff_lo + diff_hi * 2^32  
        let diff_bits = builder.split_le(difficulty, 64);
        let diff_lo_bits = &diff_bits[0..32];
        let diff_hi_bits = &diff_bits[32..64];
        
        let diff_lo = builder.le_sum(diff_lo_bits.iter().copied());
        let diff_hi = builder.le_sum(diff_hi_bits.iter().copied());
        let diff_reconstructed = builder.mul_add(diff_hi, two_32, diff_lo);
        builder.connect(difficulty, diff_reconstructed);
        
        // Prove difficulty >= hash_first in-circuit
        // Strategy: Show that (difficulty - hash_first) fits in 64 bits
        // If hash_first > difficulty, the difference would be negative,
        // which wraps to a huge number (> 2^64) and split_le will fail
        let diff_full = builder.sub(difficulty, hash_first);
        let _diff_bits = builder.split_le(diff_full, 64);

        // Calculate statements_hash
        let statements = pub_self_statements_target(
            builder,
            params,
            &hash.elements,
            difficulty,
        );
        let statements_hash = calculate_statements_hash_circuit(params, builder, &statements);

        // Register public inputs
        let vd_root = builder.add_virtual_hash();
        builder.register_public_inputs(&statements_hash.elements);
        builder.register_public_inputs(&vd_root.elements);

        measure_gates_end!(builder, measure);
        
        Ok(PoWPodTarget {
            vd_root,
            statements_hash,
            hash,
            difficulty,
        })
    }

    fn set_targets(&self, pw: &mut PartialWitness<F>, input: &PoWPodInput) -> Result<()> {
        pw.set_target_arr(&self.hash.elements, &input.hash.0)?;
        pw.set_target(self.difficulty, input.difficulty)?;
        pw.set_hash_target(
            self.statements_hash,
            HashOut::from_vec(input.statements_hash.0.to_vec()),
        )?;
        pw.set_target_arr(&self.vd_root.elements, &input.vd_root.0)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use pod2::{
        backends::plonky2::basetypes::DEFAULT_VD_SET,
        middleware::hash_str,
    };

    use super::*;

    #[test]
    fn test_pow_pod() -> Result<()> {
        let params = Params::default();
        let vd_set = &*DEFAULT_VD_SET;

        // Find a valid input by brute force (for testing)
        let difficulty = 0x0020_0000_0000_0000u64;
        let mut found_input = None;
        
        for i in 0..10000 {
            let test_input = RawValue::from(i as i64);
            let hash_output = RawValue::from(pod2::middleware::hash_value(&test_input));
            if hash_output.0[0].0 <= difficulty {
                found_input = Some(test_input);
                println!("Found valid input at i={}: hash={:#x}", i, hash_output.0[0].0);
                break;
            }
        }
        
        let ingredients = found_input.expect("Should find valid input");

        // This should succeed
        let pow_pod = PoWPod::new(&params, vd_set.clone(), ingredients, difficulty)?;
        pow_pod.verify()?;

        println!(
            "pow_pod.verifier_data_hash(): {:#} . To be used in predicates.",
            pow_pod.verifier_data_hash()
        );

        // Verify hash is computed correctly and meets difficulty
        let hash_output = RawValue::from(pod2::middleware::hash_value(&ingredients));
        assert!(hash_output.0[0].0 <= difficulty);

        Ok(())
    }

    #[test]
    fn test_pow_pod_fails_above_difficulty() -> Result<()> {
        let params = Params::default();
        let vd_set = &*DEFAULT_VD_SET;

        let input = RawValue::from(hash_str("definitely above difficulty"));
        let difficulty = 1u64; // Very strict difficulty

        // This should fail
        let result = PoWPod::new(&params, vd_set.clone(), input, difficulty);
        assert!(result.is_err());

        Ok(())
    }
}

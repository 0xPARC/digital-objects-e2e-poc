//! PoW: recursive circuit which:
//! - takes as input a custom value, which will be bounded into the recursive chain
//! - counts how many recursions have been performed

use anyhow::Result;
use plonky2::{
    field::types::Field,
    hash::{
        hash_types::{HashOut, HashOutTarget},
        poseidon::PoseidonHash,
    },
    iop::{
        target::Target,
        witness::{PartialWitness, WitnessWrite},
    },
    plonk::{
        circuit_builder::CircuitBuilder,
        circuit_data::{CircuitConfig, CircuitData, VerifierOnlyCircuitData},
        config::Hasher,
        proof::{ProofWithPublicInputs, ProofWithPublicInputsTarget},
    },
};
use pod2::{
    backends::plonky2::{
        Result as BResult,
        circuits::common::{CircuitBuilderPod, ValueTarget},
        recursion::{InnerCircuit, VerifiedProofTarget, pad_circuit},
    },
    measure_gates_begin, measure_gates_end, measure_gates_print,
    middleware::{C, D, F, HASH_SIZE, Hash, RawValue, hash_str},
};

#[cfg(test)]
mod tests {
    use std::{sync::LazyLock, time::Instant};

    use super::*;

    #[derive(Clone, Debug)]
    pub struct PowInnerCircuit {
        prev_count: Target,
        /// count contains the amount of recursive steps done
        count: Target,
        /// input that is bounded into the recursive chain
        input: ValueTarget,
        /// midput is the 'input' used for the last step of the recursion
        midput: ValueTarget,
        /// output of the recursive chain
        output: ValueTarget,
    }
    pub struct CircuitInput {
        prev_count: F,
        count: F,
        input: RawValue,
        midput: RawValue,
        output: RawValue,
    }
    impl InnerCircuit for PowInnerCircuit {
        type Input = CircuitInput;
        type Params = ();
        fn build(
            builder: &mut CircuitBuilder<F, D>,
            _params: &Self::Params,
            _verified_proofs: &[VerifiedProofTarget],
        ) -> BResult<Self> {
            let prev_count = builder.add_virtual_target();
            let input = builder.add_virtual_value();
            let midput = builder.add_virtual_value();

            let output_h = builder.hash_n_to_hash_no_pad::<PoseidonHash>(midput.elements.to_vec());
            let output = ValueTarget::from_slice(&output_h.elements.to_vec());

            // if we're at the prev_count==0, ensure that
            //   i) input==midput
            //   ii) prev_count==count==0
            let zero = builder.zero();
            let is_basecase = builder.is_equal(prev_count, zero);

            let one = builder.one();
            let count = builder.add(prev_count, one);

            // let computed_count = builder.add(prev_count, one);
            // let count_at_basecase = builder.select(is_basecase, zero, computed_count);
            // builder.connect(count, count_at_basecase);

            let input_at_basecase = ValueTarget {
                elements: std::array::from_fn(|i| {
                    builder.select(is_basecase, input.elements[i], zero)
                }),
            };
            let midput_at_basecase = ValueTarget {
                elements: std::array::from_fn(|i| {
                    builder.select(is_basecase, midput.elements[i], zero)
                }),
            };

            for i in 0..HASH_SIZE {
                builder.connect(
                    input_at_basecase.elements[i],
                    midput_at_basecase.elements[i],
                );
            }

            // register public input
            for e in input.elements.iter() {
                builder.register_public_input(*e);
            }
            for e in output.elements.iter() {
                builder.register_public_input(*e);
            }
            Ok(Self {
                prev_count,
                count,
                input,
                midput,
                output,
            })
        }
        fn set_targets(&self, pw: &mut PartialWitness<F>, input: &Self::Input) -> BResult<()> {
            pw.set_target(self.prev_count, input.prev_count)?;
            pw.set_target(self.count, input.count)?;
            pw.set_target_arr(&self.input.elements, &input.input.0)?;
            pw.set_target_arr(&self.midput.elements, &input.midput.0)?;
            pw.set_target_arr(&self.output.elements, &input.output.0)?;
            Ok(())
        }
    }

    #[test]
    fn test_inner_circuit() -> Result<()> {
        let inner_params = ();

        let starting_input = RawValue::from(hash_str("starting input"));

        // circuit
        let config = CircuitConfig::standard_recursion_zk_config();
        let mut builder = CircuitBuilder::<F, D>::new(config.clone());
        let mut pw = PartialWitness::<F>::new();

        // build circuit
        let targets = PowInnerCircuit::build(&mut builder, &inner_params, &[])?;

        // set witness
        let inner_inputs = CircuitInput {
            prev_count: F::ZERO,
            count: F::ONE,
            input: starting_input,
            midput: starting_input, // base case: midput==input
            output: RawValue::from(pod2::middleware::hash_value(&starting_input)),
            // alternatively:
            // output: RawValue::from(Hash(
            //     PoseidonHash::hash_no_pad(&starting_input.0.to_vec()).elements,
            // )),
        };
        targets.set_targets(&mut pw, &inner_inputs)?;

        // generate & verify proof
        let data = builder.build::<C>();
        let proof = data.prove(pw)?;
        data.verify(proof.clone())?;

        // Second iteration
        // circuit
        let mut builder = CircuitBuilder::<F, D>::new(config);
        let mut pw = PartialWitness::<F>::new();

        // build circuit
        let targets = PowInnerCircuit::build(&mut builder, &inner_params, &[])?;

        // set witness
        let inner_inputs = CircuitInput {
            prev_count: F::ONE,
            count: F::from_canonical_u64(2u64),
            input: starting_input,
            midput: inner_inputs.output, // base case: midput==input
            output: RawValue::from(pod2::middleware::hash_value(&inner_inputs.output)),
        };
        targets.set_targets(&mut pw, &inner_inputs)?;

        // generate & verify proof
        let data = builder.build::<C>();
        let proof = data.prove(pw)?;
        data.verify(proof.clone())?;

        Ok(())
    }
}

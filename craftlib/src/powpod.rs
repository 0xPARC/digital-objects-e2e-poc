//! PowPod: Introduction Pod that used as a "Proof of Work".
//! - takes as input a custom value, which will be bounded into the recursive chain
//! - counts how many recursions have been performed
//!
//! The 'work' comes from the proof computation cost at the each recursive step.
//!
//! An other option would be to prove the traditional PoW (hash output within a
//! range / certain amount of zeroes) inside a circuit, which is easier to
//! parallelize to gain advantatge.
//!
//! Circuits structure:
//! 1. RecursiveCircuit<PowInneCircuit>, where for each recursive step:
//!
//!   PowInnerCircuit contains the logic of:
//!     - output = hash(input)
//!     - count+1
//!
//!   And the RecursiveCircuit does the logic of:
//!     - verify previous proof of itself
//!
//! 2. PowPod:
//!     - satisfies in the pod2's Pod trait interface
//!     - verifies the proof from RecursiveCircuit<PowInnerCircuit>
//!
//!
//! Usage:
//! ```rust
//!   let n_iters: usize = 2;
//!   let input = RawValue::from(hash_str("starting input"));
//!   let pow_pod = PowPod::new(&params, n_iters, input)?;
//! ```
//! An complete example of usage can be found at the test `test_pow_pod` (bottom
//! of this file).

use anyhow::Result;
use itertools::Itertools;
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
        circuit_data::{CircuitData, VerifierOnlyCircuitData},
        proof::{ProofWithPublicInputs, ProofWithPublicInputsTarget},
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
        recursion::{
            InnerCircuit, RecursiveCircuit, RecursiveParams, VerifiedProofTarget,
            circuit::dummy as dummy_recursive, new_params as new_recursive_params,
        },
        serialize_proof,
    },
    measure_gates_begin, measure_gates_end, middleware,
    middleware::{
        C, D, EMPTY_HASH, F, HASH_SIZE, Hash, IntroPredicateRef, Params, Pod, Proof, RawValue,
        ToFields, VDSet,
    },
    timed,
};
use serde::{Deserialize, Serialize};

// ARITY is assumed to be one, this also assumed at the PowInnerCircuit.
const ARITY: usize = 1;
const NUM_PUBLIC_INPUTS: usize = 9; // 9: count + input + output
const POW_POD_TYPE: (usize, &str) = (2001, "Pow");

static STANDARD_POW_POD_DATA: std::sync::LazyLock<(PowPodTarget, CircuitData<F, C, D>)> =
    std::sync::LazyLock::new(|| build().expect("successful build"));
fn build() -> Result<(PowPodTarget, CircuitData<F, C, D>)> {
    let params = Params::default();

    // use pod2's recursion config as config for the introduction pod; which if
    // the zk feature enabled, it will have the zk property enabled
    let rec_circuit_data =
        &*pod2::backends::plonky2::cache_get_standard_rec_main_pod_common_circuit_data();

    let common_data = rec_circuit_data.0.clone();
    let config = common_data.config.clone();

    let mut builder = CircuitBuilder::<F, D>::new(config);
    let pow_pod_verify_target = PowPodTarget::add_targets(&mut builder, &params)?;
    pod2::backends::plonky2::recursion::pad_circuit(&mut builder, &common_data);

    let data = timed!("PowPod build", builder.build::<C>());
    assert_eq!(common_data, data.common);
    Ok((pow_pod_verify_target, data))
}
static POW_RECURSIVE_CIRCUIT: std::sync::LazyLock<(
    RecursiveCircuit<PowInnerCircuit>,
    RecursiveParams,
)> = std::sync::LazyLock::new(|| build_pow_recursive_circuit().expect("successful build"));
fn build_pow_recursive_circuit() -> Result<(RecursiveCircuit<PowInnerCircuit>, RecursiveParams)> {
    let recursive_params: RecursiveParams =
        new_recursive_params::<PowInnerCircuit>(ARITY, NUM_PUBLIC_INPUTS, &())?;

    let recursive_circuit = RecursiveCircuit::<PowInnerCircuit>::build(&recursive_params, &())?;

    Ok((recursive_circuit, recursive_params))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PowPod {
    pub params: Params,
    pub count: F,
    pub input: RawValue,
    pub output: RawValue, // output = H(H(H( ...H(input) ))) (count times)

    pub vd_set: VDSet,
    pub statements_hash: Hash,
    pub proof: Proof,

    pub common_hash: String,
}

#[allow(dead_code)]
impl PowPod {
    /// returns a PowPod for the given n_iters and input.
    pub fn new(params: &Params, vd_set: VDSet, n_iters: usize, input: RawValue) -> Result<PowPod> {
        let (last_iteration_values, proof_with_pis): (
            PowInnerCircuitInput,
            ProofWithPublicInputs<F, C, D>,
        ) = PowPod::get_pow_recursive_circuit_proof(n_iters, input)?;

        // generate a new PowPod from the given count, input, output
        let (count, input, output) = (
            last_iteration_values.count,
            last_iteration_values.input,
            last_iteration_values.output,
        );
        let pow_pod = timed!(
            "PowPod::new",
            PowPod::construct(params, vd_set, count, input, output, proof_with_pis)?
        );

        #[cfg(test)] // sanity check
        pow_pod.verify()?;

        Ok(pow_pod)
    }

    /// given the proof from RecursiveCircuit<PowInnerCircuit>, constructs the
    /// PowPod which verifies it.
    fn construct(
        params: &Params,
        vd_set: VDSet,
        count: F,
        input: RawValue,
        output: RawValue,
        proof: ProofWithPublicInputs<F, C, D>,
    ) -> Result<PowPod> {
        // verify the given proof in a PowPodTarget circuit
        let (pow_pod_target, circuit_data) = &*STANDARD_POW_POD_DATA;
        let statements = pub_self_statements(count, input, output)
            .into_iter()
            .map(mainpod::Statement::from)
            .collect_vec();
        let statements_hash: Hash = calculate_statements_hash(&statements, params);
        // set targets
        let pod_pow_input = PowPodVerifyInput {
            vd_root: vd_set.root(),
            statements_hash,
            proof,
        };
        let mut pw = PartialWitness::<F>::new();
        pow_pod_target.set_targets(&mut pw, &pod_pow_input)?;
        let proof_with_pis = timed!(
            "prove the pow-verification proof verification (PowPod proof)",
            circuit_data.prove(pw)?
        );
        // sanity check
        circuit_data
            .verifier_data()
            .verify(proof_with_pis.clone())?;

        let common_hash: String =
            pod2::backends::plonky2::mainpod::cache_get_rec_main_pod_common_hash(params).clone();

        Ok(PowPod {
            params: params.clone(),
            statements_hash,
            count,
            input,
            output,
            proof: proof_with_pis.proof,
            vd_set: vd_set.clone(),
            common_hash,
        })
    }

    /// computes the PoW proof out of the RecursiveCircuit<PowInnerCircuit> circuit.
    fn get_pow_recursive_circuit_proof(
        n_iters: usize,
        starting_input: RawValue,
    ) -> Result<(PowInnerCircuitInput, ProofWithPublicInputs<F, C, D>)> {
        let mut inner_inputs = PowInnerCircuitInput {
            prev_count: F::ZERO,
            count: F::ONE,
            input: starting_input,
            midput: starting_input, // base case: midput==input
            output: RawValue::from(pod2::middleware::hash_value(&starting_input)),
        };

        let (recursive_circuit, recursive_params) = &*POW_RECURSIVE_CIRCUIT;

        let (dummy_verifier_only_data, dummy_proof) =
            dummy_recursive(recursive_params.common_data(), NUM_PUBLIC_INPUTS)?;
        let mut recursive_proof = dummy_proof;
        let mut recursive_verifier_only_data = dummy_verifier_only_data;
        for i in 0..n_iters {
            if i > 0 {
                inner_inputs.prev_count = inner_inputs.count;
                inner_inputs.count += F::ONE;
                inner_inputs.midput = inner_inputs.output;
                inner_inputs.output =
                    RawValue::from(pod2::middleware::hash_value(&inner_inputs.midput));

                recursive_verifier_only_data =
                    recursive_params.verifier_data().verifier_only.clone();
            }
            recursive_proof = recursive_circuit.prove(
                &inner_inputs,
                vec![recursive_proof.clone()],
                vec![recursive_verifier_only_data.clone()],
            )?;
            recursive_params
                .verifier_data()
                .verify(recursive_proof.clone())?;

            log::debug!("{inner_inputs:?}");
            log::debug!("{:?}", recursive_proof.public_inputs);
        }
        Ok((inner_inputs, recursive_proof))
    }
}

#[derive(Serialize, Deserialize)]
struct Data {
    count: F,
    input: RawValue,
    output: RawValue,
    proof: String,
    common_hash: String,
}

impl Pod for PowPod {
    fn params(&self) -> &Params {
        &self.params
    }
    fn verify(&self) -> pod2::backends::plonky2::Result<()> {
        let statements = pub_self_statements(self.count, self.input, self.output)
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
            .map_err(|e| Error::custom(format!("PowPod proof verification failure: {e:?}")))
    }

    fn statements_hash(&self) -> Hash {
        self.statements_hash
    }

    fn pod_type(&self) -> (usize, &'static str) {
        POW_POD_TYPE
    }

    fn pub_self_statements(&self) -> Vec<middleware::Statement> {
        // exposed as a separate function for easier isolated testing
        pub_self_statements(self.count, self.input, self.output)
    }

    fn serialize_data(&self) -> serde_json::Value {
        serde_json::to_value(Data {
            count: self.count,
            input: self.input,
            output: self.output,
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
            count: data.count,
            input: data.input,
            output: data.output,
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

fn pub_self_statements(count: F, input: RawValue, output: RawValue) -> Vec<middleware::Statement> {
    vec![middleware::Statement::Intro(
        IntroPredicateRef {
            name: POW_POD_TYPE.1.to_string(),
            args_len: 3,
            verifier_data_hash: EMPTY_HASH,
        },
        vec![
            RawValue([count, F::ZERO, F::ZERO, F::ZERO]).into(),
            input.into(),
            output.into(),
        ],
    )]
}
fn pub_self_statements_target(
    builder: &mut CircuitBuilder<F, D>,
    params: &Params,
    count: Target,
    input: &[Target],
    output: &[Target],
) -> Vec<StatementTarget> {
    let zero = builder.zero();
    let st_arg_0 = StatementArgTarget::literal(
        builder,
        &ValueTarget::from_slice(&[count, zero, zero, zero]),
    );
    let st_arg_1 = StatementArgTarget::literal(builder, &ValueTarget::from_slice(input));
    let st_arg_2 = StatementArgTarget::literal(builder, &ValueTarget::from_slice(output));
    let args = [st_arg_0, st_arg_1, st_arg_2]
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
struct PowPodTarget {
    vd_root: HashOutTarget,
    statements_hash: HashOutTarget,
    proof: ProofWithPublicInputsTarget<D>,
}
struct PowPodVerifyInput {
    vd_root: Hash,
    statements_hash: Hash,
    proof: ProofWithPublicInputs<F, C, D>,
}
impl PowPodTarget {
    fn add_targets(builder: &mut CircuitBuilder<F, D>, params: &Params) -> Result<Self> {
        let measure = measure_gates_begin!(builder, "PowPodTarget");

        // Verify RecursiveCircuit<PowInnerCircuit>'s proof (with verifier_data hardcoded as constant)
        let (_, recursive_params) = &*POW_RECURSIVE_CIRCUIT;
        let verifier_data_targ =
            builder.constant_verifier_data(&recursive_params.verifier_data().verifier_only);
        let proof = builder.add_virtual_proof_with_pis(recursive_params.common_data());
        builder.verify_proof::<C>(&proof, &verifier_data_targ, recursive_params.common_data());

        // calculate statements_hash
        let count = proof.public_inputs[0];
        let input = &proof.public_inputs[1..5];
        let output = &proof.public_inputs[5..9];
        let statements = pub_self_statements_target(builder, params, count, input, output);
        let statements_hash = calculate_statements_hash_circuit(params, builder, &statements);

        // register the public inputs
        let vd_root = builder.add_virtual_hash();
        builder.register_public_inputs(&statements_hash.elements);
        builder.register_public_inputs(&vd_root.elements);

        measure_gates_end!(builder, measure);
        Ok(PowPodTarget {
            vd_root,
            statements_hash,
            proof,
        })
    }

    fn set_targets(&self, pw: &mut PartialWitness<F>, input: &PowPodVerifyInput) -> Result<()> {
        pw.set_proof_with_pis_target(&self.proof, &input.proof)?;
        pw.set_hash_target(
            self.statements_hash,
            HashOut::from_vec(input.statements_hash.0.to_vec()),
        )?;
        pw.set_target_arr(&self.vd_root.elements, &input.vd_root.0)?;

        Ok(())
    }
}

#[derive(Clone, Debug)]
struct PowInnerCircuit {
    prev_count: Target,
    count: Target,       // count contains the amount of recursive steps done
    input: ValueTarget,  // input that is bounded into the recursive chain
    midput: ValueTarget, // midput is the 'input' used for the last step of the recursion
    output: ValueTarget, // output of the recursive chain
}
#[derive(Debug)]
struct PowInnerCircuitInput {
    prev_count: F,
    count: F,
    input: RawValue,
    midput: RawValue,
    output: RawValue,
}
impl InnerCircuit for PowInnerCircuit {
    type Input = PowInnerCircuitInput;
    type Params = ();
    fn build(
        builder: &mut CircuitBuilder<F, D>,
        _params: &Self::Params,
        verified_proofs: &[VerifiedProofTarget],
    ) -> BResult<Self> {
        let prev_count = builder.add_virtual_target();
        let input = builder.add_virtual_value();
        let midput = builder.add_virtual_value();

        let output_h = builder.hash_n_to_hash_no_pad::<PoseidonHash>(midput.elements.to_vec());
        let output = ValueTarget::from_slice(output_h.elements.as_ref());

        let zero = builder.zero();
        let is_basecase = builder.is_equal(prev_count, zero);
        let is_not_basecase = builder.not(is_basecase);

        // if we're at the prev_count==0, ensure that
        // input==midput
        for i in 0..HASH_SIZE {
            builder.conditional_assert_eq(
                is_basecase.target,
                input.elements[i],
                midput.elements[i],
            );
        }

        // if we're at case prev_count>0, assert that the public_inputs of the
        // proof being verified match with the prev_count, input and midput
        builder.connect(verified_proofs[0].public_inputs[0], prev_count);
        for i in 0..HASH_SIZE {
            builder.conditional_assert_eq(
                is_not_basecase.target,
                verified_proofs[0].public_inputs[1 + i],
                input.elements[i],
            );
            builder.conditional_assert_eq(
                is_not_basecase.target,
                verified_proofs[0].public_inputs[5 + i],
                midput.elements[i],
            );
        }

        // increment count
        let one = builder.one();
        let count = builder.add(prev_count, one);

        // register public inputs: count, input, output
        builder.register_public_input(count);
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

#[cfg(test)]
mod tests {
    use plonky2::plonk::circuit_data::CircuitConfig;
    use pod2::{
        backends::plonky2::basetypes::DEFAULT_VD_SET,
        frontend, measure_gates_print,
        middleware::{Value, hash_str},
    };

    use super::*;

    // For tests only. Returns a valid VerifiedProofTarget filled with the
    // public_inputs from the given PowInnerCircuitInput, in order to run some
    // tests.
    fn empty_verified_proof_target(
        builder: &mut CircuitBuilder<F, D>,
        inp: &PowInnerCircuitInput,
    ) -> VerifiedProofTarget {
        let count = builder.constant(inp.prev_count);
        let input = builder.constants(&inp.input.0);
        let midput = if inp.prev_count.is_zero() {
            builder.constants(&inp.output.0)
        } else {
            builder.constants(&inp.midput.0)
        };
        VerifiedProofTarget {
            public_inputs: [vec![count], input, midput].concat(),
            verifier_data_hash: HashOutTarget::from_partial(&[builder.zero()], builder.zero()),
        }
    }
    #[test]
    fn test_inner_circuit() -> Result<()> {
        let inner_params = ();

        let starting_input = RawValue::from(hash_str("starting input"));

        // circuit
        let config = CircuitConfig::standard_recursion_zk_config();
        let mut builder = CircuitBuilder::<F, D>::new(config.clone());

        let inner_inputs = PowInnerCircuitInput {
            prev_count: F::ZERO,
            count: F::ONE,
            input: starting_input,
            midput: starting_input, // base case: midput==input
            output: RawValue::from(pod2::middleware::hash_value(&starting_input)),
        };

        // build circuit
        let measure = measure_gates_begin!(&builder, format!("PowInnerCircuit gates"));
        let verified_proof_target = empty_verified_proof_target(&mut builder, &inner_inputs);
        let targets =
            PowInnerCircuit::build(&mut builder, &inner_params, &[verified_proof_target])?;
        measure_gates_end!(&builder, measure);
        measure_gates_print!();
        let data = builder.build::<C>();

        // set witness
        let mut pw = PartialWitness::<F>::new();
        targets.set_targets(&mut pw, &inner_inputs)?;

        // generate & verify proof
        let proof = data.prove(pw)?;
        data.verify(proof.clone())?;

        // Second iteration
        let inner_inputs = PowInnerCircuitInput {
            prev_count: F::ONE,
            count: F::from_canonical_u64(2u64),
            input: starting_input,
            midput: inner_inputs.output, // base case: midput==input
            output: RawValue::from(pod2::middleware::hash_value(&inner_inputs.output)),
        };
        let mut builder = CircuitBuilder::<F, D>::new(config);
        let mut pw = PartialWitness::<F>::new();
        let verified_proof_target = empty_verified_proof_target(&mut builder, &inner_inputs);
        let targets =
            PowInnerCircuit::build(&mut builder, &inner_params, &[verified_proof_target])?;
        targets.set_targets(&mut pw, &inner_inputs)?;
        let data = builder.build::<C>();
        let proof = data.prove(pw)?;
        data.verify(proof.clone())?;

        Ok(())
    }

    #[test]
    fn test_recursion_on_inner_circuit() -> Result<()> {
        let starting_input = RawValue::from(hash_str("starting input"));
        let _ = PowPod::get_pow_recursive_circuit_proof(3, starting_input)?;
        Ok(())
    }

    /// test to ensure that the pub_self_statements methods match between the
    /// in-circuit and the out-circuit implementations
    #[test]
    fn test_pub_self_statements_target() -> Result<()> {
        // first generate all the circuits data so that it does not need to be
        // computed at further stages of the test (affecting the time reports)
        timed!(
            "generate POW_RECURSIVE_CIRCUIT, STANDARD_POW_POD_DATA, STANDARD_REC_MAIN_POD_CIRCUIT",
            {
                let (_, _) = &*POW_RECURSIVE_CIRCUIT;
                let (_, _) = &*STANDARD_POW_POD_DATA;
                let _ =
                    &*pod2::backends::plonky2::cache_get_standard_rec_main_pod_common_circuit_data(
                    );
            }
        );

        let params = &Default::default();

        let count = F::ONE;
        let input = RawValue::from(hash_str("starting input"));
        let output = RawValue::from(pod2::middleware::hash_value(&input));

        let st = pub_self_statements(count, input, output)
            .into_iter()
            .map(mainpod::Statement::from)
            .collect_vec();
        let statements_hash: HashOut<F> =
            HashOut::<F>::from_vec(calculate_statements_hash(&st, params).0.to_vec());

        // circuit
        let config = CircuitConfig::standard_recursion_config();
        let mut builder = CircuitBuilder::<F, D>::new(config);
        let mut pw = PartialWitness::<F>::new();

        // add targets
        let count_targ = builder.add_virtual_target();
        let input_targ = builder.add_virtual_value();
        let output_targ = builder.add_virtual_value();
        let expected_statements_hash_targ = builder.add_virtual_hash();

        // set values to targets
        pw.set_target(count_targ, count)?;
        pw.set_target_arr(&input_targ.elements, &input.0)?;
        pw.set_target_arr(&output_targ.elements, &output.0)?;
        pw.set_hash_target(expected_statements_hash_targ, statements_hash)?;

        let st_targ = pub_self_statements_target(
            &mut builder,
            params,
            count_targ,
            &input_targ.elements,
            &output_targ.elements,
        );
        let statements_hash_targ =
            calculate_statements_hash_circuit(params, &mut builder, &st_targ);

        builder.connect_hashes(expected_statements_hash_targ, statements_hash_targ);

        // generate & verify proof
        let data = builder.build::<C>();
        let proof = data.prove(pw)?;
        data.verify(proof.clone())?;

        Ok(())
    }

    #[test]
    fn test_pow_pod() -> Result<()> {
        // for this test, first generate all the circuits data so that it does
        // not need to be computed at further stages of the test (affecting the
        // time reports)
        timed!(
            "generate POW_RECURSIVE_CIRCUIT, STANDARD_POW_POD_DATA, standard_rec_main_pod_common_circuit_data",
            {
                let (_, _) = &*POW_RECURSIVE_CIRCUIT;
                let (_, _) = &*STANDARD_POW_POD_DATA;
                let _ =
                    &*pod2::backends::plonky2::cache_get_standard_rec_main_pod_common_circuit_data(
                    );
            }
        );

        let params = Params::default();
        let n_iters: usize = 2;
        let input = RawValue::from(hash_str("starting input"));

        let vd_set = &*DEFAULT_VD_SET;
        let pow_pod = PowPod::new(&params, vd_set.clone(), n_iters, input)?;
        pow_pod.verify()?;

        println!(
            "pow_pod.verifier_data_hash(): {:#}",
            pow_pod.verifier_data_hash()
        );

        // wrap the pow_pod in a 'MainPod'
        let main_pow_pod = frontend::MainPod {
            pod: Box::new(pow_pod.clone()),
            public_statements: pow_pod.pub_statements(),
            params: params.clone(),
        };

        // let expected_count = F::from_canonical_u64(n_iters as u64);
        let expected_count = Value::from(n_iters as i64);
        let expected_input = input;
        // let expected_output = pow_pod.output;

        // now generate a new MainPod from the pow_pod
        let mut main_pod_builder = frontend::MainPodBuilder::new(&params, vd_set);
        main_pod_builder.add_pod(main_pow_pod.clone());

        main_pod_builder.reveal(&main_pow_pod.public_statements[0]);

        let prover = pod2::backends::plonky2::mock::mainpod::MockProver {};
        let pod = main_pod_builder.prove(&prover)?;
        assert!(pod.pod.verify().is_ok());

        println!("going to prove the main_pod");
        let prover = mainpod::Prover {};
        let main_pod = timed!("main_pod_builder.prove", main_pod_builder.prove(&prover)?);
        let pod: Box<mainpod::MainPod> = (main_pod.pod as Box<dyn std::any::Any>)
            .downcast::<mainpod::MainPod>()
            .unwrap();
        pod.verify()?;

        let st_pow = pod.pub_statements()[0].clone();
        let count = st_pow.args()[0].literal()?;
        let input = st_pow.args()[1].literal()?;
        assert_eq!(count, expected_count);
        assert_eq!(input, Value::from(expected_input));

        Ok(())
    }
}

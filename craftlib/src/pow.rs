//! PoW: recursive circuit which:
//! - takes as input a custom value, which will be bounded into the recursive chain
//! - counts how many recursions have been performed

use anyhow::Result;
use itertools::Itertools;
use plonky2::{
    field::types::{Field, PrimeField64},
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
        circuit_data::{CircuitConfig, CircuitData, CommonCircuitData, VerifierOnlyCircuitData},
        config::Hasher,
        proof::{ProofWithPublicInputs, ProofWithPublicInputsTarget},
    },
};
use pod2::{
    backends::plonky2::{
        Error, Result as BResult,
        basetypes::DEFAULT_VD_LIST,
        circuits::{
            common::{
                CircuitBuilderPod, PredicateTarget, StatementArgTarget, StatementTarget,
                ValueTarget,
            },
            mainpod::calculate_statements_hash_circuit,
        },
        deserialize_proof, hash_common_data, mainpod,
        mainpod::calculate_statements_hash,
        recursion::{
            InnerCircuit, RecursiveCircuit, RecursiveParams, VerifiedProofTarget,
            circuit::dummy as dummy_recursive, new_params as new_recursive_params, pad_circuit,
        },
        serialization::VerifierOnlyCircuitDataSerializer,
        serialize_proof,
    },
    frontend, measure_gates_begin, measure_gates_end, measure_gates_print, middleware,
    middleware::{
        AnchoredKey, C, D, EMPTY_HASH, F, HASH_SIZE, Hash, IntroPredicateRef, Params, Pod, Proof,
        RawValue, ToFields, VDSet, Value, hash_str,
    },
    timed,
};
use serde::{Deserialize, Serialize};

const ARITY: usize = 1; // TODO set to 1 for the pow recursive circuit
const NUM_PUBLIC_INPUTS: usize = 9;
const POW_POD_TYPE: (usize, &'static str) = (2001, "PoW");

static STANDARD_POW_POD_DATA: std::sync::LazyLock<(PowPodVerifyTarget, CircuitData<F, C, D>)> =
    std::sync::LazyLock::new(|| build().expect("successful build"));

fn build() -> Result<(PowPodVerifyTarget, CircuitData<F, C, D>)> {
    let params = Params::default();

    // use pod2's recursion config as config for the introduction pod; which if
    // the zk feature enabled, it will have the zk property enabled
    let rec_circuit_data =
        &*pod2::backends::plonky2::cache_get_standard_rec_main_pod_common_circuit_data();

    let common_data = rec_circuit_data.0.clone();
    let config = common_data.config.clone();

    let mut builder = CircuitBuilder::<F, D>::new(config);
    let pow_pod_verify_target = PowPodVerifyTarget::add_targets(&mut builder, &params)?;
    pod2::backends::plonky2::recursion::pad_circuit(&mut builder, &common_data);

    let data = timed!("PowPod build", builder.build::<C>());
    assert_eq!(common_data, data.common);
    Ok((pow_pod_verify_target, data))
}

// TODO rename to POW_RECURSIVE_CIRCUIT
static POW_CIRCUIT_VERIFIER_DATA: std::sync::LazyLock<(
    RecursiveCircuit<PowInnerCircuit>,
    RecursiveParams,
)> = std::sync::LazyLock::new(|| build_pow_circuit_verifier_data().expect("successful build"));

// TODO rename to build_pow_recursive_circuit
fn build_pow_circuit_verifier_data() -> Result<(RecursiveCircuit<PowInnerCircuit>, RecursiveParams)>
{
    let recursive_params: RecursiveParams =
        new_recursive_params::<PowInnerCircuit>(ARITY, NUM_PUBLIC_INPUTS, &())?;

    let recursive_circuit = RecursiveCircuit::<PowInnerCircuit>::build(&recursive_params, &())?;

    Ok((recursive_circuit, recursive_params))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PowPod {
    params: Params,
    // recursive_params: RecursiveParams,
    count: F,
    input: RawValue,
    output: RawValue,

    vd_set: VDSet,
    statements_hash: Hash,
    proof: Proof,

    common_hash: String,
}

impl PowPod {
    fn new(
        params: &Params,
        vd_set: &VDSet,
        count: F,
        input: RawValue,
        midput: RawValue,
        output: RawValue,
        proof: ProofWithPublicInputs<F, C, D>,
    ) -> Result<PowPod> {
        // 1. prove the RecursiveCircuit<PowInnerCircuit> circuit
        let (recursive_circuit, recursive_params) = &*POW_CIRCUIT_VERIFIER_DATA;
        let pow_verify_proof = recursive_circuit.prove(
            &PowInnerCircuitInput {
                prev_count: count - F::ONE,
                count,
                input,
                midput,
                output,
            },
            vec![proof],
            vec![recursive_params.verifier_data().verifier_only.clone()],
        )?;
        // sanity check
        recursive_params
            .verifier_data()
            .verify(pow_verify_proof.clone())?;

        // 2. verify the pow_verify_proof in a PowPodVerifyTarget circuit
        let (pow_pod_target, circuit_data) = &*STANDARD_POW_POD_DATA;
        let statements = pub_self_statements(count, input, output)
            .into_iter()
            .map(mainpod::Statement::from)
            .collect_vec();
        let statements_hash: Hash = calculate_statements_hash(&statements, &params);
        // set targets
        let pod_pow_input = PowPodVerifyInput {
            vd_root: vd_set.root(),
            statements_hash,
            proof: pow_verify_proof,
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

        // let common_hash = hash_common_data(&recursive_params.common_data()).expect("hash ok");
        let common_hash: String =
            pod2::backends::plonky2::mainpod::cache_get_rec_main_pod_common_hash(params).clone();
        dbg!(&common_hash);

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

        // let circuit_data = &*STANDARD_POW_POD_DATA.1.common_data();
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
            .map_err(|e| Error::custom(format!("PowPod proof verification failure: {:?}", e)))
    }

    fn statements_hash(&self) -> Hash {
        self.statements_hash
    }

    fn pod_type(&self) -> (usize, &'static str) {
        POW_POD_TYPE
    }

    fn pub_self_statements(&self) -> Vec<middleware::Statement> {
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
        let proof = deserialize_proof(&common, &data.proof)?;
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
    // TODO rm
    // TODO use count as i64 directly instead of F
    // let count_i64 = count.to_canonical_u64() as i64;

    vec![middleware::Statement::Intro(
        IntroPredicateRef {
            name: POW_POD_TYPE.1.to_string(),
            args_len: NUM_PUBLIC_INPUTS,
            verifier_data_hash: Hash(
                // STANDARD_POW_POD_DATA
                POW_CIRCUIT_VERIFIER_DATA
                    .1
                    .verifier_data()
                    .verifier_only
                    .circuit_digest
                    .elements,
            ),
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
        &ValueTarget::from_slice(&vec![count, zero, zero, zero]),
    );
    let st_arg_1 = StatementArgTarget::literal(builder, &ValueTarget::from_slice(&input));
    let st_arg_2 = StatementArgTarget::literal(builder, &ValueTarget::from_slice(&output));
    let args = [st_arg_0, st_arg_1, st_arg_2]
        .into_iter()
        .chain(core::iter::repeat_with(|| {
            StatementArgTarget::none(builder)
        }))
        .take(params.max_statement_args)
        .collect();

    let verifier_data_hash = builder.constant_hash(HashOut {
        elements: POW_CIRCUIT_VERIFIER_DATA
            .1
            .verifier_data()
            .verifier_only
            .circuit_digest
            .elements,
    });
    let predicate = PredicateTarget::new_intro(builder, verifier_data_hash);
    vec![StatementTarget { predicate, args }]
}

#[derive(Clone, Debug)]
struct PowPodVerifyTarget {
    vd_root: HashOutTarget,
    statements_hash: HashOutTarget,
    proof: ProofWithPublicInputsTarget<D>,
}
pub struct PowPodVerifyInput {
    vd_root: Hash,
    statements_hash: Hash,
    proof: ProofWithPublicInputs<F, C, D>,
}
impl PowPodVerifyTarget {
    fn add_targets(builder: &mut CircuitBuilder<F, D>, params: &Params) -> Result<Self> {
        let measure = measure_gates_begin!(builder, "PowPodVerifyTarget");

        // Verify RecursiveCircuit<PowInnerCircuit>'s proof (with verifier_data hardcoded as constant)
        let (_, recursive_params) = &*POW_CIRCUIT_VERIFIER_DATA;
        let verifier_data_targ =
            builder.constant_verifier_data(&recursive_params.verifier_data().verifier_only);
        let proof = builder.add_virtual_proof_with_pis(&recursive_params.common_data());
        builder.verify_proof::<C>(&proof, &verifier_data_targ, &recursive_params.common_data());

        // calculate statements_hash
        // how do we know these numbers are correct??
        let count = proof.public_inputs[0];
        let input = &proof.public_inputs[1..5];
        let output = &proof.public_inputs[5..9];
        let statements = pub_self_statements_target(builder, params, count, input, output);
        let statements_hash = calculate_statements_hash_circuit(&params, builder, &statements);

        // register the public inputs
        let vd_root = builder.add_virtual_hash();
        builder.register_public_inputs(&statements_hash.elements);
        builder.register_public_inputs(&vd_root.elements);

        measure_gates_end!(builder, measure);
        Ok(PowPodVerifyTarget {
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
// TODO maybe rename to PowStepValues
#[derive(Debug)]
pub struct PowInnerCircuitInput {
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
            elements: std::array::from_fn(|i| builder.select(is_basecase, input.elements[i], zero)),
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
    use super::*;

    #[test]
    fn test_inner_circuit() -> Result<()> {
        let inner_params = ();

        let starting_input = RawValue::from(hash_str("starting input"));

        // circuit
        let config = CircuitConfig::standard_recursion_zk_config();
        let mut builder = CircuitBuilder::<F, D>::new(config.clone());

        // build circuit
        let measure = measure_gates_begin!(
            &builder,
            format!("verifier for zk 2^{}", expected_degree_bits)
        );
        let targets = PowInnerCircuit::build(&mut builder, &inner_params, &[])?;
        measure_gates_end!(&builder, measure);
        measure_gates_print!();

        // set witness
        let mut pw = PartialWitness::<F>::new();
        let inner_inputs = PowInnerCircuitInput {
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
        let inner_inputs = PowInnerCircuitInput {
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

    // TODO move to lib instead of tests
    fn get_pow_recursive_circuit(
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

        let (recursive_circuit, recursive_params) = &*POW_CIRCUIT_VERIFIER_DATA;

        let (dummy_verifier_only_data, dummy_proof) =
            dummy_recursive(recursive_params.common_data(), NUM_PUBLIC_INPUTS)?;
        let mut recursive_proof = dummy_proof;
        let mut recursive_verifier_only_data = dummy_verifier_only_data;
        for i in 0..n_iters {
            if i > 0 {
                inner_inputs.prev_count = inner_inputs.count;
                inner_inputs.count = inner_inputs.count + F::ONE;
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

            dbg!(&inner_inputs);
            dbg!(&recursive_proof.public_inputs);
        }
        Ok((inner_inputs, recursive_proof))
    }
    #[test]
    fn test_recursion_on_inner_circuit() -> Result<()> {
        let starting_input = RawValue::from(hash_str("starting input"));
        let _ = get_pow_recursive_circuit(3, starting_input)?;
        Ok(())
    }

    /// test to ensure that the pub_self_statements methods match between the
    /// in-circuit and the out-circuit implementations
    #[test]
    fn test_pub_self_statements_target() -> Result<()> {
        // first generate all the circuits data so that it does not need to be
        // computed at further stages of the test (affecting the time reports)
        timed!(
            "generate POW_CIRCUIT_VERIFIER_DATA, STANDARD_POW_POD_DATA, STANDARD_REC_MAIN_POD_CIRCUIT",
            {
                let (_, _) = &*POW_CIRCUIT_VERIFIER_DATA;
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
            calculate_statements_hash_circuit(&params, &mut builder, &st_targ);

        builder.connect_hashes(expected_statements_hash_targ, statements_hash_targ);

        // generate & verify proof
        let data = builder.build::<C>();
        let proof = data.prove(pw)?;
        data.verify(proof.clone())?;

        Ok(())
    }

    fn get_test_pow_pod(
        n_iters: usize,
        starting_input: RawValue,
    ) -> Result<(Box<dyn Pod>, Params, VDSet, F, RawValue, RawValue)> {
        let (last_iteration_values, proof_with_pis): (
            PowInnerCircuitInput,
            ProofWithPublicInputs<F, C, D>,
        ) = get_pow_recursive_circuit(n_iters, starting_input)?;

        // first generate all the circuits data so that it does not need to be
        // computed at further stages of the test (affecting the time reports)
        timed!(
            "generate ECDSA_VERIFY, STANDARD_ECDSA_POD_DATA, STANDARD_REC_MAIN_POD_CIRCUIT",
            {
                let (_, _) = &*POW_CIRCUIT_VERIFIER_DATA;
                let (_, _) = &*STANDARD_POW_POD_DATA;
                let _ =
                    &*pod2::backends::plonky2::cache_get_standard_rec_main_pod_common_circuit_data(
                    );
            }
        );
        let params = Params::default();

        let mut vds: Vec<VerifierOnlyCircuitData<C, D>> = DEFAULT_VD_LIST.clone();
        vds.push(STANDARD_POW_POD_DATA.1.verifier_only.clone());
        vds.push(
            POW_CIRCUIT_VERIFIER_DATA
                .1
                .verifier_data()
                .verifier_only
                .clone(),
        );
        let vd_set = VDSet::new(params.max_depth_mt_vds, &vds).unwrap();
        // generate a new PowPod from the given msg, pk, signature
        // This is the line
        let (count, input, midput, output) = (
            last_iteration_values.count,
            last_iteration_values.input,
            last_iteration_values.midput,
            last_iteration_values.output,
        );
        let pow_pod = timed!(
            "PowPod::new",
            PowPod::new(
                &params,
                &vd_set,
                count,
                input,
                midput,
                output,
                proof_with_pis
            )
            .unwrap()
        );
        Ok((Box::new(pow_pod), params, vd_set, count, input, output))
    }

    #[test]
    fn test_pow_pod() -> Result<()> {
        let n_iters: usize = 2;
        let starting_input = RawValue::from(hash_str("starting input"));
        let (pow_pod, params, vd_set, count, input, output) =
            get_test_pow_pod(n_iters, starting_input)?;

        pow_pod.verify().unwrap();

        // wrap the pow_pod in a 'MainPod' (RecursivePod)
        let main_pow_pod = frontend::MainPod {
            pod: pow_pod.clone(),
            public_statements: pow_pod.pub_statements(),
            params: params.clone(),
        };

        let expected_count = F::from_canonical_u64(n_iters as u64);
        let expected_input = starting_input.clone();
        let expected_output = output;

        // now generate a new MainPod from the pow_pod
        let mut main_pod_builder = frontend::MainPodBuilder::new(&params, &vd_set);
        main_pod_builder.add_pod(main_pow_pod.clone());

        // add operation that ensures that the count is as expected in the PowPod
        main_pod_builder
            .pub_op(frontend::Operation::eq(
                expected_count.to_canonical_u64() as i64,
                count.to_canonical_u64() as i64,
                // RawValue([expected_count, F::ZERO, F::ZERO, F::ZERO]).into(),
                // RawValue([count, F::ZERO, F::ZERO, F::ZERO]).into(),
            ))
            .unwrap();
        main_pod_builder
            .pub_op(frontend::Operation::eq(expected_input, input))
            .unwrap();
        main_pod_builder
            .pub_op(frontend::Operation::eq(expected_output, output))
            .unwrap();

        // TODO WIP
        // perpetuate the count
        // main_pod_builder
        //     .pub_op(frontend::Operation::copy(
        //         main_pow_pod.public_statements[0].clone(),
        //     ))
        //     .unwrap();

        let mut prover = pod2::backends::plonky2::mock::mainpod::MockProver {};
        let pod = main_pod_builder.prove(&mut prover).unwrap();
        assert!(pod.pod.verify().is_ok());

        println!("going to prove the main_pod");
        let mut prover = mainpod::Prover {};
        let main_pod = timed!(
            "main_pod_builder.prove",
            main_pod_builder.prove(&mut prover).unwrap()
        );
        let pod = (main_pod.pod as Box<dyn std::any::Any>)
            .downcast::<mainpod::MainPod>()
            .unwrap();
        pod.verify().unwrap();

        Ok(())
    }
}

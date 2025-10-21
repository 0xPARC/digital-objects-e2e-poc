use std::io::{Read, Write};

use anyhow::{Result, anyhow};
use plonky2::{
    field::types::{Field, Field64, PrimeField64},
    plonk::proof::CompressedProof,
    util::serialization::Buffer,
};
use pod2::middleware::{C, CommonCircuitData, D, F, RawValue};

use crate::ProofType;

pub fn write_elems<const N: usize>(bytes: &mut Vec<u8>, elems: &[F; N]) {
    for elem in elems {
        bytes
            .write_all(&elem.to_canonical_u64().to_le_bytes())
            .expect("vec write");
    }
}

pub fn read_elems<const N: usize>(bytes: &mut impl Read) -> Result<[F; N]> {
    let mut elems = [F::ZERO; N];
    let mut elem_bytes = [0; 8];
    #[allow(clippy::needless_range_loop)]
    for i in 0..N {
        bytes.read_exact(&mut elem_bytes)?;
        let n = u64::from_le_bytes(elem_bytes);
        if n >= F::ORDER {
            return Err(anyhow!("{n} >= F::ORDER"));
        }
        elems[i] = F::from_canonical_u64(n);
    }
    Ok(elems)
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub struct Payload {
    pub proof: PayloadProof,
    pub item: RawValue,
    pub created_items_root: RawValue,
    pub nullifiers: Vec<RawValue>,
}

const PAYLOAD_MAGIC: u16 = 0xd10b;

impl Payload {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buffer = Vec::new();
        buffer
            .write_all(&PAYLOAD_MAGIC.to_le_bytes())
            .expect("vec write");
        self.proof.write_bytes(&mut buffer);
        write_elems(&mut buffer, &self.item.0);
        write_elems(&mut buffer, &self.created_items_root.0);
        assert!(self.nullifiers.len() <= 255);
        buffer
            .write_all(&(self.nullifiers.len() as u8).to_le_bytes())
            .expect("vec write");
        for nullifier in &self.nullifiers {
            write_elems(&mut buffer, &nullifier.0);
        }
        buffer
    }

    pub fn from_bytes(bytes: &[u8], common_data: &CommonCircuitData) -> Result<Self> {
        let mut bytes = bytes;
        let magic = {
            let mut buffer = [0; 2];
            bytes.read_exact(&mut buffer)?;
            u16::from_le_bytes(buffer)
        };
        if magic != PAYLOAD_MAGIC {
            return Err(anyhow!("Invalid payload magic: {magic:04x}"));
        }

        let (proof, len) = PayloadProof::from_bytes(bytes, common_data)?;
        bytes = &bytes[len..];
        let item = RawValue(read_elems(&mut bytes)?);
        let created_items_root = RawValue(read_elems(&mut bytes)?);
        let nullifiers_len = {
            let mut buffer = [0; 1];
            bytes.read_exact(&mut buffer)?;
            u8::from_le_bytes(buffer)
        };
        let mut nullifiers = Vec::with_capacity(nullifiers_len as usize);
        for _ in 0..nullifiers_len {
            nullifiers.push(RawValue(read_elems(&mut bytes)?));
        }
        Ok(Self {
            proof,
            item,
            created_items_root,
            nullifiers,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PayloadProof {
    Plonky2(Box<CompressedProof<F, C, D>>),
    Groth16(Vec<u8>),
}

impl PayloadProof {
    pub fn write_bytes(&self, buffer: &mut Vec<u8>) {
        match self {
            PayloadProof::Plonky2(shrunk_main_pod_proof) => {
                buffer
                    .write_all(&[ProofType::Plonky2.to_byte()])
                    .expect("byte write");
                plonky2::util::serialization::Write::write_compressed_proof(
                    buffer,
                    shrunk_main_pod_proof,
                )
                .expect("vec write");
            }
            PayloadProof::Groth16(b) => {
                buffer
                    .write_all(&[ProofType::Groth16.to_byte()])
                    .expect("byte write");
                buffer
                    .write_all(&b.len().to_le_bytes())
                    .expect("g16 proof bytes length write");
                buffer.write_all(b).expect("g16 proof bytes write");
            }
        }
    }
    pub fn from_bytes(bytes: &[u8], common_data: &CommonCircuitData) -> Result<(Self, usize)> {
        let proof_type = ProofType::from_byte(&bytes[0])?;
        let bytes = &bytes[1..];
        let (proof, len): (Self, usize) = match proof_type {
            ProofType::Plonky2 => {
                let mut buffer = Buffer::new(bytes);
                let proof = plonky2::util::serialization::Read::read_compressed_proof(
                    &mut buffer,
                    common_data,
                )
                .map_err(|e| anyhow!("read_compressed_proof: {e}"))?;
                let len = buffer.pos();
                (PayloadProof::Plonky2(Box::new(proof)), len)
            }
            ProofType::Groth16 => {
                // get the length
                let len_bytes: [u8; 8] = bytes[0..8].try_into()?;
                let len: usize = u64::from_le_bytes(len_bytes) as usize;
                // return the rest of bytes of the Groth16 proof
                (PayloadProof::Groth16(bytes[8..8 + len].to_vec()), 8 + len)
            }
        };

        // len+1 because at the beginning we used the first byte for the
        // proof_type
        Ok((proof, len + 1))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use plonky2::plonk::proof::CompressedProofWithPublicInputs;
    use pod2::{
        backends::plonky2::{
            basetypes::DEFAULT_VD_SET,
            mainpod::{Prover, calculate_statements_hash},
        },
        frontend::{MainPodBuilder, Operation},
        middleware::{Params, Statement, Value, containers::Set},
    };

    use super::*;
    use crate::shrink::{ShrunkMainPodSetup, shrink_compress_pod};

    #[test]
    fn test_payload_roundtrip() {
        let test_groth = false;

        let params = Params::default();
        let vd_set = &*DEFAULT_VD_SET;
        let vds_root = vd_set.root();

        let input = r#"
        DummyCommitCrafting(item, nullifiers, created_items) = AND(
            Equal(0, 0)
        )
        "#;
        let batch = pod2::lang::parse(input, &params, &[]).unwrap().custom_batch;
        let pred = batch.predicate_ref_by_name("DummyCommitCrafting").unwrap();

        println!("ShrunkMainPod setup");
        let shrunk_main_pod_build = ShrunkMainPodSetup::new(&params).build().unwrap();
        let common_data = &shrunk_main_pod_build.circuit_data.common;

        let payload = {
            let mut builder = MainPodBuilder::new(&params, vd_set);
            let item = Value::from("dummy_item");
            let nullifiers = vec![
                Value::from(1).raw(),
                Value::from(2).raw(),
                Value::from(3).raw(),
            ];
            let nullifiers_set = Value::from(
                Set::new(
                    params.max_depth_mt_containers,
                    HashSet::from_iter(nullifiers.iter().map(|r| Value::from(*r))),
                )
                .unwrap(),
            );
            let created_items = Value::from("dummy_created_items");
            let st0 = builder.priv_op(Operation::eq(0, 0)).unwrap();
            let st_commit_crafting = builder
                .op(
                    true,
                    vec![
                        (0, item.clone()),
                        (1, nullifiers_set.clone()),
                        (2, created_items.clone()),
                    ],
                    Operation::custom(pred.clone(), [st0]),
                )
                .unwrap();
            println!("st: {st_commit_crafting:?}");

            println!("MainPod prove");
            let prover = Prover {};
            let pod = builder.prove(&prover).unwrap();
            pod.pod.verify().unwrap();

            println!("MainPod shrink & compress");
            let shrunk_main_pod_proof =
                shrink_compress_pod(&shrunk_main_pod_build, pod.clone()).unwrap();

            if test_groth {
                todo!();
            }

            Payload {
                proof: PayloadProof::Plonky2(Box::new(shrunk_main_pod_proof.clone())),
                item: item.raw(),
                created_items_root: created_items.raw(),
                nullifiers,
            }
        };

        println!("PayloadU roundtrip");
        let payload_bytes = payload.to_bytes();
        let payload_decoded = Payload::from_bytes(&payload_bytes, common_data).unwrap();
        assert_eq!(payload, payload_decoded);

        println!("Verify shrunk mainPod");

        let nullifiers_set = Value::from(
            Set::new(
                params.max_depth_mt_containers,
                HashSet::from_iter(payload.nullifiers.iter().map(|r| Value::from(*r))),
            )
            .unwrap(),
        );
        let st = Statement::Custom(
            pred,
            vec![
                Value::from(payload.item),
                nullifiers_set,
                Value::from(payload.created_items_root),
            ],
        );
        println!("st: {st:?}");

        let sts_hash = calculate_statements_hash(&[st.clone().into()], &params);
        let public_inputs = [sts_hash.0, vds_root.0].concat();
        let shrunk_main_pod_proof = match payload.proof {
            PayloadProof::Plonky2(proof) => proof,
            PayloadProof::Groth16(_) => todo!(),
        };
        let proof_with_pis = CompressedProofWithPublicInputs {
            proof: *shrunk_main_pod_proof,
            public_inputs,
        };
        let proof = proof_with_pis
            .decompress(
                &shrunk_main_pod_build
                    .circuit_data
                    .verifier_only
                    .circuit_digest,
                &shrunk_main_pod_build.circuit_data.common,
            )
            .unwrap();
        shrunk_main_pod_build.circuit_data.verify(proof).unwrap();
    }
}

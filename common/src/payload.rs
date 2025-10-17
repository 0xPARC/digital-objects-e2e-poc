use std::io::{Read, Write};

use anyhow::{Result, anyhow};
use plonky2::{
    field::types::{Field, Field64, PrimeField64},
    plonk::proof::CompressedProof,
    util::serialization::Buffer,
};
use pod2::middleware::{
    C, CommonCircuitData, CustomPredicateBatch, CustomPredicateRef, D, F, Hash, RawValue,
};

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
            return Err(anyhow!("{} >= F::ORDER", n));
        }
        elems[i] = F::from_canonical_u64(n);
    }
    Ok(elems)
}

pub fn write_custom_predicate_ref(bytes: &mut Vec<u8>, cpr: &CustomPredicateRef) {
    write_elems(bytes, &cpr.batch.id().0);
    bytes
        .write_all(&(cpr.index as u8).to_le_bytes())
        .expect("vec write");
}

pub fn read_custom_predicate_ref(bytes: &mut impl Read) -> Result<CustomPredicateRef> {
    let custom_pred_batch_id = Hash(read_elems(bytes)?);
    let custom_pred_index = {
        let mut buffer = [0; 1];
        bytes.read_exact(&mut buffer)?;
        u8::from_le_bytes(buffer) as usize
    };
    Ok(CustomPredicateRef {
        batch: CustomPredicateBatch::new_opaque("unknown".to_string(), custom_pred_batch_id),
        index: custom_pred_index,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum Payload {
    Create(PayloadCreate),
    Update(PayloadUpdate),
}

const PAYLOAD_MAGIC: u16 = 0xad00;
const PAYLOAD_TYPE_CREATE: u8 = 1;
const PAYLOAD_TYPE_UPDATE: u8 = 2;

impl Payload {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buffer = Vec::new();
        buffer
            .write_all(&PAYLOAD_MAGIC.to_le_bytes())
            .expect("vec write");
        match self {
            Self::Create(payload) => {
                buffer
                    .write_all(&PAYLOAD_TYPE_CREATE.to_le_bytes())
                    .expect("vec write");
                payload.write_bytes(&mut buffer);
            }
            Self::Update(payload) => {
                buffer
                    .write_all(&PAYLOAD_TYPE_UPDATE.to_le_bytes())
                    .expect("vec write");
                payload.write_bytes(&mut buffer);
            }
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
            return Err(anyhow!("Invalid payload magic: {:04x}", magic));
        }
        let type_ = {
            let mut buffer = [0; 1];
            bytes.read_exact(&mut buffer)?;
            u8::from_le_bytes(buffer)
        };
        Ok(match type_ {
            PAYLOAD_TYPE_CREATE => Payload::Create(PayloadCreate::from_bytes(bytes)?),
            PAYLOAD_TYPE_UPDATE => Payload::Update(PayloadUpdate::from_bytes(bytes, common_data)?),
            t => return Err(anyhow!("Invalid payload type: {}", t)),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayloadCreate {
    pub id: Hash,
    pub custom_predicate_ref: CustomPredicateRef,
    pub vds_root: Hash,
}

impl PayloadCreate {
    pub fn write_bytes(&self, buffer: &mut Vec<u8>) {
        write_elems(buffer, &self.id.0);
        write_custom_predicate_ref(buffer, &self.custom_predicate_ref);
        write_elems(buffer, &self.vds_root.0);
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut bytes = bytes;
        let id = Hash(read_elems(&mut bytes)?);
        let custom_predicate_ref = read_custom_predicate_ref(&mut bytes)?;
        let vds_root = Hash(read_elems(&mut bytes)?);
        Ok(Self {
            id,
            custom_predicate_ref,
            vds_root,
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
                .map_err(|e| anyhow!("read_compressed_proof: {}", e))?;
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PayloadUpdate {
    pub id: Hash,
    pub proof: PayloadProof,
    pub new_state: RawValue,
    pub op: RawValue,
}

impl PayloadUpdate {
    pub fn write_bytes(&self, buffer: &mut Vec<u8>) {
        write_elems(buffer, &self.id.0);
        self.proof.write_bytes(buffer);
        write_elems(buffer, &self.new_state.0);
        write_elems(buffer, &self.op.0);
    }

    pub fn from_bytes(bytes: &[u8], common_data: &CommonCircuitData) -> Result<Self> {
        let mut bytes = bytes;
        let id = Hash(read_elems(&mut bytes)?);
        let (proof, len) = PayloadProof::from_bytes(bytes, common_data)?;
        bytes = &bytes[len..];
        let new_state = RawValue(read_elems(&mut bytes)?);
        let op = RawValue(read_elems(&mut bytes)?);
        Ok(Self {
            id,
            proof,
            new_state,
            op,
        })
    }
}

#[cfg(test)]
mod tests {
    // TODO tests once predicates ready to generate a pod proof to compute
    // payload from it.
}

use alloc::vec::Vec;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::layout::{IrVersion, IR_VERSION};

pub const MAGIC: [u8; 4] = *b"EBIR";
pub const HEADER_LEN: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodecError {
    TooShort,
    BadMagic,
    Version {
        found: IrVersion,
        expected: IrVersion,
    },
    Encode,
    Decode,
}

pub fn encode<T: Serialize>(value: &T, out: &mut Vec<u8>) -> Result<(), CodecError> {
    out.clear();
    out.reserve(HEADER_LEN);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&IR_VERSION.major.to_le_bytes());
    out.extend_from_slice(&IR_VERSION.minor.to_le_bytes());
    let body = postcard::to_extend(value, Vec::new()).map_err(|_| CodecError::Encode)?;
    out.extend_from_slice(&body);
    Ok(())
}

pub fn to_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, CodecError> {
    let mut out = Vec::new();
    encode(value, &mut out)?;
    Ok(out)
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, CodecError> {
    read_header(bytes)?;
    postcard::from_bytes(&bytes[HEADER_LEN..]).map_err(|_| CodecError::Decode)
}

pub fn read_header(bytes: &[u8]) -> Result<IrVersion, CodecError> {
    if bytes.len() < HEADER_LEN {
        return Err(CodecError::TooShort);
    }
    if bytes[0..4] != MAGIC {
        return Err(CodecError::BadMagic);
    }
    let found = IrVersion {
        major: u16::from_le_bytes([bytes[4], bytes[5]]),
        minor: u16::from_le_bytes([bytes[6], bytes[7]]),
    };
    if found.major != IR_VERSION.major {
        return Err(CodecError::Version {
            found,
            expected: IR_VERSION,
        });
    }
    Ok(found)
}

pub fn hash_of(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

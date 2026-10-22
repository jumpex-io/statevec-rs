//! One canonical ustar subset, not a general archive extractor.
use super::CoreMaterialError;

pub const BLOCK: usize = 512;

pub fn header(name: &str, length: u64) -> Result<[u8; BLOCK], CoreMaterialError> {
    if name.len() > 100 || length >= (1 << 33) {
        return Err(CoreMaterialError::Header { offset: 0 });
    }
    let mut bytes = [0; BLOCK];
    bytes[..name.len()].copy_from_slice(name.as_bytes());
    bytes[100..108].copy_from_slice(b"0000600\0");
    bytes[108..116].copy_from_slice(b"0000000\0");
    bytes[116..124].copy_from_slice(b"0000000\0");
    bytes[124..135].copy_from_slice(format!("{length:011o}").as_bytes());
    bytes[136..148].copy_from_slice(b"00000000000\0");
    bytes[148..156].fill(b' ');
    bytes[156] = b'0';
    bytes[257..263].copy_from_slice(b"ustar\0");
    bytes[263..265].copy_from_slice(b"00");
    let sum: u32 = bytes.iter().map(|b| u32::from(*b)).sum();
    bytes[148..154].copy_from_slice(format!("{sum:06o}").as_bytes());
    bytes[154] = 0;
    Ok(bytes)
}

pub fn check(bytes: &[u8; BLOCK], name: &str, maximum: u64, offset: u64) -> Result<u64, CoreMaterialError> {
    let mut length = 0u64;
    for &byte in &bytes[124..135] {
        if !(b'0'..=b'7').contains(&byte) {
            return Err(CoreMaterialError::Header { offset });
        }
        length = length * 8 + u64::from(byte - b'0');
    }
    if length > maximum {
        return Err(CoreMaterialError::TooLarge { observed: length, limit: maximum });
    }
    if *bytes != header(name, length)? {
        return Err(CoreMaterialError::Header { offset });
    }
    Ok(length)
}

pub fn padding(length: u64) -> usize {
    ((BLOCK as u64 - length % BLOCK as u64) % BLOCK as u64) as usize
}

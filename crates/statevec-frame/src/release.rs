//! Format numbers shared by cluster writers and the SDK's wire readers.
//! Release 1 fixes the cluster level and client/admin protocol at one.

/// Maximum commands that release 1 can encode or replay in one stream batch.
pub const BATCH_COMMAND_CEILING: u16 = 2048;
/// Largest representable live engine extent (2^19 pages of 64 KiB).
pub const PAGE_MEMORY_CEILING_BYTES: u64 = 32 << 30;

pub const FORMAT_LEVEL: u16 = 1;
pub const PROTOCOL_NUMBER: u16 = 1;
pub const FROZEN_WIRE_NUMBER: u16 = 0;

pub const RELEASE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixFailure<const N: usize> {
    Incomplete,
    Magic { observed: [u8; N] },
    Number { observed: u16 },
}

/// Check the discriminator using only the bytes known to both readers.
/// The caller checks length and decodes the body after this check.
pub fn check_prefix<const N: usize>(
    bytes: &[u8],
    magic: &[u8; N],
    number: Option<u16>,
) -> Result<(), PrefixFailure<N>> {
    let observed = bytes.first_chunk::<N>().ok_or(PrefixFailure::Incomplete)?;
    if observed != magic {
        return Err(PrefixFailure::Magic { observed: *observed });
    }
    if let Some(number) = number {
        let prefix = bytes[N..].first_chunk::<2>().ok_or(PrefixFailure::Incomplete)?;
        let observed = u16::from_le_bytes(*prefix);
        if observed != number {
            return Err(PrefixFailure::Number { observed });
        }
    }
    Ok(())
}

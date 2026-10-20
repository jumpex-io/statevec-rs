//! Engine lifecycle inputs. Decoding establishes structure, not eligibility,
//! operator permission, material availability or successful activation.
use crate::command::{CommandPayloadError, encode_envelope};
use crate::{CoreVersion, Digest32, EngineCommandPayload};
pub use statevec_model::SYSTEM_LIFECYCLE_COMMAND_KIND;

const MAGIC: &[u8; 4] = b"SVLC";
const VERSION: u16 = 1;
const INITIALIZE: u8 = 1;
const UPDATE: u8 = 2;

/// Explicit input to a deterministic, repeatable lifecycle callback.
/// Updating never uses package bytes or the StateVec platform version as V.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleCommand<'a> {
    InitializeCore { input: &'a [u8] },
    UpdateCore { source: CoreVersion, target: CoreVersion, input: &'a [u8] },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleCodecError {
    Truncated,
    WrongMagic,
    UnsupportedVersion { version: u16 },
    UnknownOperation { tag: u8 },
    ReservedFlags { flags: u8 },
    LengthOverflow,
    TrailingBytes,
    Envelope(CommandPayloadError),
}

impl std::fmt::Display for LifecycleCodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Core lifecycle command: {self:?}")
    }
}

impl std::error::Error for LifecycleCodecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Envelope(source) => Some(source),
            _ => None,
        }
    }
}

impl<'a> LifecycleCommand<'a> {
    /// Decode only the body of an already checked lifecycle envelope.
    /// No allocation and no plugin callback occur here.
    pub fn decode(bytes: &'a [u8]) -> Result<Self, LifecycleCodecError> {
        let mut rest = bytes;
        if take::<4>(&mut rest)? != *MAGIC {
            return Err(LifecycleCodecError::WrongMagic);
        }
        let version = u16::from_le_bytes(take(&mut rest)?);
        if version != VERSION {
            return Err(LifecycleCodecError::UnsupportedVersion { version });
        }
        let [tag, flags] = take(&mut rest)?;
        if flags != 0 {
            return Err(LifecycleCodecError::ReservedFlags { flags });
        }
        match tag {
            INITIALIZE => Ok(Self::InitializeCore { input: input(rest)? }),
            UPDATE => {
                let source = CoreVersion::from_digest(Digest32::new(take(&mut rest)?));
                let target = CoreVersion::from_digest(Digest32::new(take(&mut rest)?));
                Ok(Self::UpdateCore { source, target, input: input(rest)? })
            }
            tag => Err(LifecycleCodecError::UnknownOperation { tag }),
        }
    }

    /// Encode the complete command envelope for the dedicated operator path.
    /// A caller must still bind it to the operator stream and a single-command
    /// batch. Ordinary public ingress does not accept this command.
    pub fn encode_command(
        self,
        ext_seq: u64,
        ref_ext_time_us: u64,
    ) -> Result<EngineCommandPayload, LifecycleCodecError> {
        let (header, input) = match self {
            Self::InitializeCore { input } => (12, input),
            Self::UpdateCore { input, .. } => (76, input),
        };
        let size = encoded_length(header, input.len())?;
        let mut body = Vec::with_capacity(size);
        body.extend_from_slice(MAGIC);
        body.extend_from_slice(&VERSION.to_le_bytes());
        match self {
            Self::InitializeCore { .. } => body.extend_from_slice(&[INITIALIZE, 0]),
            Self::UpdateCore { source, target, .. } => {
                body.extend_from_slice(&[UPDATE, 0]);
                body.extend_from_slice(&source.digest().bytes());
                body.extend_from_slice(&target.digest().bytes());
            }
        }
        body.extend_from_slice(&(input.len() as u32).to_le_bytes());
        body.extend_from_slice(input);
        encode_envelope(SYSTEM_LIFECYCLE_COMMAND_KIND, ext_seq, ref_ext_time_us, &body)
            .map_err(LifecycleCodecError::Envelope)
    }
}

fn encoded_length(header: usize, input: usize) -> Result<usize, LifecycleCodecError> {
    let length = header.checked_add(input).ok_or(LifecycleCodecError::LengthOverflow)?;
    // The shared outer writer checks both its u32 body length and usize header
    // addition before this caller allocates the inner body.
    crate::command::encoded_length(length).map_err(|_| LifecycleCodecError::LengthOverflow)?;
    Ok(length)
}

fn input(mut rest: &[u8]) -> Result<&[u8], LifecycleCodecError> {
    let length = usize::try_from(u32::from_le_bytes(take(&mut rest)?))
        .map_err(|_| LifecycleCodecError::LengthOverflow)?;
    if rest.len() < length {
        return Err(LifecycleCodecError::Truncated);
    }
    if rest.len() > length {
        return Err(LifecycleCodecError::TrailingBytes);
    }
    Ok(rest)
}

fn take<const N: usize>(rest: &mut &[u8]) -> Result<[u8; N], LifecycleCodecError> {
    let (value, tail) = rest.split_at_checked(N).ok_or(LifecycleCodecError::Truncated)?;
    *rest = tail;
    Ok(value.try_into().expect("fixed checked length"))
}

#[cfg(test)]
#[path = "ut_lifecycle.rs"]
mod tests;

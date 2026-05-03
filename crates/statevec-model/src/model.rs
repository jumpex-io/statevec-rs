// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use std::fmt;

// ---- Record types ----

/// Stable numeric kind assigned to a record type within a schema version.
pub type RecordKind = u8;
/// Host-assigned system identifier for one materialized record.
pub type SysId = u64;

/// Fully qualified record key used by runtime hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RecordKey {
    /// Stable record kind.
    pub kind: RecordKind,
    /// Host-assigned system identifier.
    pub sys_id: SysId,
}
/// Monotonic committed transaction sequence.
pub type TxSeq = u64;
/// Encoded primary-key bytes.
pub type PkBytes = SmallVec<[u8; 64]>;
/// Function pointer used by generated record schemas to encode primary keys.
pub type PkEncodeFn = fn(&[u8]) -> PkBytes;

/// Number of bytes reserved by the runtime record header.
///
/// These bytes are host-owned metadata and are not available to generated
/// record fields. A record with `RECORD_LEN = 64` has `48` bytes of generated
/// record data.
pub const RECORD_HEADER_SIZE: usize = 16;

/// Primitive field representation supported by StateVec schemas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    /// Boolean field encoded as one byte.
    Bool,
    /// Unsigned 8-bit integer.
    U8,
    /// Unsigned 16-bit little-endian integer.
    U16,
    /// Unsigned 32-bit little-endian integer.
    U32,
    /// Unsigned 64-bit little-endian integer.
    U64,
    /// Signed 32-bit little-endian integer.
    I32,
    /// Signed 64-bit little-endian integer.
    I64,
    /// Unsigned 128-bit little-endian integer.
    U128,
    /// Fixed-capacity byte field with an encoded logical length.
    FixedBytes,
    /// Variable-length byte field used in command and event payloads.
    VarBytes,
    /// User-defined `repr(u8)` enum.
    EnumU8,
}

/// Two-component schema version.
#[repr(C)]
#[derive(
    Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize,
)]
pub struct Version {
    main: u8,
    minor: u8,
}

const _: () = assert!(std::mem::size_of::<Version>() == 2);

impl Version {
    /// Creates a schema version from main and minor components.
    #[inline(always)]
    pub const fn new(main: u8, minor: u8) -> Self {
        Self { main, minor }
    }

    /// Returns the main schema version component.
    #[inline(always)]
    pub const fn main(self) -> u8 {
        self.main
    }

    /// Returns the minor schema version component.
    #[inline(always)]
    pub const fn minor(self) -> u8 {
        self.minor
    }

    /// Encodes the version as two bytes.
    #[inline(always)]
    pub const fn to_bytes(self) -> [u8; 2] {
        [self.main, self.minor]
    }

    /// Decodes the version from two bytes.
    #[inline(always)]
    pub const fn from_bytes(bytes: [u8; 2]) -> Self {
        Self::new(bytes[0], bytes[1])
    }
}

impl From<u16> for Version {
    #[inline(always)]
    fn from(value: u16) -> Self {
        Self::from_bytes(value.to_le_bytes())
    }
}

impl From<Version> for u16 {
    #[inline(always)]
    fn from(value: Version) -> Self {
        u16::from_le_bytes(value.to_bytes())
    }
}

/// Static definition for one schema field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldDefinition {
    /// Rust/source-level field name.
    pub name: &'static str,
    /// Stable field index within the enclosing schema object.
    pub field_index: u32,
    /// Byte offset for fixed-layout record fields.
    pub offset: u32,
    /// Encoded field type.
    pub ty: FieldType,
    /// Encoded byte length for fixed-layout fields.
    pub len: u32,
    /// Rust type name recorded for diagnostics and IDL output.
    pub rust_type_name: &'static str,
    /// Enum type name when `ty` is [`FieldType::EnumU8`].
    pub enum_type_name: Option<&'static str>,
    /// Whether this field is immutable after record creation.
    pub immutable: bool,
}

/// Static definition for one record type.
#[derive(Debug, Clone, Copy)]
pub struct RecordDefinition {
    /// Stable record kind.
    pub kind: RecordKind,
    /// Rust/source-level record name.
    pub name: &'static str,
    /// Whether a primary-key index is defined.
    pub is_pk_idx: bool,
    /// Whether range scans are expected for this record's primary-key index.
    pub support_range_scan: bool,
    /// Record data bytes excluding host-owned metadata.
    pub data_size: u32,
    /// Schema version encoded as a compact `u16`.
    pub version: u16,
    /// Optional generated primary-key encoder.
    pub pk_encode: Option<PkEncodeFn>,
    /// Active schema fields.
    pub fields: &'static [FieldDefinition],
    /// Reserved fields retained for layout compatibility.
    pub reserved_fields: &'static [FieldDefinition],
    /// Field names used to build the primary key.
    pub pk_fields: &'static [&'static str],
}

impl RecordDefinition {
    /// Returns the active field definition with the given source-level name.
    #[inline]
    pub fn field_by_name(&self, name: &str) -> Option<&FieldDefinition> {
        self.fields.iter().find(|f| f.name == name)
    }
}

/// Trait implemented by generated record types.
pub trait RecordSchema {
    /// Stable record kind.
    const KIND: RecordKind;
    /// Fixed record byte length.
    const RECORD_LEN: usize;
    /// Number of active fields.
    const FIELD_COUNT: usize;

    /// Returns the static record definition.
    fn definition() -> &'static RecordDefinition;
}

/// Primary-key encoding hook implemented by generated record types.
pub trait PkCodec {
    /// Encodes primary-key bytes from a fixed-layout record buffer.
    fn encode_pk_from_bytes(data: &[u8]) -> PkBytes;
}

/// Generated typed accessors for a fixed-layout record.
pub trait GeneratedRecordAccess: RecordSchema {
    /// Record data bytes excluding host-owned metadata.
    const DATA_LEN: usize;
    /// Read-only accessor type.
    type Access<'a>;
    /// Builder used when creating a new record.
    type NewBuilder<'a>;
    /// Builder used when updating an existing record.
    type UpdateBuilder<'a>;
    /// Wraps a record buffer in a read-only accessor.
    fn wrap<'a>(buf: &'a [u8]) -> Self::Access<'a>;
    /// Wraps a record buffer in a creation builder.
    fn wrap_new<'a>(buf: &'a mut [u8]) -> Self::NewBuilder<'a>;
    /// Wraps a record buffer in an update builder.
    fn wrap_update<'a>(buf: &'a mut [u8]) -> Self::UpdateBuilder<'a>;
}

/// Error returned when an encoded enum discriminant is unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnumDecodeError {
    /// Enum type name.
    pub type_name: &'static str,
    /// Raw encoded discriminant.
    pub raw: u8,
}

impl fmt::Display for EnumDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid {} discriminant: {}", self.type_name, self.raw)
    }
}

impl std::error::Error for EnumDecodeError {}

/// Buffer access error used by generated readers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessError {
    /// Required number of bytes.
    pub required: usize,
    /// Actual available number of bytes.
    pub actual: usize,
}

impl fmt::Display for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "buffer too small: required {} bytes, got {}",
            self.required, self.actual
        )
    }
}

impl std::error::Error for AccessError {}

/// Static definition for one `repr(u8)` enum variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnumVariantDefinition {
    /// Variant name.
    pub name: &'static str,
    /// Encoded discriminant.
    pub discriminant: u8,
}

/// Static definition for one generated `repr(u8)` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnumDefinition {
    /// Enum type name.
    pub name: &'static str,
    /// Variant definitions in declaration order.
    pub variants: &'static [EnumVariantDefinition],
}

/// Trait implemented by `#[derive(EnumU8)]`.
pub trait EnumU8: Copy + Eq + 'static {
    /// Encodes the enum as its `u8` discriminant.
    fn to_u8(self) -> u8;
    /// Decodes the enum from a `u8` discriminant.
    fn try_from_u8(v: u8) -> Result<Self, EnumDecodeError>;
    /// Returns the enum type name.
    fn type_name() -> &'static str;
    /// Returns the static enum definition.
    fn definition() -> &'static EnumDefinition;
}

/// Fixed-capacity byte value with a logical length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedBytes<const N: usize> {
    len: u16,
    buf: [u8; N],
}

impl<const N: usize> FixedBytes<N> {
    /// Creates a fixed-capacity byte value from a slice.
    pub fn new(bytes: &[u8]) -> Result<Self, AccessError> {
        if bytes.len() > N {
            return Err(AccessError {
                required: N,
                actual: bytes.len(),
            });
        }
        let mut buf = [0u8; N];
        buf[..bytes.len()].copy_from_slice(bytes);
        Ok(Self {
            len: bytes.len() as u16,
            buf,
        })
    }

    /// Returns the logical byte length.
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len as usize
    }

    /// Returns whether the logical byte content is empty.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the logical byte content.
    #[inline(always)]
    pub fn as_slice(&self) -> &[u8] {
        &self.buf[..self.len()]
    }

    /// Returns the full padded storage buffer.
    #[inline(always)]
    pub fn padded_slice(&self) -> &[u8; N] {
        &self.buf
    }

    /// Returns fixed-width bytes suitable for primary-key encoding.
    #[inline]
    pub fn pk_bytes(&self) -> PkBytes {
        let mut pk = PkBytes::new();
        pk.extend_from_slice(&self.buf);
        pk
    }
}

impl<const N: usize, const M: usize> From<&[u8; M]> for FixedBytes<N> {
    #[inline]
    fn from(bytes: &[u8; M]) -> Self {
        const { assert!(M <= N, "FixedBytes: source length exceeds capacity") };
        let mut buf = [0u8; N];
        buf[..M].copy_from_slice(bytes);
        Self { len: M as u16, buf }
    }
}

/// Incremental primary-key byte encoder.
pub struct PkBuilder {
    buf: PkBytes,
}

impl PkBuilder {
    /// Creates an empty primary-key builder.
    #[inline]
    pub fn new() -> Self {
        Self {
            buf: SmallVec::new(),
        }
    }

    /// Appends a `u8` component.
    #[inline]
    pub fn push_u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    /// Appends a big-endian `u16` component.
    #[inline]
    pub fn push_u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    /// Appends a big-endian `u32` component.
    #[inline]
    pub fn push_u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    /// Appends a big-endian `u64` component.
    #[inline]
    pub fn push_u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_be_bytes());
    }

    /// Appends an order-preserving signed `i32` component.
    #[inline]
    pub fn push_i32(&mut self, v: i32) {
        let encoded = ((v as u32) ^ 0x8000_0000).to_be_bytes();
        self.buf.extend_from_slice(&encoded);
    }

    /// Appends an order-preserving signed `i64` component.
    #[inline]
    pub fn push_i64(&mut self, v: i64) {
        let encoded = ((v as u64) ^ 0x8000_0000_0000_0000).to_be_bytes();
        self.buf.extend_from_slice(&encoded);
    }

    /// Appends raw bytes.
    #[inline]
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Finishes primary-key encoding.
    #[inline]
    pub fn finish(self) -> PkBytes {
        self.buf
    }
}

impl Default for PkBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns `Err` if `buf` is shorter than `need` bytes.
#[inline(always)]
fn check_len(buf: &[u8], need: usize) -> Result<(), AccessError> {
    if buf.len() >= need {
        Ok(())
    } else {
        Err(AccessError {
            required: need,
            actual: buf.len(),
        })
    }
}

#[inline(always)]
fn checked_need(offset: usize, len: usize, actual: usize) -> Result<usize, AccessError> {
    offset.checked_add(len).ok_or(AccessError {
        required: usize::MAX,
        actual,
    })
}

/// Panics if `buf` is shorter than `need` bytes. Used only by write primitives
/// whose callers guarantee the buffer is framework-sized.
#[inline(always)]
fn assert_len(buf: &[u8], need: usize) {
    assert!(
        buf.len() >= need,
        "write buffer too small: {} < {}",
        buf.len(),
        need
    );
}

#[inline(always)]
/// Reads a `u8` from a fixed-layout buffer.
pub fn read_u8(buf: &[u8], offset: usize) -> Result<u8, AccessError> {
    check_len(buf, checked_need(offset, 1, buf.len())?)?;
    Ok(buf[offset])
}

#[inline(always)]
/// Writes a `u8` into a fixed-layout buffer.
pub fn write_u8(buf: &mut [u8], offset: usize, v: u8) {
    assert_len(buf, offset + 1);
    buf[offset] = v;
}

#[inline(always)]
/// Reads a boolean from a fixed-layout buffer.
pub fn read_bool(buf: &[u8], offset: usize) -> Result<bool, AccessError> {
    Ok(read_u8(buf, offset)? != 0)
}

#[inline(always)]
/// Writes a boolean into a fixed-layout buffer.
pub fn write_bool(buf: &mut [u8], offset: usize, v: bool) {
    write_u8(buf, offset, if v { 1 } else { 0 });
}

#[inline(always)]
/// Reads a little-endian `u16` from a fixed-layout buffer.
pub fn read_u16_le(buf: &[u8], offset: usize) -> Result<u16, AccessError> {
    let end = checked_need(offset, 2, buf.len())?;
    check_len(buf, end)?;
    let mut tmp = [0u8; 2];
    tmp.copy_from_slice(&buf[offset..end]);
    Ok(u16::from_le_bytes(tmp))
}

#[inline(always)]
/// Writes a little-endian `u16` into a fixed-layout buffer.
pub fn write_u16_le(buf: &mut [u8], offset: usize, v: u16) {
    assert_len(buf, offset + 2);
    buf[offset..offset + 2].copy_from_slice(&v.to_le_bytes());
}

#[inline(always)]
/// Reads a little-endian `u32` from a fixed-layout buffer.
pub fn read_u32_le(buf: &[u8], offset: usize) -> Result<u32, AccessError> {
    let end = checked_need(offset, 4, buf.len())?;
    check_len(buf, end)?;
    let mut tmp = [0u8; 4];
    tmp.copy_from_slice(&buf[offset..end]);
    Ok(u32::from_le_bytes(tmp))
}

#[inline(always)]
/// Writes a little-endian `u32` into a fixed-layout buffer.
pub fn write_u32_le(buf: &mut [u8], offset: usize, v: u32) {
    assert_len(buf, offset + 4);
    buf[offset..offset + 4].copy_from_slice(&v.to_le_bytes());
}

#[inline(always)]
/// Reads a little-endian `u64` from a fixed-layout buffer.
pub fn read_u64_le(buf: &[u8], offset: usize) -> Result<u64, AccessError> {
    let end = checked_need(offset, 8, buf.len())?;
    check_len(buf, end)?;
    let mut tmp = [0u8; 8];
    tmp.copy_from_slice(&buf[offset..end]);
    Ok(u64::from_le_bytes(tmp))
}

#[inline(always)]
/// Writes a little-endian `u64` into a fixed-layout buffer.
pub fn write_u64_le(buf: &mut [u8], offset: usize, v: u64) {
    assert_len(buf, offset + 8);
    buf[offset..offset + 8].copy_from_slice(&v.to_le_bytes());
}

#[inline(always)]
/// Reads a little-endian `i32` from a fixed-layout buffer.
pub fn read_i32_le(buf: &[u8], offset: usize) -> Result<i32, AccessError> {
    Ok(read_u32_le(buf, offset)? as i32)
}

#[inline(always)]
/// Writes a little-endian `i32` into a fixed-layout buffer.
pub fn write_i32_le(buf: &mut [u8], offset: usize, v: i32) {
    write_u32_le(buf, offset, v as u32);
}

#[inline(always)]
/// Reads a little-endian `i64` from a fixed-layout buffer.
pub fn read_i64_le(buf: &[u8], offset: usize) -> Result<i64, AccessError> {
    Ok(read_u64_le(buf, offset)? as i64)
}

#[inline(always)]
/// Writes a little-endian `i64` into a fixed-layout buffer.
pub fn write_i64_le(buf: &mut [u8], offset: usize, v: i64) {
    write_u64_le(buf, offset, v as u64);
}

#[inline(always)]
/// Reads a little-endian `u128` from a fixed-layout buffer.
pub fn read_u128_le(buf: &[u8], offset: usize) -> Result<u128, AccessError> {
    let end = checked_need(offset, 16, buf.len())?;
    check_len(buf, end)?;
    let mut tmp = [0u8; 16];
    tmp.copy_from_slice(&buf[offset..end]);
    Ok(u128::from_le_bytes(tmp))
}

#[inline(always)]
/// Writes a little-endian `u128` into a fixed-layout buffer.
pub fn write_u128_le(buf: &mut [u8], offset: usize, v: u128) {
    assert_len(buf, offset + 16);
    buf[offset..offset + 16].copy_from_slice(&v.to_le_bytes());
}

#[inline(always)]
/// Reads a fixed-capacity byte field from a fixed-layout buffer.
pub fn read_fixed_bytes<const N: usize>(
    buf: &[u8],
    offset: usize,
) -> Result<FixedBytes<N>, AccessError> {
    let start = checked_need(offset, 2, buf.len())?;
    let end = checked_need(start, N, buf.len())?;
    check_len(buf, end)?;
    let len = read_u16_le(buf, offset)? as usize;
    if len > N {
        return Err(AccessError {
            required: len,
            actual: N,
        });
    }
    let mut tmp = [0u8; N];
    tmp.copy_from_slice(&buf[start..end]);
    Ok(FixedBytes {
        len: len as u16,
        buf: tmp,
    })
}

/// Writes a fixed-capacity byte field into a fixed-layout buffer.
///
/// The caller must provide a record data buffer large enough for the field
/// offset and encoded size. Generated record builders pass buffers with
/// `R::DATA_LEN` bytes.
#[inline(always)]
pub fn write_fixed_bytes<const N: usize>(buf: &mut [u8], offset: usize, v: &FixedBytes<N>) {
    assert_len(buf, offset + 2 + N);
    write_u16_le(buf, offset, v.len);
    buf[offset + 2..offset + 2 + N].fill(0);
    buf[offset + 2..offset + 2 + v.len()].copy_from_slice(v.as_slice());
}

// ---- Command types ----

/// Runtime command value with kind, external sequence, external time, and payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    command_kind: u8,
    ext_seq: u64,
    ref_ext_time_us: u64,
    payload: Vec<u8>,
}

impl Command {
    /// Creates a command envelope.
    #[inline]
    pub fn new(command_kind: u8, ext_seq: u64, ref_ext_time_us: u64, payload: Vec<u8>) -> Self {
        Self {
            command_kind,
            ext_seq,
            ref_ext_time_us,
            payload,
        }
    }

    /// Returns the command kind.
    #[inline(always)]
    pub fn command_kind(&self) -> u8 {
        self.command_kind
    }

    /// Returns the source queue sequence.
    #[inline(always)]
    pub fn ext_seq(&self) -> u64 {
        self.ext_seq
    }

    /// Returns the dedupe key used by ingress.
    #[inline(always)]
    pub fn ingress_dedupe_key(&self) -> u64 {
        self.ext_seq
    }

    /// Returns the source-provided reference time in microseconds.
    #[inline(always)]
    pub fn ref_ext_time_us(&self) -> u64 {
        self.ref_ext_time_us
    }

    /// Returns the payload byte length.
    #[inline(always)]
    pub fn payload_len(&self) -> usize {
        self.payload.len()
    }

    /// Returns the encoded command payload.
    #[inline(always)]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Consumes the command and returns the encoded payload.
    #[inline(always)]
    pub fn into_payload(self) -> Vec<u8> {
        self.payload
    }
}

// ---- VarBytes helpers (payload-layer only) ----

#[inline(always)]
/// Reads a payload-layer variable byte field and returns the field plus next offset.
pub fn read_var_bytes(data: &[u8], offset: usize) -> Result<(&[u8], usize), AccessError> {
    let len = read_u16_le(data, offset)? as usize;
    let end = checked_need(checked_need(offset, 2, data.len())?, len, data.len())?;
    if end > data.len() {
        return Err(AccessError {
            required: end,
            actual: data.len(),
        });
    }
    Ok((&data[offset + 2..end], end))
}

#[inline]
/// Appends a payload-layer variable byte field.
pub fn write_var_bytes(buf: &mut Vec<u8>, content: &[u8]) -> Result<(), AccessError> {
    let len = u16::try_from(content.len()).map_err(|_| AccessError {
        required: content.len(),
        actual: u16::MAX as usize,
    })?;
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(content);
    Ok(())
}

// ---- Payload schema types ----

/// Static definition for one command or event payload field.
#[derive(Debug, Clone, Copy)]
pub struct PayloadFieldDefinition {
    /// Rust/source-level field name.
    pub name: &'static str,
    /// Stable field index within the payload.
    pub field_index: u32,
    /// Encoded field type.
    pub ty: FieldType,
    /// Rust type name recorded for diagnostics and IDL output.
    pub rust_type_name: &'static str,
    /// Enum type name when `ty` is [`FieldType::EnumU8`].
    pub enum_type_name: Option<&'static str>,
    /// Fixed encoded size for fixed-width payload fields.
    pub fixed_size: Option<u32>,
}

/// Static definition for one command type.
#[derive(Debug, Clone, Copy)]
pub struct CommandDefinition {
    /// Stable command kind.
    pub kind: u8,
    /// Rust/source-level command name.
    pub name: &'static str,
    /// Schema version encoded as a compact `u16`.
    pub version: u16,
    /// Payload fields.
    pub fields: &'static [PayloadFieldDefinition],
}

/// Trait implemented by generated command types.
pub trait CommandSchema {
    /// Stable command kind.
    const KIND: u8;
    /// Returns the static command definition.
    fn definition() -> &'static CommandDefinition;
}

/// Generated typed accessors for command payloads.
pub trait GeneratedCommandAccess: CommandSchema {
    /// Read-only payload accessor type.
    type Access<'a>;
    /// Payload builder type.
    type Builder;
    /// Wraps encoded payload bytes in a read-only accessor.
    fn wrap(data: &[u8]) -> Self::Access<'_>;
    /// Creates a payload builder.
    fn builder() -> Self::Builder;
}

// ---- Event types ----

/// Runtime event value emitted by a command transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    event_kind: u8,
    event_seq: u32,
    payload: Vec<u8>,
}

impl Event {
    /// Creates an event.
    #[inline]
    pub fn new(event_kind: u8, event_seq: u32, payload: Vec<u8>) -> Self {
        Self {
            event_kind,
            event_seq,
            payload,
        }
    }

    /// Returns the event kind.
    #[inline(always)]
    pub fn event_kind(&self) -> u8 {
        self.event_kind
    }

    /// Returns the event sequence within the transaction.
    #[inline(always)]
    pub fn event_seq(&self) -> u32 {
        self.event_seq
    }

    /// Returns the encoded event payload.
    #[inline(always)]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Consumes the event and returns the encoded payload.
    #[inline(always)]
    pub fn into_payload(self) -> Vec<u8> {
        self.payload
    }

    /// Attaches a committed transaction sequence to this event.
    #[inline]
    pub fn into_frame(self, tx_seq: TxSeq) -> EventFrame {
        EventFrame::new(tx_seq, self.event_seq, self.event_kind, self.payload)
    }
}

/// Event plus committed transaction sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventFrame {
    tx_seq: TxSeq,
    event_seq: u32,
    event_kind: u8,
    payload: Vec<u8>,
}

impl EventFrame {
    /// Creates an event frame.
    #[inline]
    pub fn new(tx_seq: u64, event_seq: u32, event_kind: u8, payload: Vec<u8>) -> Self {
        Self {
            tx_seq,
            event_seq,
            event_kind,
            payload,
        }
    }

    /// Returns the committed transaction sequence.
    #[inline(always)]
    pub fn tx_seq(&self) -> TxSeq {
        self.tx_seq
    }

    /// Returns the event sequence within the transaction.
    #[inline(always)]
    pub fn event_seq(&self) -> u32 {
        self.event_seq
    }

    /// Returns the event kind.
    #[inline(always)]
    pub fn event_kind(&self) -> u8 {
        self.event_kind
    }

    /// Returns the encoded frame byte length.
    #[inline(always)]
    pub fn frame_len(&self) -> usize {
        24 + self.payload.len()
    }

    /// Returns the encoded event payload.
    #[inline(always)]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Consumes the frame and returns the encoded payload.
    #[inline(always)]
    pub fn into_payload(self) -> Vec<u8> {
        self.payload
    }

    /// Splits the frame into transaction sequence and event.
    #[inline]
    pub fn into_parts(self) -> (TxSeq, Event) {
        (
            self.tx_seq,
            Event {
                event_kind: self.event_kind,
                event_seq: self.event_seq,
                payload: self.payload,
            },
        )
    }
}

/// Static definition for one event type.
#[derive(Debug, Clone, Copy)]
pub struct EventDefinition {
    /// Stable event kind.
    pub kind: u8,
    /// Rust/source-level event name.
    pub name: &'static str,
    /// Schema version encoded as a compact `u16`.
    pub version: u16,
    /// Payload fields.
    pub fields: &'static [PayloadFieldDefinition],
}

/// Trait implemented by generated event types.
pub trait EventSchema {
    /// Stable event kind.
    const KIND: u8;
    /// Returns the static event definition.
    fn definition() -> &'static EventDefinition;
}

/// Generated typed accessors for event payloads.
pub trait GeneratedEventAccess: EventSchema {
    /// Read-only payload accessor type.
    type Access<'a>;
    /// Payload builder type.
    type Builder;
    /// Wraps encoded payload bytes in a read-only accessor.
    fn wrap(data: &[u8]) -> Self::Access<'_>;
    /// Creates a payload builder.
    fn builder() -> Self::Builder;
}

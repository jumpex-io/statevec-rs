// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use std::fmt;

// ---- Record types ----

/// Stable numeric kind assigned to a record type within a schema version.
pub type RecordKind = u16;
/// Stable numeric kind assigned to a command type within a schema version.
pub type CommandKind = u16;
/// Stable numeric kind assigned to an event type within a schema version.
pub type EventKind = u16;
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
/// Encoded unique-key bytes.
/// Single- and two-component u64 keys stay inline; longer keys may spill.
/// Inline capacity is independent of the canonical index's encoded-size limit.
pub type KeyBytes = SmallVec<[u8; 16]>;
/// Function pointer used by generated record schemas to encode unique keys.
pub type KeyEncodeFn = fn(&[u8]) -> KeyBytes;
/// Maximum number of unique keys supported by one record kind.
pub const MAX_UNIQUE_KEYS_PER_RECORD: usize = 3;
/// Maximum number of canonical indexes supported by one record kind.
pub const MAX_CANONICAL_INDEXES_PER_RECORD: usize = 2;
/// Maximum number of user fields in one canonical index.
pub const MAX_CANONICAL_INDEX_FIELDS: usize = 3;
/// Maximum encoded user-key bytes in one canonical index entry.
pub const MAX_CANONICAL_INDEX_KEY_BYTES: usize = 64;
/// Invalid kind sentinel shared by record, command, and event namespaces.
pub const INVALID_KIND: u16 = 0;
/// First user/application kind value.
pub const USER_KIND_MIN: u16 = 1;
/// Last user/application kind value.
pub const USER_KIND_MAX: u16 = 0xEFFF;
/// First StateVec engine-owned system kind value.
pub const SYSTEM_KIND_MIN: u16 = 0xF000;
/// Last StateVec engine-owned system kind value.
pub const SYSTEM_KIND_MAX: u16 = u16::MAX;
/// Reserved system schema metadata record kind.
pub const SYSTEM_SCHEMA_RECORD_KIND: RecordKind = 0xF000;
/// Reserved system runtime manifest metadata record kind.
pub const SYSTEM_RUNTIME_MANIFEST_RECORD_KIND: RecordKind = 0xF001;
/// Reserved future schema/runtime cutover command kind.
pub const SYSTEM_SCHEMA_CUTOVER_COMMAND_KIND: CommandKind = 0xF000;
/// Reserved future runtime manifest cutover command kind.
pub const SYSTEM_RUNTIME_MANIFEST_CUTOVER_COMMAND_KIND: CommandKind = 0xF001;
/// Reserved future schema/runtime cutover event kind.
pub const SYSTEM_SCHEMA_CUTOVER_EVENT_KIND: EventKind = 0xF000;
/// Reserved future runtime manifest cutover event kind.
pub const SYSTEM_RUNTIME_MANIFEST_CUTOVER_EVENT_KIND: EventKind = 0xF001;

/// Number of bytes reserved by the runtime record header.
///
/// These bytes are host-owned metadata and are not available to generated
/// record fields. A record with `RECORD_LEN = 64` has `40` bytes of generated
/// record data.
pub const RECORD_HEADER_SIZE: usize = 24;

/// Maximum physical record slot length, including the host-owned header.
/// The runtime page layout reserves at most half of a 64 KiB page per slot.
pub const MAX_RECORD_LEN: usize = 32 * 1024;

/// Maximum decimal scale supported by [`Decimal`].
pub const MAX_DECIMAL_SCALE: u8 = 38;

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
    /// Fixed-point signed decimal encoded as an i128 mantissa.
    Decimal,
}

/// Optional presentation/consumer semantic for a schema field.
///
/// Semantic tags do not change encoded bytes, deterministic replay, or schema
/// fingerprints. They are metadata for IDL consumers and downstream projectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticTag {
    /// Bytes should be rendered as text.
    Text,
    /// Integer micros since the Unix epoch.
    TimestampMicros,
    /// Sixteen canonical UUID bytes.
    Uuid,
}

impl SemanticTag {
    /// Stable IDL spelling for this semantic tag.
    #[inline(always)]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::TimestampMicros => "timestampMicros",
            Self::Uuid => "uuid",
        }
    }
}

impl std::str::FromStr for SemanticTag {
    type Err = SemanticTagParseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "text" => Ok(Self::Text),
            "timestampMicros" => Ok(Self::TimestampMicros),
            "uuid" => Ok(Self::Uuid),
            other => Err(SemanticTagParseError { value: other.to_string() }),
        }
    }
}

/// Error returned when a semantic tag string is unknown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticTagParseError {
    /// Unknown semantic value.
    pub value: String,
}

impl fmt::Display for SemanticTagParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown semantic tag: {}", self.value)
    }
}

impl std::error::Error for SemanticTagParseError {}

/// Error returned when a semantic tag is not valid for a field's storage type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticCompatibilityError {
    /// Field type carrying the semantic tag.
    pub ty: FieldType,
    /// Fixed encoded size when available.
    pub fixed_size: Option<u32>,
    /// Requested semantic tag.
    pub semantic: SemanticTag,
}

impl fmt::Display for SemanticCompatibilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "semantic {} is not valid for {:?} with fixed size {:?}",
            self.semantic.as_str(),
            self.ty,
            self.fixed_size
        )
    }
}

impl std::error::Error for SemanticCompatibilityError {}

/// Error returned when decimal scale metadata is not valid for a field type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecimalScaleCompatibilityError {
    /// Field type carrying the scale metadata.
    pub ty: FieldType,
    /// Declared decimal scale.
    pub decimal_scale: Option<u8>,
}

impl fmt::Display for DecimalScaleCompatibilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (self.ty, self.decimal_scale) {
            (FieldType::Decimal, None) => write!(f, "decimal field must declare decimal scale"),
            (FieldType::Decimal, Some(scale)) if scale > MAX_DECIMAL_SCALE => {
                write!(f, "decimal scale {} exceeds maximum {}", scale, MAX_DECIMAL_SCALE)
            }
            (ty, Some(scale)) => write!(f, "non-decimal field {:?} cannot declare decimal scale {}", ty, scale),
            _ => write!(f, "invalid decimal scale metadata"),
        }
    }
}

impl std::error::Error for DecimalScaleCompatibilityError {}

/// Validates decimal scale metadata against a field's storage type.
#[inline]
pub fn validate_decimal_scale_compat(
    ty: FieldType,
    decimal_scale: Option<u8>,
) -> Result<(), DecimalScaleCompatibilityError> {
    match (ty, decimal_scale) {
        (FieldType::Decimal, Some(scale)) if scale <= MAX_DECIMAL_SCALE => Ok(()),
        (FieldType::Decimal, _) => Err(DecimalScaleCompatibilityError { ty, decimal_scale }),
        (_, None) => Ok(()),
        (_, Some(_)) => Err(DecimalScaleCompatibilityError { ty, decimal_scale }),
    }
}

/// Validates that a semantic tag matches a field's storage type.
#[inline]
pub fn validate_semantic_compat(
    ty: FieldType,
    fixed_size: Option<u32>,
    semantic: Option<SemanticTag>,
) -> Result<(), SemanticCompatibilityError> {
    let Some(semantic) = semantic else {
        return Ok(());
    };
    let valid = match semantic {
        SemanticTag::Text => matches!(ty, FieldType::VarBytes),
        SemanticTag::TimestampMicros => matches!(ty, FieldType::U64),
        SemanticTag::Uuid => matches!(ty, FieldType::FixedBytes) && fixed_size == Some(18),
    };
    if valid { Ok(()) } else { Err(SemanticCompatibilityError { ty, fixed_size, semantic }) }
}

/// Error returned when repeated payload metadata is not valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatedCompatibilityError {
    /// Command payloads do not support repeated fields in the first cut.
    CommandRepeatedUnsupported,
    /// Repeated fields must declare a non-zero maximum element count.
    MissingElementCountMax,
    /// Only fixed-width scalar element types are supported in the first cut.
    UnsupportedElementType(FieldType),
    /// Repeated field metadata is present on a non-repeated field.
    NonRepeatedFieldHasMax(u16),
}

impl fmt::Display for RepeatedCompatibilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CommandRepeatedUnsupported => write!(f, "repeated command payload fields are not supported"),
            Self::MissingElementCountMax => write!(f, "repeated payload field must declare element_count_max > 0"),
            Self::UnsupportedElementType(ty) => write!(f, "repeated payload element type {:?} is not supported", ty),
            Self::NonRepeatedFieldHasMax(max) => write!(f, "non-repeated payload field has element_count_max {}", max),
        }
    }
}

impl std::error::Error for RepeatedCompatibilityError {}

/// Validates repeated payload metadata.
#[inline]
pub fn validate_repeated_payload_compat(
    ty: FieldType,
    repeated: bool,
    element_count_max: u16,
    command_payload: bool,
) -> Result<(), RepeatedCompatibilityError> {
    if !repeated {
        if element_count_max == 0 {
            return Ok(());
        }
        return Err(RepeatedCompatibilityError::NonRepeatedFieldHasMax(element_count_max));
    }
    if command_payload {
        return Err(RepeatedCompatibilityError::CommandRepeatedUnsupported);
    }
    if element_count_max == 0 {
        return Err(RepeatedCompatibilityError::MissingElementCountMax);
    }
    match ty {
        FieldType::Bool
        | FieldType::U8
        | FieldType::U16
        | FieldType::U32
        | FieldType::U64
        | FieldType::I32
        | FieldType::I64
        | FieldType::U128
        | FieldType::FixedBytes
        | FieldType::EnumU8
        | FieldType::Decimal => Ok(()),
        FieldType::VarBytes => Err(RepeatedCompatibilityError::UnsupportedElementType(ty)),
    }
}

/// Two-component schema version.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
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
    /// Decimal scale when `ty` is [`FieldType::Decimal`].
    pub decimal_scale: Option<u8>,
    /// Optional downstream rendering/consumer semantic metadata.
    pub semantic: Option<SemanticTag>,
    /// Whether this field is immutable after record creation.
    pub immutable: bool,
}

/// Static definition for one unique key on a record type.
#[derive(Debug, Clone, Copy)]
pub struct UniqueKeyDefinition {
    /// Stable unique-key id within the enclosing record kind.
    pub id: u8,
    /// Source-level unique-key name. Empty means the single anonymous UK.
    pub name: &'static str,
    /// Optional generated unique-key encoder.
    pub encode: Option<KeyEncodeFn>,
    /// Field names used to build this unique key.
    pub fields: &'static [&'static str],
}

/// Unique-key bytes tagged with the stable per-record UK id.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UniqueKeyBytes {
    /// Stable unique-key id within the enclosing record kind.
    pub uk_id: u8,
    /// Encoded unique-key bytes.
    pub bytes: KeyBytes,
}

impl UniqueKeyBytes {
    /// Creates tagged unique-key bytes.
    #[inline]
    pub fn new(uk_id: u8, bytes: KeyBytes) -> Self {
        Self { uk_id, bytes }
    }
}

/// Static definition for one canonical ordered index on a record type.
#[derive(Debug, Clone, Copy)]
pub struct CanonicalIndexDefinition {
    /// Stable canonical-index id within the enclosing record kind.
    pub id: u8,
    /// Source-level index name.
    pub name: &'static str,
    /// Optional generated index-key encoder for all declared user fields.
    pub encode: Option<KeyEncodeFn>,
    /// Field names used to build this canonical index key.
    pub fields: &'static [&'static str],
}

/// Static definition for one record type.
#[derive(Debug, Clone, Copy)]
pub struct RecordDefinition {
    /// Stable record kind.
    pub kind: RecordKind,
    /// Rust/source-level record name.
    pub name: &'static str,
    /// Record data bytes excluding host-owned metadata.
    pub data_size: u32,
    /// Schema version encoded as a compact `u16`.
    pub version: u16,
    /// All unique keys defined for this record kind.
    pub unique_keys: &'static [UniqueKeyDefinition],
    /// All canonical indexes defined for this record kind.
    pub canonical_indexes: &'static [CanonicalIndexDefinition],
    /// Active schema fields.
    pub fields: &'static [FieldDefinition],
    /// Reserved fields retained for layout compatibility.
    pub reserved_fields: &'static [FieldDefinition],
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

/// Unique-key encoding hook implemented by generated record types.
pub trait UkCodec {
    /// Encodes unique-key bytes from a fixed-layout record buffer.
    fn encode_uk_from_bytes(data: &[u8]) -> KeyBytes;
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
        write!(f, "buffer too small: required {} bytes, got {}", self.required, self.actual)
    }
}

impl std::error::Error for AccessError {}

/// Static command-schema failure. Never a business rejection or execution result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSchemaFailure {
    UnsupportedKind {
        kind: CommandKind,
    },
    FieldAccess {
        field: u32,
        source: AccessError,
    },
    Enum {
        field: u32,
        source: EnumDecodeError,
    },
    Boolean {
        field: u32,
        raw: u8,
    },
    FixedBytesLength {
        field: u32,
        length: usize,
        capacity: usize,
    },
    /// First nonzero byte after the logical length; offset is within the N-byte buffer.
    FixedBytesPadding {
        field: u32,
        offset: usize,
        raw: u8,
    },
    TrailingBytes {
        consumed: usize,
        actual: usize,
    },
}

impl fmt::Display for CommandSchemaFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "command schema: {self:?}")
    }
}

impl std::error::Error for CommandSchemaFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::FieldAccess { source, .. } => Some(source),
            Self::Enum { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Error returned by generated payload builders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PayloadBuildError {
    /// A required builder field was not set.
    MissingField(&'static str),
    /// A `VarBytes` field exceeded the u16 wire length prefix.
    VarBytesTooLong { field: &'static str, len: usize },
    /// A repeated field exceeded its declared maximum element count.
    RepeatedTooLong { field: &'static str, len: usize, max: u16 },
}

impl fmt::Display for PayloadBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingField(field) => write!(f, "payload field {field} not set"),
            Self::VarBytesTooLong { field, len } => {
                write!(f, "payload field {field} VarBytes length {len} exceeds u16::MAX")
            }
            Self::RepeatedTooLong { field, len, max } => {
                write!(f, "payload field {field} repeated length {len} exceeds max {max}")
            }
        }
    }
}

impl std::error::Error for PayloadBuildError {}

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
    /// Canonical definition, also available to generated static field metadata.
    const DEFINITION: &'static EnumDefinition;

    /// Encodes the enum as its `u8` discriminant.
    fn to_u8(self) -> u8;
    /// Decodes the enum from a `u8` discriminant.
    fn try_from_u8(v: u8) -> Result<Self, EnumDecodeError>;
    /// Returns the enum type name.
    fn type_name() -> &'static str {
        Self::DEFINITION.name
    }
    /// Returns the static enum definition.
    fn definition() -> &'static EnumDefinition {
        Self::DEFINITION
    }
}

/// Fixed-capacity byte value with a logical length.
///
/// The logical length must fit in the encoded `u16`, even when `N` is larger.
/// Array conversion rejects unrepresentable lengths at compile time:
///
/// ```compile_fail,E0080
/// use statevec_model::FixedBytes;
/// let bytes = [0u8; 65_536];
/// let _ = FixedBytes::<65_536>::from(&bytes);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixedBytes<const N: usize> {
    len: u16,
    buf: [u8; N],
}

impl<const N: usize> FixedBytes<N> {
    /// Creates a fixed-capacity byte value from a slice.
    pub fn new(bytes: &[u8]) -> Result<Self, AccessError> {
        if bytes.len() > N {
            return Err(AccessError { required: N, actual: bytes.len() });
        }
        let len =
            u16::try_from(bytes.len()).map_err(|_| AccessError { required: u16::MAX as usize, actual: bytes.len() })?;
        let mut buf = [0u8; N];
        buf[..bytes.len()].copy_from_slice(bytes);
        Ok(Self { len, buf })
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

    /// Returns fixed-width bytes suitable for unique-key encoding.
    #[inline]
    pub fn key_bytes(&self) -> KeyBytes {
        let mut uk = KeyBytes::new();
        uk.extend_from_slice(&self.buf);
        uk
    }
}

impl<const N: usize, const M: usize> From<&[u8; M]> for FixedBytes<N> {
    #[inline]
    fn from(bytes: &[u8; M]) -> Self {
        const { assert!(M <= N, "FixedBytes: source length exceeds capacity") };
        const { assert!(M <= u16::MAX as usize, "FixedBytes: source length exceeds encoded u16 length") };
        let mut buf = [0u8; N];
        buf[..M].copy_from_slice(bytes);
        Self { len: M as u16, buf }
    }
}

/// Fixed-point signed decimal value with compile-time scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Decimal<const SCALE: u8> {
    mantissa: i128,
}

/// Explicit rounding policy for decimal scale conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecimalRounding {
    /// Drop fractional digits toward zero.
    Truncate,
    /// Round toward negative infinity.
    Floor,
    /// Round toward positive infinity.
    Ceil,
    /// Round nearest; exact halves round away from zero.
    HalfUp,
    /// Round nearest; exact halves round to an even mantissa.
    HalfEven,
}

/// Error returned by strict decimal string parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecimalParseError {
    /// The decimal scale encoded in the type is unsupported.
    InvalidScale { scale: u8, max: u8 },
    /// The input is empty.
    Empty,
    /// The input is not a canonical fixed-point decimal literal.
    InvalidFormat,
    /// The input has more fractional digits than the target scale.
    TooManyFractionalDigits { scale: u8, actual: usize },
    /// The parsed mantissa does not fit in `i128`.
    Overflow,
}

impl fmt::Display for DecimalParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidScale { scale, max } => write!(f, "decimal scale {scale} exceeds maximum {max}"),
            Self::Empty => f.write_str("decimal literal is empty"),
            Self::InvalidFormat => f.write_str("invalid decimal literal"),
            Self::TooManyFractionalDigits { scale, actual } => {
                write!(f, "decimal literal has {actual} fractional digits, expected at most {scale}")
            }
            Self::Overflow => f.write_str("decimal literal mantissa overflows i128"),
        }
    }
}

impl std::error::Error for DecimalParseError {}

impl<const SCALE: u8> Decimal<SCALE> {
    /// Decimal scale encoded in the type.
    pub const SCALE: u8 = SCALE;

    /// Creates a decimal from the raw fixed-point mantissa.
    #[inline(always)]
    pub const fn from_mantissa(mantissa: i128) -> Self {
        Self { mantissa }
    }

    /// Returns the raw fixed-point mantissa.
    #[inline(always)]
    pub const fn mantissa(self) -> i128 {
        self.mantissa
    }

    /// Adds two same-scale decimals, returning `None` on overflow.
    #[inline(always)]
    pub const fn checked_add(self, rhs: Self) -> Option<Self> {
        match self.mantissa.checked_add(rhs.mantissa) {
            Some(mantissa) => Some(Self { mantissa }),
            None => None,
        }
    }

    /// Subtracts two same-scale decimals, returning `None` on overflow.
    #[inline(always)]
    pub const fn checked_sub(self, rhs: Self) -> Option<Self> {
        match self.mantissa.checked_sub(rhs.mantissa) {
            Some(mantissa) => Some(Self { mantissa }),
            None => None,
        }
    }

    /// Converts this decimal to another scale with an explicit rounding policy.
    pub fn rescale<const OUT: u8>(self, rounding: DecimalRounding) -> Option<Decimal<OUT>> {
        if SCALE > MAX_DECIMAL_SCALE || OUT > MAX_DECIMAL_SCALE {
            return None;
        }
        if OUT == SCALE {
            return Some(Decimal::from_mantissa(self.mantissa));
        }
        if OUT > SCALE {
            let factor = decimal_pow10_i128(OUT - SCALE)?;
            return self.mantissa.checked_mul(factor).map(Decimal::from_mantissa);
        }
        let factor = decimal_pow10_i128(SCALE - OUT)?;
        let quotient = self.mantissa / factor;
        let remainder = self.mantissa % factor;
        decimal_round_quotient(quotient, remainder, factor, rounding).map(Decimal::from_mantissa)
    }
}

impl<const SCALE: u8> std::str::FromStr for Decimal<SCALE> {
    type Err = DecimalParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        if SCALE > MAX_DECIMAL_SCALE {
            return Err(DecimalParseError::InvalidScale { scale: SCALE, max: MAX_DECIMAL_SCALE });
        }
        if input.is_empty() {
            return Err(DecimalParseError::Empty);
        }

        let bytes = input.as_bytes();
        let mut cursor = 0usize;
        let negative = bytes[0] == b'-';
        if negative {
            cursor = 1;
            if cursor == bytes.len() {
                return Err(DecimalParseError::InvalidFormat);
            }
        }

        let integer_start = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if cursor == integer_start {
            return Err(DecimalParseError::InvalidFormat);
        }

        let integer = &input[integer_start..cursor];
        let fractional = if cursor == bytes.len() {
            ""
        } else if bytes[cursor] == b'.' {
            cursor += 1;
            let fractional_start = cursor;
            while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
                cursor += 1;
            }
            if cursor == fractional_start || cursor != bytes.len() {
                return Err(DecimalParseError::InvalidFormat);
            }
            &input[fractional_start..cursor]
        } else {
            return Err(DecimalParseError::InvalidFormat);
        };
        if fractional.len() > SCALE as usize {
            return Err(DecimalParseError::TooManyFractionalDigits { scale: SCALE, actual: fractional.len() });
        }

        let max_magnitude = if negative { (i128::MAX as u128) + 1 } else { i128::MAX as u128 };
        let mut magnitude = 0u128;
        for byte in integer.bytes().chain(fractional.bytes()) {
            magnitude = magnitude
                .checked_mul(10)
                .and_then(|value| value.checked_add(u128::from(byte - b'0')))
                .ok_or(DecimalParseError::Overflow)?;
            if magnitude > max_magnitude {
                return Err(DecimalParseError::Overflow);
            }
        }
        for _ in fractional.len()..SCALE as usize {
            magnitude = magnitude.checked_mul(10).ok_or(DecimalParseError::Overflow)?;
            if magnitude > max_magnitude {
                return Err(DecimalParseError::Overflow);
            }
        }

        let mantissa = if negative {
            if magnitude == max_magnitude { i128::MIN } else { -(magnitude as i128) }
        } else {
            magnitude as i128
        };
        Ok(Self::from_mantissa(mantissa))
    }
}

fn decimal_pow10_i128(power: u8) -> Option<i128> {
    let mut out = 1i128;
    for _ in 0..power {
        out = out.checked_mul(10)?;
    }
    Some(out)
}

fn decimal_round_quotient(quotient: i128, remainder: i128, factor: i128, rounding: DecimalRounding) -> Option<i128> {
    if remainder == 0 {
        return Some(quotient);
    }
    let negative = remainder < 0;
    match rounding {
        DecimalRounding::Truncate => Some(quotient),
        DecimalRounding::Floor => {
            if negative {
                quotient.checked_sub(1)
            } else {
                Some(quotient)
            }
        }
        DecimalRounding::Ceil => {
            if negative {
                Some(quotient)
            } else {
                quotient.checked_add(1)
            }
        }
        DecimalRounding::HalfUp | DecimalRounding::HalfEven => {
            let abs_remainder = remainder.unsigned_abs();
            let factor = factor as u128;
            let twice = abs_remainder.checked_mul(2)?;
            let round_away = if twice > factor {
                true
            } else if twice < factor {
                false
            } else {
                match rounding {
                    DecimalRounding::HalfUp => true,
                    DecimalRounding::HalfEven => quotient % 2 != 0,
                    _ => unreachable!(),
                }
            };
            if !round_away {
                return Some(quotient);
            }
            if negative { quotient.checked_sub(1) } else { quotient.checked_add(1) }
        }
    }
}

impl<const SCALE: u8> std::fmt::Display for Decimal<SCALE> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if SCALE > MAX_DECIMAL_SCALE {
            return Err(std::fmt::Error);
        }
        let negative = self.mantissa < 0;
        let magnitude = if negative { self.mantissa.wrapping_neg() as u128 } else { self.mantissa as u128 };
        if SCALE == 0 {
            if negative { write!(f, "-{magnitude}") } else { write!(f, "{magnitude}") }
        } else {
            let divisor = 10u128.pow(SCALE as u32);
            let whole = magnitude / divisor;
            let fractional = magnitude % divisor;
            if negative {
                write!(f, "-")?;
            }
            write!(f, "{whole}.{fractional:0width$}", width = SCALE as usize)
        }
    }
}

/// Incremental unique-key byte encoder.
pub struct KeyBuilder {
    buf: KeyBytes,
}

impl KeyBuilder {
    /// Creates an empty unique-key builder.
    #[inline]
    pub fn new() -> Self {
        Self { buf: SmallVec::new() }
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

    /// Appends an order-preserving signed `i128` component.
    /// Decimal keys use the same encoding of their fixed-scale mantissa.
    #[inline]
    pub fn push_i128(&mut self, v: i128) {
        self.buf.extend_from_slice(&((v as u128) ^ (1u128 << 127)).to_be_bytes());
    }

    /// Appends a fixed-scale decimal component. The owning schema fixes SCALE;
    /// its identity distinguishes scales, which are not repeated in key bytes.
    #[inline]
    pub fn push_decimal<const SCALE: u8>(&mut self, v: Decimal<SCALE>) {
        self.push_i128(v.mantissa());
    }

    /// Appends raw bytes.
    #[inline]
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Finishes unique-key encoding.
    #[inline]
    pub fn finish(self) -> KeyBytes {
        self.buf
    }
}

impl Default for KeyBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns `Err` if `buf` is shorter than `need` bytes.
#[inline(always)]
fn check_len(buf: &[u8], need: usize) -> Result<(), AccessError> {
    if buf.len() >= need { Ok(()) } else { Err(AccessError { required: need, actual: buf.len() }) }
}

#[inline(always)]
fn checked_need(offset: usize, len: usize, actual: usize) -> Result<usize, AccessError> {
    offset.checked_add(len).ok_or(AccessError { required: usize::MAX, actual })
}

/// Panics if `buf` is shorter than `need` bytes. Used only by write primitives
/// whose callers guarantee the buffer is framework-sized.
#[inline(always)]
fn assert_len(buf: &[u8], need: usize) {
    assert!(buf.len() >= need, "write buffer too small: {} < {}", buf.len(), need);
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
/// Reads a little-endian `i128` from a fixed-layout buffer.
pub fn read_i128_le(buf: &[u8], offset: usize) -> Result<i128, AccessError> {
    Ok(read_u128_le(buf, offset)? as i128)
}

#[inline(always)]
/// Writes a little-endian `i128` into a fixed-layout buffer.
pub fn write_i128_le(buf: &mut [u8], offset: usize, v: i128) {
    write_u128_le(buf, offset, v as u128);
}

#[inline(always)]
/// Reads a fixed-scale decimal from a fixed-layout buffer.
pub fn read_decimal_le<const SCALE: u8>(buf: &[u8], offset: usize) -> Result<Decimal<SCALE>, AccessError> {
    Ok(Decimal::from_mantissa(read_i128_le(buf, offset)?))
}

#[inline(always)]
/// Writes a fixed-scale decimal into a fixed-layout buffer.
pub fn write_decimal_le<const SCALE: u8>(buf: &mut [u8], offset: usize, v: Decimal<SCALE>) {
    write_i128_le(buf, offset, v.mantissa());
}

#[inline(always)]
/// Reads a fixed-capacity byte field from a fixed-layout buffer.
pub fn read_fixed_bytes<const N: usize>(buf: &[u8], offset: usize) -> Result<FixedBytes<N>, AccessError> {
    let start = checked_need(offset, 2, buf.len())?;
    let end = checked_need(start, N, buf.len())?;
    check_len(buf, end)?;
    let len = read_u16_le(buf, offset)? as usize;
    if len > N {
        return Err(AccessError { required: len, actual: N });
    }
    let mut tmp = [0u8; N];
    tmp.copy_from_slice(&buf[start..end]);
    Ok(FixedBytes { len: len as u16, buf: tmp })
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
    command_kind: CommandKind,
    ext_seq: u64,
    ref_ext_time_us: u64,
    payload: Vec<u8>,
}

impl Command {
    /// Creates a command envelope.
    #[inline]
    pub fn new(command_kind: CommandKind, ext_seq: u64, ref_ext_time_us: u64, payload: Vec<u8>) -> Self {
        Self { command_kind, ext_seq, ref_ext_time_us, payload }
    }

    /// Returns the command kind.
    #[inline(always)]
    pub fn command_kind(&self) -> CommandKind {
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
        return Err(AccessError { required: end, actual: data.len() });
    }
    Ok((&data[offset + 2..end], end))
}

#[inline]
/// Appends a payload-layer variable byte field.
pub fn write_var_bytes(buf: &mut Vec<u8>, content: &[u8]) -> Result<(), AccessError> {
    let len =
        u16::try_from(content.len()).map_err(|_| AccessError { required: content.len(), actual: u16::MAX as usize })?;
    buf.extend_from_slice(&len.to_le_bytes());
    buf.extend_from_slice(content);
    Ok(())
}

/// Element type supported by first-cut repeated payload fields.
pub trait RepeatedElement: Copy + 'static {
    /// Encoded element size in bytes.
    const SIZE: usize;
    /// Reads one encoded element from `buf` at `offset`.
    fn read_element(buf: &[u8], offset: usize) -> Result<Self, AccessError>;
    /// Appends one encoded element to `buf`.
    fn write_element(buf: &mut Vec<u8>, value: Self);
}

impl RepeatedElement for bool {
    const SIZE: usize = 1;
    #[inline(always)]
    fn read_element(buf: &[u8], offset: usize) -> Result<Self, AccessError> {
        read_bool(buf, offset)
    }
    #[inline(always)]
    fn write_element(buf: &mut Vec<u8>, value: Self) {
        buf.push(u8::from(value));
    }
}

impl RepeatedElement for u8 {
    const SIZE: usize = 1;
    #[inline(always)]
    fn read_element(buf: &[u8], offset: usize) -> Result<Self, AccessError> {
        read_u8(buf, offset)
    }
    #[inline(always)]
    fn write_element(buf: &mut Vec<u8>, value: Self) {
        buf.push(value);
    }
}

macro_rules! impl_repeated_le {
    ($ty:ty, $size:expr, $read:ident) => {
        impl RepeatedElement for $ty {
            const SIZE: usize = $size;
            #[inline(always)]
            fn read_element(buf: &[u8], offset: usize) -> Result<Self, AccessError> {
                $read(buf, offset)
            }
            #[inline(always)]
            fn write_element(buf: &mut Vec<u8>, value: Self) {
                buf.extend_from_slice(&value.to_le_bytes());
            }
        }
    };
}

impl_repeated_le!(u16, 2, read_u16_le);
impl_repeated_le!(u32, 4, read_u32_le);
impl_repeated_le!(u64, 8, read_u64_le);
impl_repeated_le!(i32, 4, read_i32_le);
impl_repeated_le!(i64, 8, read_i64_le);
impl_repeated_le!(u128, 16, read_u128_le);

impl<const SCALE: u8> RepeatedElement for Decimal<SCALE> {
    const SIZE: usize = 16;
    #[inline(always)]
    fn read_element(buf: &[u8], offset: usize) -> Result<Self, AccessError> {
        read_decimal_le(buf, offset)
    }
    #[inline(always)]
    fn write_element(buf: &mut Vec<u8>, value: Self) {
        buf.extend_from_slice(&value.mantissa().to_le_bytes());
    }
}

impl<const N: usize> RepeatedElement for FixedBytes<N> {
    const SIZE: usize = 2 + N;
    #[inline(always)]
    fn read_element(buf: &[u8], offset: usize) -> Result<Self, AccessError> {
        read_fixed_bytes(buf, offset)
    }
    #[inline(always)]
    fn write_element(buf: &mut Vec<u8>, value: Self) {
        buf.extend_from_slice(&value.len.to_le_bytes());
        buf.extend_from_slice(value.padded_slice());
    }
}

impl<T: EnumU8> RepeatedElement for T {
    const SIZE: usize = 1;
    #[inline(always)]
    fn read_element(buf: &[u8], offset: usize) -> Result<Self, AccessError> {
        let raw = read_u8(buf, offset)?;
        T::try_from_u8(raw).map_err(|_| AccessError { required: 0, actual: raw as usize })
    }
    #[inline(always)]
    fn write_element(buf: &mut Vec<u8>, value: Self) {
        buf.push(value.to_u8());
    }
}

/// Borrowed view over one count-prefixed repeated payload field.
#[derive(Debug, Clone, Copy)]
pub struct RepeatedScalar<'a, T: RepeatedElement> {
    data: &'a [u8],
    len: usize,
    _marker: std::marker::PhantomData<T>,
}

impl<'a, T: RepeatedElement> RepeatedScalar<'a, T> {
    /// Number of encoded elements.
    #[inline(always)]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the repeated field is empty.
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Returns the element at `index`.
    #[inline(always)]
    pub fn get(&self, index: usize) -> T {
        if index >= self.len {
            panic!("repeated payload index {index} out of bounds for len {}", self.len);
        }
        T::read_element(self.data, index * T::SIZE).expect("repeated payload access")
    }

    /// Iterates elements in encoded order.
    #[inline]
    pub fn iter(self) -> impl Iterator<Item = T> + 'a {
        (0..self.len).map(move |idx| self.get(idx))
    }
}

#[inline]
/// Reads a repeated payload field and returns the view plus next offset.
pub fn read_repeated_scalar<T: RepeatedElement>(
    data: &[u8],
    offset: usize,
) -> Result<(RepeatedScalar<'_, T>, usize), AccessError> {
    let count = read_u16_le(data, offset)? as usize;
    let start = checked_need(offset, 2, data.len())?;
    let byte_len = count
        .checked_mul(T::SIZE)
        .ok_or(AccessError { required: usize::MAX, actual: data.len() })?;
    let end = checked_need(start, byte_len, data.len())?;
    check_len(data, end)?;
    Ok((RepeatedScalar { data: &data[start..end], len: count, _marker: std::marker::PhantomData }, end))
}

#[inline]
/// Validates a repeated payload field and returns the next offset.
pub fn validate_repeated_scalar<T: RepeatedElement>(
    data: &[u8],
    offset: usize,
    max: u16,
) -> Result<usize, AccessError> {
    let (view, next) = read_repeated_scalar::<T>(data, offset)?;
    if view.len() > max as usize {
        return Err(AccessError { required: max as usize, actual: view.len() });
    }
    for idx in 0..view.len() {
        T::read_element(view.data, idx * T::SIZE)?;
    }
    Ok(next)
}

#[inline]
/// Appends a count-prefixed repeated payload field.
pub fn write_repeated_scalar<T: RepeatedElement>(
    buf: &mut Vec<u8>,
    values: &[T],
    field: &'static str,
    max: u16,
) -> Result<(), PayloadBuildError> {
    if values.len() > max as usize {
        return Err(PayloadBuildError::RepeatedTooLong { field, len: values.len(), max });
    }
    let len = u16::try_from(values.len()).map_err(|_| PayloadBuildError::RepeatedTooLong {
        field,
        len: values.len(),
        max,
    })?;
    buf.extend_from_slice(&len.to_le_bytes());
    for value in values {
        T::write_element(buf, *value);
    }
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
    /// Decimal scale when `ty` is [`FieldType::Decimal`].
    pub decimal_scale: Option<u8>,
    /// Optional downstream rendering/consumer semantic metadata.
    pub semantic: Option<SemanticTag>,
    /// Whether this payload field is encoded as a count-prefixed repeated list.
    pub repeated: bool,
    /// Maximum element count for repeated payload fields. Must be zero when `repeated` is false.
    pub element_count_max: u16,
    /// Fixed encoded size for fixed-width payload fields.
    pub fixed_size: Option<u32>,
}

/// Static definition for one command type.
#[derive(Debug, Clone, Copy)]
pub struct CommandDefinition {
    /// Stable command kind.
    pub kind: CommandKind,
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
    const KIND: CommandKind;
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
    /// Validate untrusted input first; wrapping does not perform full schema checks.
    fn wrap(data: &[u8]) -> Self::Access<'_>;
    /// Check every field and exact payload consumption without constructing
    /// an unchecked accessor or invoking business code. FixedBytes requires
    /// zero padding after its logical length, without normalizing input bytes.
    fn validate_payload(data: &[u8]) -> Result<(), CommandSchemaFailure>;
    /// Creates a payload builder.
    fn builder() -> Self::Builder;
}

/// Fallible payload builder trait implemented by generated command builders.
pub trait StatevecCommandPayloadBuilder {
    /// Stable command kind built by this payload builder.
    const COMMAND_KIND: CommandKind;
    /// Builds encoded command payload bytes.
    fn build_payload(self) -> Result<Vec<u8>, PayloadBuildError>;
}

// ---- Event types ----

/// Runtime event value emitted by a command transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    event_kind: EventKind,
    event_seq: u32,
    payload: Vec<u8>,
}

impl Event {
    /// Creates an event.
    #[inline]
    pub fn new(event_kind: EventKind, event_seq: u32, payload: Vec<u8>) -> Self {
        Self { event_kind, event_seq, payload }
    }

    /// Returns the event kind.
    #[inline(always)]
    pub fn event_kind(&self) -> EventKind {
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
    event_kind: EventKind,
    payload: Vec<u8>,
}

impl EventFrame {
    /// Creates an event frame.
    #[inline]
    pub fn new(tx_seq: u64, event_seq: u32, event_kind: EventKind, payload: Vec<u8>) -> Self {
        Self { tx_seq, event_seq, event_kind, payload }
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
    pub fn event_kind(&self) -> EventKind {
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
        (self.tx_seq, Event { event_kind: self.event_kind, event_seq: self.event_seq, payload: self.payload })
    }
}

/// Static definition for one event type.
#[derive(Debug, Clone, Copy)]
pub struct EventDefinition {
    /// Stable event kind.
    pub kind: EventKind,
    /// Rust/source-level event name.
    pub name: &'static str,
    /// Schema version encoded as a compact `u16`.
    pub version: u16,
    /// Payload fields.
    pub fields: &'static [PayloadFieldDefinition],
    /// Whether this event is eligible for RPC commit-response projection.
    ///
    /// This is response metadata only and is intentionally excluded from
    /// canonical schema fingerprints and canonical IDL bytes.
    pub inline_response: bool,
}

/// Trait implemented by generated event types.
pub trait EventSchema {
    /// Stable event kind.
    const KIND: EventKind;
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

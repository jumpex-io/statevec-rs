// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use crate::idl::SchemaIdlError;
use crate::model::{
    CanonicalIndexDefinition, CommandDefinition, CommandKind, EnumDefinition, EventDefinition, EventKind,
    FieldDefinition, FieldType, KeyBuilder, KeyBytes, MAX_CANONICAL_INDEX_KEY_BYTES, MAX_CANONICAL_INDEXES_PER_RECORD,
    MAX_UNIQUE_KEYS_PER_RECORD, PayloadFieldDefinition, RecordDefinition, RecordKind, SYSTEM_KIND_MIN, USER_KIND_MAX,
    UniqueKeyBytes, UniqueKeyDefinition, Version, read_bool, read_i32_le, read_i64_le, read_u8, read_u16_le,
    read_u32_le, read_u64_le, validate_decimal_scale_compat, validate_repeated_payload_compat,
    validate_semantic_compat,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Compact stable fingerprint for one schema section.
///
/// Fingerprints are deterministic drift detectors for schema compatibility.
/// They are not cryptographic hashes and should not be used as an
/// adversarial-collision defense.
pub type SchemaFingerprint = [u8; 16];

/// Complete schema identity for compatibility checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaIdentity {
    /// Schema module version.
    pub schema_version: Version,
    /// Fingerprint of named enum/type definitions.
    pub types_schema_fingerprint: SchemaFingerprint,
    /// Fingerprint of record definitions.
    pub record_schema_fingerprint: SchemaFingerprint,
    /// Fingerprint of command definitions.
    pub command_schema_fingerprint: SchemaFingerprint,
    /// Fingerprint of event definitions.
    pub event_schema_fingerprint: SchemaFingerprint,
}

/// Registry of all schema definitions exported by one domain schema module.
#[derive(Debug, Clone)]
pub struct SchemaRegistry {
    schema_version: Version,
    record_defs: BTreeMap<RecordKind, RecordDefinition>,
    command_defs: BTreeMap<CommandKind, CommandDefinition>,
    event_defs: BTreeMap<EventKind, EventDefinition>,
    enum_defs: BTreeMap<&'static str, EnumDefinition>,
    record_schema_fingerprint: SchemaFingerprint,
    command_schema_fingerprint: SchemaFingerprint,
    event_schema_fingerprint: SchemaFingerprint,
    types_schema_fingerprint: SchemaFingerprint,
}

impl SchemaRegistry {
    /// Builds a schema registry from static definitions.
    pub fn new(
        schema_version: Version,
        record_defs: &[RecordDefinition],
        command_defs: &[CommandDefinition],
        event_defs: &[EventDefinition],
        enum_defs: &[EnumDefinition],
    ) -> Self {
        Self::try_new(schema_version, record_defs, command_defs, event_defs, enum_defs)
            .unwrap_or_else(|error| panic!("{error}"))
    }

    // Static definitions keep their programmer-error panic contract; decoded
    // IDL uses these same guards without turning invalid input into an unwind.
    pub(crate) fn try_new(
        schema_version: Version,
        record_defs: &[RecordDefinition],
        command_defs: &[CommandDefinition],
        event_defs: &[EventDefinition],
        enum_defs: &[EnumDefinition],
    ) -> Result<Self, SchemaIdlError> {
        let record_defs = build_record_map(record_defs)?;
        let command_defs = build_command_map(command_defs)?;
        let event_defs = build_event_map(event_defs)?;
        let enum_defs = build_enum_map(enum_defs)?;

        let record_schema_fingerprint = fingerprint_record_defs(record_defs.values().copied());
        let command_schema_fingerprint = fingerprint_command_defs(command_defs.values().copied());
        let event_schema_fingerprint = fingerprint_event_defs(event_defs.values().copied());
        let types_schema_fingerprint = fingerprint_enum_defs(enum_defs.values().copied());

        Ok(Self {
            schema_version,
            record_defs,
            command_defs,
            event_defs,
            enum_defs,
            record_schema_fingerprint,
            command_schema_fingerprint,
            event_schema_fingerprint,
            types_schema_fingerprint,
        })
    }

    /// Builds a schema registry containing only record definitions.
    pub fn with_records(schema_version: Version, record_defs: &[RecordDefinition]) -> Self {
        Self::new(schema_version, record_defs, &[], &[], &[])
    }

    /// Returns the schema module version.
    #[inline(always)]
    pub fn schema_version(&self) -> Version {
        self.schema_version
    }

    /// Returns the record schema fingerprint.
    #[inline(always)]
    pub fn record_schema_fingerprint(&self) -> SchemaFingerprint {
        self.record_schema_fingerprint
    }

    /// Returns the command schema fingerprint.
    #[inline(always)]
    pub fn command_schema_fingerprint(&self) -> SchemaFingerprint {
        self.command_schema_fingerprint
    }

    /// Returns the event schema fingerprint.
    #[inline(always)]
    pub fn event_schema_fingerprint(&self) -> SchemaFingerprint {
        self.event_schema_fingerprint
    }

    /// Returns the enum/type schema fingerprint.
    #[inline(always)]
    pub fn types_schema_fingerprint(&self) -> SchemaFingerprint {
        self.types_schema_fingerprint
    }

    /// Returns the complete schema identity.
    #[inline(always)]
    pub fn identity(&self) -> SchemaIdentity {
        SchemaIdentity {
            schema_version: self.schema_version,
            types_schema_fingerprint: self.types_schema_fingerprint,
            record_schema_fingerprint: self.record_schema_fingerprint,
            command_schema_fingerprint: self.command_schema_fingerprint,
            event_schema_fingerprint: self.event_schema_fingerprint,
        }
    }

    /// Returns a record definition by kind.
    pub fn try_get(&self, kind: RecordKind) -> Option<&RecordDefinition> {
        self.record_defs.get(&kind)
    }

    /// Iterates record definitions in stable kind order.
    #[inline]
    pub fn record_defs(&self) -> impl Iterator<Item = &RecordDefinition> + '_ {
        self.record_defs.values()
    }

    /// Returns a command definition by kind.
    pub fn try_get_command(&self, kind: CommandKind) -> Option<&CommandDefinition> {
        self.command_defs.get(&kind)
    }

    /// Iterates command definitions in stable kind order.
    #[inline]
    pub fn command_defs(&self) -> impl Iterator<Item = &CommandDefinition> + '_ {
        self.command_defs.values()
    }

    /// Returns an event definition by kind.
    pub fn try_get_event(&self, kind: EventKind) -> Option<&EventDefinition> {
        self.event_defs.get(&kind)
    }

    /// Iterates event definitions in stable kind order.
    #[inline]
    pub fn event_defs(&self) -> impl Iterator<Item = &EventDefinition> + '_ {
        self.event_defs.values()
    }

    /// Iterates enum definitions in stable name order.
    #[inline]
    pub fn enum_defs(&self) -> impl Iterator<Item = &EnumDefinition> + '_ {
        self.enum_defs.values()
    }

    /// Encodes a unique key for a record buffer when the record supports it.
    #[inline]
    pub fn encode_uk(&self, kind: RecordKind, data: &[u8]) -> Option<KeyBytes> {
        let def = self.try_get(kind)?;
        encode_unique_key(def, 0, data)
    }

    /// Encodes all unique keys for a record buffer when the record supports them.
    #[inline]
    pub fn encode_unique_keys(&self, kind: RecordKind, data: &[u8]) -> Option<smallvec::SmallVec<[UniqueKeyBytes; 3]>> {
        let def = self.try_get(kind)?;
        if def.unique_keys.is_empty() {
            return None;
        }
        let mut out = smallvec::SmallVec::with_capacity(def.unique_keys.len());
        for uk in normalized_unique_keys(def) {
            out.push(UniqueKeyBytes::new(uk.id, encode_unique_key_def(def, uk, data)?));
        }
        Some(out)
    }

    /// Returns the source-level name for a unique key id.
    #[inline]
    pub fn unique_key_name(&self, kind: RecordKind, uk_id: u8) -> Option<&'static str> {
        let def = self.try_get(kind)?;
        normalized_unique_keys(def).find(|uk| uk.id == uk_id).map(|uk| uk.name)
    }

    /// Returns whether the record kind has enough metadata for unique-key encoding.
    #[inline]
    pub fn supports_uk_encoding(&self, kind: RecordKind) -> bool {
        let Some(def) = self.try_get(kind) else {
            return false;
        };
        if def.unique_keys.is_empty() {
            return false;
        }
        normalized_unique_keys(def).all(|uk| uk.encode.is_some() || can_encode_uk_generic(def, &uk))
    }

    /// Encodes a canonical index key for a record buffer when metadata is available.
    #[inline]
    pub fn encode_canonical_index(&self, kind: RecordKind, index_id: u8, data: &[u8]) -> Option<KeyBytes> {
        let def = self.try_get(kind)?;
        let index = def.canonical_indexes.iter().copied().find(|index| index.id == index_id)?;
        encode_canonical_index_def(def, index, data)
    }
}

fn normalized_unique_keys(def: &RecordDefinition) -> impl Iterator<Item = UniqueKeyDefinition> + '_ {
    def.unique_keys.iter().copied()
}

fn encode_unique_key(def: &RecordDefinition, uk_id: u8, data: &[u8]) -> Option<KeyBytes> {
    let uk = normalized_unique_keys(def).find(|uk| uk.id == uk_id)?;
    encode_unique_key_def(def, uk, data)
}

fn encode_unique_key_def(def: &RecordDefinition, uk: UniqueKeyDefinition, data: &[u8]) -> Option<KeyBytes> {
    if let Some(encode) = uk.encode {
        return Some(encode(data));
    }
    encode_uk_generic(def, uk, data)
}

fn encode_canonical_index_def(
    def: &RecordDefinition,
    index: CanonicalIndexDefinition,
    data: &[u8],
) -> Option<KeyBytes> {
    if let Some(encode) = index.encode {
        return Some(encode(data));
    }
    encode_key_fields_generic(def, index.fields, data)
}

fn encode_uk_generic(def: &RecordDefinition, uk_def: UniqueKeyDefinition, data: &[u8]) -> Option<KeyBytes> {
    if !can_encode_key_fields_generic(def, uk_def.fields) {
        return None;
    }

    encode_key_fields_generic(def, uk_def.fields, data)
}

fn encode_key_fields_generic(def: &RecordDefinition, fields: &[&'static str], data: &[u8]) -> Option<KeyBytes> {
    let mut builder = KeyBuilder::new();
    for field_name in fields {
        let field = def.field_by_name(field_name)?;
        push_uk_field_bytes(&mut builder, field, data)?;
    }
    Some(builder.finish())
}

fn can_encode_uk_generic(def: &RecordDefinition, uk: &UniqueKeyDefinition) -> bool {
    can_encode_key_fields_generic(def, uk.fields)
}

fn can_encode_canonical_index_generic(def: &RecordDefinition, index: &CanonicalIndexDefinition) -> bool {
    can_encode_key_fields_generic(def, index.fields)
        && encoded_key_fields_len(def, index.fields).is_some_and(|len| len <= MAX_CANONICAL_INDEX_KEY_BYTES)
}

fn can_encode_key_fields_generic(def: &RecordDefinition, fields: &[&'static str]) -> bool {
    if fields.is_empty() {
        return false;
    }

    for field_name in fields {
        let Some(field) = def.field_by_name(field_name) else {
            return false;
        };
        match field.ty {
            FieldType::Bool
            | FieldType::U8
            | FieldType::EnumU8
            | FieldType::U16
            | FieldType::U32
            | FieldType::U64
            | FieldType::I32
            | FieldType::I64 => {}
            FieldType::FixedBytes => {
                if field.len < 2 {
                    return false;
                }
            }
            FieldType::U128 | FieldType::VarBytes | FieldType::Decimal => return false,
        }
    }
    true
}

fn encoded_key_fields_len(def: &RecordDefinition, fields: &[&'static str]) -> Option<usize> {
    let mut len = 0usize;
    for field_name in fields {
        let field = def.field_by_name(field_name)?;
        len = len.checked_add(encoded_key_field_len(field)?)?;
    }
    Some(len)
}

fn encoded_key_field_len(field: &FieldDefinition) -> Option<usize> {
    match field.ty {
        FieldType::Bool | FieldType::U8 | FieldType::EnumU8 => Some(1),
        FieldType::U16 => Some(2),
        FieldType::U32 | FieldType::I32 => Some(4),
        FieldType::U64 | FieldType::I64 => Some(8),
        // FixedBytes key encoding uses the padded payload bytes, not the inline u16 length prefix.
        FieldType::FixedBytes => usize::try_from(field.len).ok()?.checked_sub(2),
        FieldType::U128 | FieldType::VarBytes | FieldType::Decimal => None,
    }
}

fn push_uk_field_bytes(builder: &mut KeyBuilder, field: &FieldDefinition, data: &[u8]) -> Option<()> {
    let offset = field.offset as usize;
    match field.ty {
        FieldType::Bool => builder.push_u8(u8::from(read_bool(data, offset).ok()?)),
        FieldType::U8 | FieldType::EnumU8 => builder.push_u8(read_u8(data, offset).ok()?),
        FieldType::U16 => builder.push_u16(read_u16_le(data, offset).ok()?),
        FieldType::U32 => builder.push_u32(read_u32_le(data, offset).ok()?),
        FieldType::U64 => builder.push_u64(read_u64_le(data, offset).ok()?),
        FieldType::I32 => builder.push_i32(read_i32_le(data, offset).ok()?),
        FieldType::I64 => builder.push_i64(read_i64_le(data, offset).ok()?),
        FieldType::FixedBytes => {
            let padded_len = usize::try_from(field.len).ok()?;
            if padded_len < 2 {
                return None;
            }
            let start = offset.checked_add(2)?;
            let end = offset.checked_add(padded_len)?;
            let bytes = data.get(start..end)?;
            builder.push_bytes(bytes);
        }
        FieldType::U128 | FieldType::VarBytes | FieldType::Decimal => return None,
    }
    Some(())
}

fn build_record_map(defs: &[RecordDefinition]) -> Result<BTreeMap<RecordKind, RecordDefinition>, SchemaIdlError> {
    let mut map = BTreeMap::new();
    for def in defs {
        if def.kind == 0 {
            return Err(SchemaIdlError::InvalidSchema(format!(
                "record kind 0 is reserved and cannot be registered: {}",
                def.name
            )));
        }
        if def.kind >= SYSTEM_KIND_MIN {
            return Err(SchemaIdlError::InvalidSchema(format!(
                "record kind {} is reserved for StateVec system metadata and cannot be registered: {}",
                def.kind, def.name
            )));
        }
        if map.insert(def.kind, *def).is_some() {
            return Err(SchemaIdlError::InvalidSchema(format!(
                "duplicate record kind registered: kind={}, name={}",
                def.kind, def.name
            )));
        }
        for field in def.fields {
            validate_decimal_scale_compat(field.ty, field.decimal_scale).map_err(|error| {
                SchemaIdlError::InvalidSchema(format!("record field decimal scale/type mismatch: {error}"))
            })?;
            validate_semantic_compat(field.ty, Some(field.len), field.semantic).map_err(|error| {
                SchemaIdlError::InvalidSchema(format!("record field semantic/type mismatch: {error}"))
            })?;
        }
        for field in def.reserved_fields {
            validate_decimal_scale_compat(field.ty, field.decimal_scale).map_err(|error| {
                SchemaIdlError::InvalidSchema(format!("reserved record field decimal scale/type mismatch: {error}"))
            })?;
            validate_semantic_compat(field.ty, Some(field.len), field.semantic).map_err(|error| {
                SchemaIdlError::InvalidSchema(format!("reserved record field semantic/type mismatch: {error}"))
            })?;
        }
        let unique_key_count = normalized_unique_keys(def).count();
        if unique_key_count > MAX_UNIQUE_KEYS_PER_RECORD {
            return Err(SchemaIdlError::InvalidSchema(format!(
                "record {} defines {} unique keys; max is {}",
                def.name, unique_key_count, MAX_UNIQUE_KEYS_PER_RECORD
            )));
        }
        let mut seen_ids = BTreeMap::new();
        let mut seen_names = BTreeMap::new();
        for uk in normalized_unique_keys(def) {
            if (uk.id as usize) >= MAX_UNIQUE_KEYS_PER_RECORD {
                return Err(SchemaIdlError::InvalidSchema(format!(
                    "record {} unique key id {} is out of range; max exclusive is {}",
                    def.name, uk.id, MAX_UNIQUE_KEYS_PER_RECORD
                )));
            }
            if seen_ids.insert(uk.id, uk.name).is_some() {
                return Err(SchemaIdlError::InvalidSchema(format!(
                    "duplicate unique key id registered: record={}, uk_id={}",
                    def.name, uk.id
                )));
            }
            if !uk.name.is_empty() && seen_names.insert(uk.name, uk.id).is_some() {
                return Err(SchemaIdlError::InvalidSchema(format!(
                    "duplicate unique key name registered: record={}, uk={}",
                    def.name, uk.name
                )));
            }
        }
        for expected in 0..unique_key_count {
            if !seen_ids.contains_key(&(expected as u8)) {
                return Err(SchemaIdlError::InvalidSchema(format!(
                    "record {} unique key ids must be contiguous from 0; missing id {}",
                    def.name, expected
                )));
            }
        }
        if def.canonical_indexes.len() > MAX_CANONICAL_INDEXES_PER_RECORD {
            return Err(SchemaIdlError::InvalidSchema(format!(
                "record {} defines {} canonical indexes; max is {}",
                def.name,
                def.canonical_indexes.len(),
                MAX_CANONICAL_INDEXES_PER_RECORD
            )));
        }
        let mut seen_index_ids = BTreeMap::new();
        let mut seen_index_names = BTreeMap::new();
        for index in def.canonical_indexes {
            if (index.id as usize) >= MAX_CANONICAL_INDEXES_PER_RECORD {
                return Err(SchemaIdlError::InvalidSchema(format!(
                    "record {} canonical index id {} is out of range; max exclusive is {}",
                    def.name, index.id, MAX_CANONICAL_INDEXES_PER_RECORD
                )));
            }
            if seen_index_ids.insert(index.id, index.name).is_some() {
                return Err(SchemaIdlError::InvalidSchema(format!(
                    "duplicate canonical index id registered: record={}, index_id={}",
                    def.name, index.id
                )));
            }
            if index.name.is_empty() {
                return Err(SchemaIdlError::InvalidSchema(format!(
                    "record {} canonical index name cannot be empty",
                    def.name
                )));
            }
            if seen_index_names.insert(index.name, index.id).is_some() {
                return Err(SchemaIdlError::InvalidSchema(format!(
                    "duplicate canonical index name registered: record={}, index={}",
                    def.name, index.name
                )));
            }
            if !can_encode_canonical_index_generic(def, index) {
                return Err(SchemaIdlError::InvalidSchema(format!(
                    "record {} canonical index {} has unsupported or oversized key fields",
                    def.name, index.name
                )));
            }
        }
        for expected in 0..def.canonical_indexes.len() {
            if !seen_index_ids.contains_key(&(expected as u8)) {
                return Err(SchemaIdlError::InvalidSchema(format!(
                    "record {} canonical index ids must be contiguous from 0; missing id {}",
                    def.name, expected
                )));
            }
        }
    }
    Ok(map)
}

fn build_command_map(defs: &[CommandDefinition]) -> Result<BTreeMap<CommandKind, CommandDefinition>, SchemaIdlError> {
    let mut map = BTreeMap::new();
    for def in defs {
        if def.kind == 0 {
            return Err(SchemaIdlError::InvalidSchema(format!(
                "command kind 0 is reserved and cannot be registered: {}",
                def.name
            )));
        }
        if def.kind > USER_KIND_MAX {
            return Err(SchemaIdlError::InvalidSchema(format!(
                "command kind {} is reserved for StateVec system commands and cannot be registered: {}",
                def.kind, def.name
            )));
        }
        if map.insert(def.kind, *def).is_some() {
            return Err(SchemaIdlError::InvalidSchema(format!(
                "duplicate command kind registered: kind={}, name={}",
                def.kind, def.name
            )));
        }
        for field in def.fields {
            validate_decimal_scale_compat(field.ty, field.decimal_scale).map_err(|error| {
                SchemaIdlError::InvalidSchema(format!("command field decimal scale/type mismatch: {error}"))
            })?;
            validate_semantic_compat(field.ty, field.fixed_size, field.semantic).map_err(|error| {
                SchemaIdlError::InvalidSchema(format!("command field semantic/type mismatch: {error}"))
            })?;
            validate_repeated_payload_compat(field.ty, field.repeated, field.element_count_max, true).map_err(
                |error| SchemaIdlError::InvalidSchema(format!("command field repeated metadata mismatch: {error}")),
            )?;
        }
    }
    Ok(map)
}

fn build_event_map(defs: &[EventDefinition]) -> Result<BTreeMap<EventKind, EventDefinition>, SchemaIdlError> {
    let mut map = BTreeMap::new();
    for def in defs {
        if def.kind == 0 {
            return Err(SchemaIdlError::InvalidSchema(format!(
                "event kind 0 is reserved and cannot be registered: {}",
                def.name
            )));
        }
        if def.kind > USER_KIND_MAX {
            return Err(SchemaIdlError::InvalidSchema(format!(
                "event kind {} is reserved for StateVec system events and cannot be registered: {}",
                def.kind, def.name
            )));
        }
        if map.insert(def.kind, *def).is_some() {
            return Err(SchemaIdlError::InvalidSchema(format!(
                "duplicate event kind registered: kind={}, name={}",
                def.kind, def.name
            )));
        }
        for field in def.fields {
            validate_decimal_scale_compat(field.ty, field.decimal_scale).map_err(|error| {
                SchemaIdlError::InvalidSchema(format!("event field decimal scale/type mismatch: {error}"))
            })?;
            validate_semantic_compat(field.ty, field.fixed_size, field.semantic).map_err(|error| {
                SchemaIdlError::InvalidSchema(format!("event field semantic/type mismatch: {error}"))
            })?;
            validate_repeated_payload_compat(field.ty, field.repeated, field.element_count_max, false).map_err(
                |error| SchemaIdlError::InvalidSchema(format!("event field repeated metadata mismatch: {error}")),
            )?;
        }
    }
    Ok(map)
}

fn build_enum_map(defs: &[EnumDefinition]) -> Result<BTreeMap<&'static str, EnumDefinition>, SchemaIdlError> {
    let mut map = BTreeMap::new();
    for def in defs {
        if map.insert(def.name, *def).is_some() {
            return Err(SchemaIdlError::InvalidSchema(format!(
                "duplicate enum definition registered: name={}",
                def.name
            )));
        }
    }
    Ok(map)
}

#[derive(Clone, Copy)]
struct Fingerprinter {
    lo: u64,
    hi: u64,
}

impl Fingerprinter {
    const LO_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const HI_OFFSET: u64 = 0x8422_2325_cbf2_9ce4;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    fn new() -> Self {
        Self { lo: Self::LO_OFFSET, hi: Self::HI_OFFSET }
    }

    fn write_u8(&mut self, value: u8) {
        self.lo ^= value as u64;
        self.lo = self.lo.wrapping_mul(Self::PRIME);
        self.hi ^= (value as u64).wrapping_add(0x9e37_79b9);
        self.hi = self.hi.wrapping_mul(Self::PRIME);
    }

    fn write_bool(&mut self, value: bool) {
        self.write_u8(u8::from(value));
    }

    fn write_u16(&mut self, value: u16) {
        self.write_bytes(&value.to_le_bytes());
    }

    fn write_u32(&mut self, value: u32) {
        self.write_bytes(&value.to_le_bytes());
    }

    fn write_str(&mut self, value: &str) {
        self.write_u32(value.len() as u32);
        self.write_bytes(value.as_bytes());
    }

    fn write_opt_str(&mut self, value: Option<&str>) {
        match value {
            Some(value) => {
                self.write_u8(1);
                self.write_str(value);
            }
            None => self.write_u8(0),
        }
    }

    fn write_opt_u32(&mut self, value: Option<u32>) {
        match value {
            Some(value) => {
                self.write_u8(1);
                self.write_u32(value);
            }
            None => self.write_u8(0),
        }
    }

    fn write_opt_u8(&mut self, value: Option<u8>) {
        match value {
            Some(value) => {
                self.write_u8(1);
                self.write_u8(value);
            }
            None => self.write_u8(0),
        }
    }

    fn write_bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.write_u8(byte);
        }
    }

    fn finish(self) -> SchemaFingerprint {
        let mut out = [0u8; 16];
        out[..8].copy_from_slice(&self.lo.to_le_bytes());
        out[8..].copy_from_slice(&self.hi.to_le_bytes());
        out
    }
}

fn fingerprint_record_defs(defs: impl IntoIterator<Item = RecordDefinition>) -> SchemaFingerprint {
    let mut fp = Fingerprinter::new();
    for def in defs {
        fp.write_u16(def.kind);
        fp.write_str(def.name);
        fp.write_u32(def.data_size);
        fp.write_u16(def.version);
        let mut unique_keys: Vec<_> = normalized_unique_keys(&def).collect();
        unique_keys.sort_by_key(|uk| uk.id);
        fp.write_u32(unique_keys.len() as u32);
        for uk in unique_keys {
            fp.write_u8(uk.id);
            fp.write_str(uk.name);
            fp.write_u32(uk.fields.len() as u32);
            for uk_field in uk.fields {
                fp.write_str(uk_field);
            }
        }
        fp.write_u32(def.canonical_indexes.len() as u32);
        let mut canonical_indexes: Vec<_> = def.canonical_indexes.iter().copied().collect();
        canonical_indexes.sort_by_key(|index| index.id);
        for index in canonical_indexes {
            fp.write_u8(index.id);
            fp.write_str(index.name);
            fp.write_u32(index.fields.len() as u32);
            for field in index.fields {
                fp.write_str(field);
            }
        }
        fp.write_u32(def.fields.len() as u32);
        for field in def.fields {
            write_record_field(&mut fp, field);
        }
        fp.write_u32(def.reserved_fields.len() as u32);
        for field in def.reserved_fields {
            write_record_field(&mut fp, field);
        }
    }
    fp.finish()
}

fn fingerprint_command_defs(defs: impl IntoIterator<Item = CommandDefinition>) -> SchemaFingerprint {
    let mut fp = Fingerprinter::new();
    for def in defs {
        fp.write_u16(def.kind);
        fp.write_str(def.name);
        fp.write_u16(def.version);
        fp.write_u32(def.fields.len() as u32);
        for field in def.fields {
            write_payload_field(&mut fp, field);
        }
    }
    fp.finish()
}

fn fingerprint_event_defs(defs: impl IntoIterator<Item = EventDefinition>) -> SchemaFingerprint {
    let mut fp = Fingerprinter::new();
    for def in defs {
        fp.write_u16(def.kind);
        fp.write_str(def.name);
        fp.write_u16(def.version);
        fp.write_u32(def.fields.len() as u32);
        for field in def.fields {
            write_payload_field(&mut fp, field);
        }
    }
    fp.finish()
}

fn fingerprint_enum_defs(defs: impl IntoIterator<Item = EnumDefinition>) -> SchemaFingerprint {
    let mut fp = Fingerprinter::new();
    for def in defs {
        fp.write_str(def.name);
        fp.write_u32(def.variants.len() as u32);
        let mut variants: Vec<_> = def.variants.iter().collect();
        variants.sort_by_key(|variant| variant.discriminant);
        for variant in variants {
            fp.write_str(variant.name);
            fp.write_u8(variant.discriminant);
        }
    }
    fp.finish()
}

fn write_record_field(fp: &mut Fingerprinter, field: &FieldDefinition) {
    fp.write_str(field.name);
    fp.write_u32(field.field_index);
    fp.write_u32(field.offset);
    fp.write_u8(field_type_tag(field.ty));
    fp.write_u32(field.len);
    fp.write_opt_str(field.enum_type_name);
    fp.write_opt_u8(field.decimal_scale);
    fp.write_bool(field.immutable);
}

fn write_payload_field(fp: &mut Fingerprinter, field: &PayloadFieldDefinition) {
    fp.write_str(field.name);
    fp.write_u32(field.field_index);
    fp.write_u8(field_type_tag(field.ty));
    fp.write_opt_str(field.enum_type_name);
    fp.write_opt_u8(field.decimal_scale);
    fp.write_bool(field.repeated);
    fp.write_u16(field.element_count_max);
    fp.write_opt_u32(field.fixed_size);
}

pub(crate) fn field_type_tag(ty: FieldType) -> u8 {
    match ty {
        FieldType::Bool => 1,
        FieldType::U8 => 2,
        FieldType::U16 => 3,
        FieldType::U32 => 4,
        FieldType::U64 => 5,
        FieldType::I32 => 6,
        FieldType::I64 => 7,
        FieldType::U128 => 8,
        FieldType::FixedBytes => 9,
        FieldType::VarBytes => 10,
        FieldType::EnumU8 => 11,
        FieldType::Decimal => 12,
    }
}

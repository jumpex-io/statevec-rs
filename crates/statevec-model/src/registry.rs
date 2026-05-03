// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use crate::model::{
    CommandDefinition, EnumDefinition, EventDefinition, FieldDefinition, FieldType,
    PayloadFieldDefinition, PkBuilder, PkBytes, RecordDefinition, RecordKind, Version, read_bool,
    read_i32_le, read_i64_le, read_u8, read_u16_le, read_u32_le, read_u64_le,
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
    command_defs: BTreeMap<u8, CommandDefinition>,
    event_defs: BTreeMap<u8, EventDefinition>,
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
        let record_defs = build_record_map(record_defs);
        let command_defs = build_command_map(command_defs);
        let event_defs = build_event_map(event_defs);
        let enum_defs = build_enum_map(enum_defs);

        let record_schema_fingerprint = fingerprint_record_defs(record_defs.values().copied());
        let command_schema_fingerprint = fingerprint_command_defs(command_defs.values().copied());
        let event_schema_fingerprint = fingerprint_event_defs(event_defs.values().copied());
        let types_schema_fingerprint = fingerprint_enum_defs(enum_defs.values().copied());

        Self {
            schema_version,
            record_defs,
            command_defs,
            event_defs,
            enum_defs,
            record_schema_fingerprint,
            command_schema_fingerprint,
            event_schema_fingerprint,
            types_schema_fingerprint,
        }
    }

    /// Builds a schema registry containing only record definitions.
    pub fn with_records(schema_version: Version, record_defs: &[RecordDefinition]) -> Self {
        Self::new(schema_version, record_defs, &[], &[], &[])
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_with_fingerprints(
        schema_version: Version,
        record_defs: &[RecordDefinition],
        command_defs: &[CommandDefinition],
        event_defs: &[EventDefinition],
        enum_defs: &[EnumDefinition],
        record_schema_fingerprint: SchemaFingerprint,
        command_schema_fingerprint: SchemaFingerprint,
        event_schema_fingerprint: SchemaFingerprint,
        types_schema_fingerprint: SchemaFingerprint,
    ) -> Self {
        let mut registry = Self::new(
            schema_version,
            record_defs,
            command_defs,
            event_defs,
            enum_defs,
        );
        registry.record_schema_fingerprint = record_schema_fingerprint;
        registry.command_schema_fingerprint = command_schema_fingerprint;
        registry.event_schema_fingerprint = event_schema_fingerprint;
        registry.types_schema_fingerprint = types_schema_fingerprint;
        registry
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
    pub fn try_get_command(&self, kind: u8) -> Option<&CommandDefinition> {
        self.command_defs.get(&kind)
    }

    /// Iterates command definitions in stable kind order.
    #[inline]
    pub fn command_defs(&self) -> impl Iterator<Item = &CommandDefinition> + '_ {
        self.command_defs.values()
    }

    /// Returns an event definition by kind.
    pub fn try_get_event(&self, kind: u8) -> Option<&EventDefinition> {
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

    /// Encodes a primary key for a record buffer when the record supports it.
    #[inline]
    pub fn encode_pk(&self, kind: RecordKind, data: &[u8]) -> Option<PkBytes> {
        let def = self.try_get(kind)?;
        if let Some(encode) = def.pk_encode {
            return Some(encode(data));
        }
        encode_pk_generic(def, data)
    }

    /// Returns whether the record kind has enough metadata for primary-key encoding.
    #[inline]
    pub fn supports_pk_encoding(&self, kind: RecordKind) -> bool {
        let Some(def) = self.try_get(kind) else {
            return false;
        };
        def.pk_encode.is_some() || can_encode_pk_generic(def)
    }
}

fn encode_pk_generic(def: &RecordDefinition, data: &[u8]) -> Option<PkBytes> {
    if !can_encode_pk_generic(def) {
        return None;
    }

    let mut pk = PkBuilder::new();
    for field_name in def.pk_fields {
        let field = def.field_by_name(field_name)?;
        push_pk_field_bytes(&mut pk, field, data)?;
    }
    Some(pk.finish())
}

fn can_encode_pk_generic(def: &RecordDefinition) -> bool {
    if !def.is_pk_idx || def.pk_fields.is_empty() {
        return false;
    }

    for field_name in def.pk_fields {
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
            FieldType::U128 | FieldType::VarBytes => return false,
        }
    }
    true
}

fn push_pk_field_bytes(
    builder: &mut PkBuilder,
    field: &FieldDefinition,
    data: &[u8],
) -> Option<()> {
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
        FieldType::U128 | FieldType::VarBytes => return None,
    }
    Some(())
}

fn build_record_map(defs: &[RecordDefinition]) -> BTreeMap<RecordKind, RecordDefinition> {
    let mut map = BTreeMap::new();
    for def in defs {
        assert_ne!(
            def.kind, 0,
            "record kind 0 is reserved and cannot be registered: {}",
            def.name
        );
        assert!(
            map.insert(def.kind, *def).is_none(),
            "duplicate record kind registered: kind={}, name={}",
            def.kind,
            def.name
        );
    }
    map
}

fn build_command_map(defs: &[CommandDefinition]) -> BTreeMap<u8, CommandDefinition> {
    let mut map = BTreeMap::new();
    for def in defs {
        assert!(
            map.insert(def.kind, *def).is_none(),
            "duplicate command kind registered: kind={}, name={}",
            def.kind,
            def.name
        );
    }
    map
}

fn build_event_map(defs: &[EventDefinition]) -> BTreeMap<u8, EventDefinition> {
    let mut map = BTreeMap::new();
    for def in defs {
        assert!(
            map.insert(def.kind, *def).is_none(),
            "duplicate event kind registered: kind={}, name={}",
            def.kind,
            def.name
        );
    }
    map
}

fn build_enum_map(defs: &[EnumDefinition]) -> BTreeMap<&'static str, EnumDefinition> {
    let mut map = BTreeMap::new();
    for def in defs {
        assert!(
            map.insert(def.name, *def).is_none(),
            "duplicate enum definition registered: name={}",
            def.name
        );
    }
    map
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
        Self {
            lo: Self::LO_OFFSET,
            hi: Self::HI_OFFSET,
        }
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
        fp.write_u8(def.kind);
        fp.write_str(def.name);
        fp.write_bool(def.is_pk_idx);
        fp.write_bool(def.support_range_scan);
        fp.write_u32(def.data_size);
        fp.write_u16(def.version);
        fp.write_bool(def.pk_encode.is_some());
        fp.write_u32(def.fields.len() as u32);
        for field in def.fields {
            write_record_field(&mut fp, field);
        }
        fp.write_u32(def.reserved_fields.len() as u32);
        for field in def.reserved_fields {
            write_record_field(&mut fp, field);
        }
        fp.write_u32(def.pk_fields.len() as u32);
        for pk_field in def.pk_fields {
            fp.write_str(pk_field);
        }
    }
    fp.finish()
}

fn fingerprint_command_defs(
    defs: impl IntoIterator<Item = CommandDefinition>,
) -> SchemaFingerprint {
    let mut fp = Fingerprinter::new();
    for def in defs {
        fp.write_u8(def.kind);
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
        fp.write_u8(def.kind);
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
        for variant in def.variants {
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
    fp.write_str(field.rust_type_name);
    fp.write_opt_str(field.enum_type_name);
    fp.write_bool(field.immutable);
}

fn write_payload_field(fp: &mut Fingerprinter, field: &PayloadFieldDefinition) {
    fp.write_str(field.name);
    fp.write_u32(field.field_index);
    fp.write_u8(field_type_tag(field.ty));
    fp.write_str(field.rust_type_name);
    fp.write_opt_str(field.enum_type_name);
    fp.write_opt_u32(field.fixed_size);
}

fn field_type_tag(ty: FieldType) -> u8 {
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
    }
}

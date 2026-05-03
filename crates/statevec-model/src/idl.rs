// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use crate::model::FieldType;
use crate::registry::{SchemaFingerprint, SchemaRegistry};
use crate::{
    CommandDefinition, EnumDefinition, EnumVariantDefinition, EventDefinition, FieldDefinition,
    PayloadFieldDefinition, RecordDefinition, Version,
};
use serde::Deserialize;
use std::fmt;

struct JsonWriter {
    buf: String,
    indent: usize,
}

impl JsonWriter {
    fn new() -> Self {
        Self {
            buf: String::with_capacity(4096),
            indent: 0,
        }
    }

    fn indent_str(&self) -> String {
        "  ".repeat(self.indent)
    }

    fn begin_object(&mut self) {
        self.buf.push('{');
        self.indent += 1;
    }

    fn end_object(&mut self) {
        self.indent -= 1;
        self.buf.push('\n');
        self.buf.push_str(&self.indent_str());
        self.buf.push('}');
    }

    fn begin_array(&mut self) {
        self.buf.push('[');
        self.indent += 1;
    }

    fn end_array(&mut self) {
        self.indent -= 1;
        self.buf.push('\n');
        self.buf.push_str(&self.indent_str());
        self.buf.push(']');
    }

    fn key(&mut self, name: &str, first: bool) {
        if !first {
            self.buf.push(',');
        }
        self.buf.push('\n');
        self.buf.push_str(&self.indent_str());
        self.buf.push('"');
        self.buf.push_str(name);
        self.buf.push_str("\": ");
    }

    fn array_sep(&mut self, first: bool) {
        if !first {
            self.buf.push(',');
        }
        self.buf.push('\n');
        self.buf.push_str(&self.indent_str());
    }

    fn write_str(&mut self, s: &str) {
        self.buf.push('"');
        for ch in s.chars() {
            match ch {
                '"' => self.buf.push_str("\\\""),
                '\\' => self.buf.push_str("\\\\"),
                '\n' => self.buf.push_str("\\n"),
                '\r' => self.buf.push_str("\\r"),
                '\t' => self.buf.push_str("\\t"),
                c if c.is_control() => {
                    use std::fmt::Write;
                    let _ = write!(self.buf, "\\u{:04x}", c as u32);
                }
                c => self.buf.push(c),
            }
        }
        self.buf.push('"');
    }

    fn write_u32(&mut self, v: u32) {
        self.buf.push_str(&v.to_string());
    }

    fn write_bool(&mut self, v: bool) {
        self.buf.push_str(if v { "true" } else { "false" });
    }

    fn finish(self) -> String {
        self.buf
    }
}

fn hex16(bytes: &[u8; 16]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(32);
    for b in bytes {
        let _ = write!(s, "{:02x}", b);
    }
    s
}

fn field_type_json(
    w: &mut JsonWriter,
    ty: FieldType,
    rust_type_name: &str,
    enum_type_name: Option<&str>,
    fixed_size: Option<u32>,
) {
    match ty {
        FieldType::Bool => w.write_str("bool"),
        FieldType::U8 => w.write_str("u8"),
        FieldType::U16 => w.write_str("u16"),
        FieldType::U32 => w.write_str("u32"),
        FieldType::U64 => w.write_str("u64"),
        FieldType::I32 => w.write_str("i32"),
        FieldType::I64 => w.write_str("i64"),
        FieldType::U128 => w.write_str("u128"),
        FieldType::FixedBytes => {
            if let Some(n) = fixed_size {
                // For records: fixed_size is the total allocation (2 + N), so capacity = N - 2
                // For payloads: fixed_size is already 2 + N
                // We parse from rust_type_name to get N directly
                let capacity =
                    parse_fixed_bytes_capacity(rust_type_name).unwrap_or(n.saturating_sub(2));
                w.buf.push_str(&format!("{{\"fixedBytes\": {}}}", capacity));
            } else {
                // Fallback: parse from rust_type_name
                if let Some(n) = parse_fixed_bytes_capacity(rust_type_name) {
                    w.buf.push_str(&format!("{{\"fixedBytes\": {}}}", n));
                } else {
                    w.write_str("fixedBytes");
                }
            }
        }
        FieldType::VarBytes => w.write_str("varBytes"),
        FieldType::EnumU8 => {
            if let Some(name) = enum_type_name {
                w.buf.push_str(&format!("{{\"defined\": \"{}\"}}", name));
            } else {
                w.write_str("enumU8");
            }
        }
    }
}

#[doc(hidden)]
pub(crate) fn parse_fixed_bytes_capacity(rust_type_name: &str) -> Option<u32> {
    // rust_type_name looks like "FixedBytes<16>" or "FixedBytes < 16 >"
    let s = rust_type_name.trim();
    let after = s.strip_prefix("FixedBytes")?;
    let inner = after.trim().strip_prefix('<')?.strip_suffix('>')?.trim();
    inner.parse::<u32>().ok()
}

/// Error returned when schema IDL JSON cannot be decoded or validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaIdlError {
    /// Input bytes were not valid UTF-8.
    InvalidUtf8,
    /// JSON parsing failed.
    Json(String),
    /// The IDL document version is not supported by this crate.
    UnsupportedIdlVersion(String),
    /// The IDL document is syntactically valid but semantically invalid.
    InvalidSchema(String),
}

impl fmt::Display for SchemaIdlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUtf8 => f.write_str("schema IDL bytes are not valid UTF-8"),
            Self::Json(message) => write!(f, "failed to parse schema IDL JSON: {message}"),
            Self::UnsupportedIdlVersion(version) => {
                write!(f, "unsupported schema IDL version: {version}")
            }
            Self::InvalidSchema(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for SchemaIdlError {}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdlDocument {
    idl_version: String,
    schema_version: IdlVersion,
    fingerprints: IdlFingerprints,
    #[serde(default)]
    types: Vec<IdlEnumDefinition>,
    #[serde(default)]
    records: Vec<IdlRecordDefinition>,
    #[serde(default)]
    commands: Vec<IdlPayloadDefinition>,
    #[serde(default)]
    events: Vec<IdlPayloadDefinition>,
}

#[derive(Debug, Deserialize)]
struct IdlVersion {
    main: u8,
    minor: u8,
}

#[derive(Debug, Deserialize)]
struct IdlFingerprints {
    records: String,
    commands: String,
    events: String,
    types: String,
}

#[derive(Debug, Deserialize)]
struct IdlEnumDefinition {
    name: String,
    kind: String,
    variants: Vec<IdlEnumVariantDefinition>,
}

#[derive(Debug, Deserialize)]
struct IdlEnumVariantDefinition {
    name: String,
    value: u8,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdlRecordDefinition {
    kind: u8,
    name: String,
    version: u16,
    data_size: u32,
    has_pk: bool,
    #[serde(default)]
    pk_fields: Vec<String>,
    support_range_scan: bool,
    #[serde(default)]
    fields: Vec<IdlRecordFieldDefinition>,
}

#[derive(Debug, Deserialize)]
struct IdlRecordFieldDefinition {
    name: String,
    index: u32,
    #[serde(rename = "type")]
    ty: IdlFieldType,
    offset: u32,
    size: u32,
    immutable: bool,
}

#[derive(Debug, Deserialize)]
struct IdlPayloadDefinition {
    kind: u8,
    name: String,
    version: u16,
    #[serde(default)]
    fields: Vec<IdlPayloadFieldDefinition>,
}

#[derive(Debug, Deserialize)]
struct IdlPayloadFieldDefinition {
    name: String,
    index: u32,
    #[serde(rename = "type")]
    ty: IdlFieldType,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum IdlFieldType {
    Primitive(String),
    FixedBytes {
        #[serde(rename = "fixedBytes")]
        fixed_bytes: u32,
    },
    Defined {
        defined: String,
    },
}

struct DecodedFieldType {
    ty: FieldType,
    rust_type_name: &'static str,
    enum_type_name: Option<&'static str>,
    fixed_size: Option<u32>,
}

impl SchemaRegistry {
    /// Parses a schema registry from UTF-8 IDL JSON bytes.
    ///
    /// Parsed names are promoted to `'static` storage so the returned registry
    /// has the same lifetime shape as macro-generated registries. Repeatedly
    /// parsing unbounded IDL documents leaks those promoted strings; do not use
    /// this on a request path or hot reload loop.
    pub fn from_idl_bytes(bytes: &[u8]) -> Result<Self, SchemaIdlError> {
        let json = std::str::from_utf8(bytes).map_err(|_| SchemaIdlError::InvalidUtf8)?;
        Self::from_idl_json(json)
    }

    /// Parses a schema registry from IDL JSON text.
    ///
    /// See [`SchemaRegistry::from_idl_bytes`] for the lifetime and leak
    /// behavior of parsed names.
    pub fn from_idl_json(json: &str) -> Result<Self, SchemaIdlError> {
        let doc: IdlDocument =
            serde_json::from_str(json).map_err(|err| SchemaIdlError::Json(err.to_string()))?;
        if doc.idl_version != "1.0" {
            return Err(SchemaIdlError::UnsupportedIdlVersion(doc.idl_version));
        }

        let enum_defs: Vec<EnumDefinition> = doc
            .types
            .into_iter()
            .map(|def| {
                if def.kind != "enumU8" {
                    return Err(SchemaIdlError::InvalidSchema(format!(
                        "unsupported type definition kind for {}: {}",
                        def.name, def.kind
                    )));
                }
                let name = leak_string(def.name);
                let variants = def
                    .variants
                    .into_iter()
                    .map(|variant| EnumVariantDefinition {
                        name: leak_string(variant.name),
                        discriminant: variant.value,
                    })
                    .collect::<Vec<_>>();
                Ok(EnumDefinition {
                    name,
                    variants: leak_slice(variants),
                })
            })
            .collect::<Result<_, _>>()?;

        let record_defs: Vec<RecordDefinition> = doc
            .records
            .into_iter()
            .map(|def| {
                let pk_fields = def
                    .pk_fields
                    .into_iter()
                    .map(leak_string)
                    .collect::<Vec<_>>();
                let is_pk_idx = def.has_pk || !pk_fields.is_empty();
                let fields = def
                    .fields
                    .into_iter()
                    .map(|field| {
                        let decoded = decode_field_type(field.ty)?;
                        Ok(FieldDefinition {
                            name: leak_string(field.name),
                            field_index: field.index,
                            offset: field.offset,
                            ty: decoded.ty,
                            len: field.size,
                            rust_type_name: decoded.rust_type_name,
                            enum_type_name: decoded.enum_type_name,
                            immutable: field.immutable,
                        })
                    })
                    .collect::<Result<Vec<_>, SchemaIdlError>>()?;

                Ok(RecordDefinition {
                    kind: def.kind,
                    name: leak_string(def.name),
                    is_pk_idx,
                    support_range_scan: def.support_range_scan,
                    data_size: def.data_size,
                    version: def.version,
                    pk_encode: None,
                    fields: leak_slice(fields),
                    reserved_fields: &[],
                    pk_fields: leak_slice(pk_fields),
                })
            })
            .collect::<Result<_, _>>()?;

        let command_defs: Vec<CommandDefinition> = doc
            .commands
            .into_iter()
            .map(idl_payload_to_command_definition)
            .collect::<Result<_, _>>()?;

        let event_defs: Vec<EventDefinition> = doc
            .events
            .into_iter()
            .map(idl_payload_to_event_definition)
            .collect::<Result<_, _>>()?;

        Ok(SchemaRegistry::new_with_fingerprints(
            Version::new(doc.schema_version.main, doc.schema_version.minor),
            &record_defs,
            &command_defs,
            &event_defs,
            &enum_defs,
            parse_fingerprint("records", &doc.fingerprints.records)?,
            parse_fingerprint("commands", &doc.fingerprints.commands)?,
            parse_fingerprint("events", &doc.fingerprints.events)?,
            parse_fingerprint("types", &doc.fingerprints.types)?,
        ))
    }
}

fn idl_payload_to_command_definition(
    def: IdlPayloadDefinition,
) -> Result<CommandDefinition, SchemaIdlError> {
    Ok(CommandDefinition {
        kind: def.kind,
        name: leak_string(def.name),
        version: def.version,
        fields: leak_slice(idl_payload_fields(def.fields)?),
    })
}

fn idl_payload_to_event_definition(
    def: IdlPayloadDefinition,
) -> Result<EventDefinition, SchemaIdlError> {
    Ok(EventDefinition {
        kind: def.kind,
        name: leak_string(def.name),
        version: def.version,
        fields: leak_slice(idl_payload_fields(def.fields)?),
    })
}

fn idl_payload_fields(
    fields: Vec<IdlPayloadFieldDefinition>,
) -> Result<Vec<PayloadFieldDefinition>, SchemaIdlError> {
    fields
        .into_iter()
        .map(|field| {
            let decoded = decode_field_type(field.ty)?;
            Ok(PayloadFieldDefinition {
                name: leak_string(field.name),
                field_index: field.index,
                ty: decoded.ty,
                rust_type_name: decoded.rust_type_name,
                enum_type_name: decoded.enum_type_name,
                fixed_size: decoded.fixed_size,
            })
        })
        .collect()
}

fn decode_field_type(field_type: IdlFieldType) -> Result<DecodedFieldType, SchemaIdlError> {
    match field_type {
        IdlFieldType::Primitive(kind) => {
            let (ty, fixed_size) = match kind.as_str() {
                "bool" => (FieldType::Bool, Some(1)),
                "u8" => (FieldType::U8, Some(1)),
                "u16" => (FieldType::U16, Some(2)),
                "u32" => (FieldType::U32, Some(4)),
                "u64" => (FieldType::U64, Some(8)),
                "i32" => (FieldType::I32, Some(4)),
                "i64" => (FieldType::I64, Some(8)),
                "u128" => (FieldType::U128, Some(16)),
                "varBytes" => (FieldType::VarBytes, None),
                "enumU8" => {
                    return Err(SchemaIdlError::InvalidSchema(
                        "enumU8 field is missing its defined type name".to_string(),
                    ));
                }
                other => {
                    return Err(SchemaIdlError::InvalidSchema(format!(
                        "unsupported field type: {other}"
                    )));
                }
            };
            Ok(DecodedFieldType {
                ty,
                rust_type_name: leak_string(kind),
                enum_type_name: None,
                fixed_size,
            })
        }
        IdlFieldType::FixedBytes { fixed_bytes } => Ok(DecodedFieldType {
            ty: FieldType::FixedBytes,
            rust_type_name: leak_string(format!("FixedBytes<{fixed_bytes}>")),
            enum_type_name: None,
            fixed_size: fixed_bytes.checked_add(2),
        }),
        IdlFieldType::Defined { defined } => {
            let name = leak_string(defined.clone());
            Ok(DecodedFieldType {
                ty: FieldType::EnumU8,
                rust_type_name: name,
                enum_type_name: Some(name),
                fixed_size: Some(1),
            })
        }
    }
}

fn parse_fingerprint(
    label: &'static str,
    value: &str,
) -> Result<SchemaFingerprint, SchemaIdlError> {
    if value.len() != 32 {
        return Err(SchemaIdlError::InvalidSchema(format!(
            "{label} fingerprint must be 32 hex chars, got {}",
            value.len()
        )));
    }
    let mut out = [0u8; 16];
    for (idx, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let pair = std::str::from_utf8(chunk).map_err(|_| {
            SchemaIdlError::InvalidSchema(format!("{label} fingerprint contains non-utf8 bytes"))
        })?;
        out[idx] = u8::from_str_radix(pair, 16).map_err(|_| {
            SchemaIdlError::InvalidSchema(format!(
                "{label} fingerprint contains invalid hex: {value}"
            ))
        })?;
    }
    Ok(out)
}

fn leak_string(value: impl Into<String>) -> &'static str {
    Box::leak(value.into().into_boxed_str())
}

fn leak_slice<T>(values: Vec<T>) -> &'static [T] {
    Box::leak(values.into_boxed_slice())
}

impl SchemaRegistry {
    /// Serializes this schema registry to stable IDL JSON.
    pub fn to_idl_json(&self) -> String {
        let mut w = JsonWriter::new();
        w.begin_object();

        // idlVersion
        w.key("idlVersion", true);
        w.write_str("1.0");

        // schemaVersion
        w.key("schemaVersion", false);
        w.begin_object();
        w.key("main", true);
        w.write_u32(self.schema_version().main() as u32);
        w.key("minor", false);
        w.write_u32(self.schema_version().minor() as u32);
        w.end_object();

        // fingerprints
        w.key("fingerprints", false);
        w.begin_object();
        w.key("records", true);
        w.write_str(&hex16(&self.record_schema_fingerprint()));
        w.key("commands", false);
        w.write_str(&hex16(&self.command_schema_fingerprint()));
        w.key("events", false);
        w.write_str(&hex16(&self.event_schema_fingerprint()));
        w.key("types", false);
        w.write_str(&hex16(&self.types_schema_fingerprint()));
        w.end_object();

        // types (enums)
        w.key("types", false);
        w.begin_array();
        let mut first_type = true;
        for def in self.enum_defs() {
            w.array_sep(first_type);
            first_type = false;
            w.begin_object();
            w.key("name", true);
            w.write_str(def.name);
            w.key("kind", false);
            w.write_str("enumU8");
            w.key("variants", false);
            w.begin_array();
            let mut first_v = true;
            for v in def.variants {
                w.array_sep(first_v);
                first_v = false;
                w.begin_object();
                w.key("name", true);
                w.write_str(v.name);
                w.key("value", false);
                w.write_u32(v.discriminant as u32);
                w.end_object();
            }
            w.end_array();
            w.end_object();
        }
        w.end_array();

        // records
        w.key("records", false);
        w.begin_array();
        let mut first_rec = true;
        for def in self.record_defs() {
            w.array_sep(first_rec);
            first_rec = false;
            w.begin_object();
            w.key("kind", true);
            w.write_u32(def.kind as u32);
            w.key("name", false);
            w.write_str(def.name);
            w.key("version", false);
            w.write_u32(def.version as u32);
            w.key("dataSize", false);
            w.write_u32(def.data_size);
            w.key("hasPk", false);
            w.write_bool(def.is_pk_idx);

            w.key("pkFields", false);
            w.begin_array();
            let mut first_pk = true;
            for pk in def.pk_fields {
                w.array_sep(first_pk);
                first_pk = false;
                w.write_str(pk);
            }
            w.end_array();

            w.key("supportRangeScan", false);
            w.write_bool(def.support_range_scan);

            w.key("fields", false);
            w.begin_array();
            let mut first_field = true;
            for field in def.fields {
                w.array_sep(first_field);
                first_field = false;
                w.begin_object();
                w.key("name", true);
                w.write_str(field.name);
                w.key("index", false);
                w.write_u32(field.field_index);
                w.key("type", false);
                // For record fields, fixed_size is field.len
                field_type_json(
                    &mut w,
                    field.ty,
                    field.rust_type_name,
                    field.enum_type_name,
                    Some(field.len),
                );
                w.key("offset", false);
                w.write_u32(field.offset);
                w.key("size", false);
                w.write_u32(field.len);
                w.key("immutable", false);
                w.write_bool(field.immutable);
                w.end_object();
            }
            w.end_array();
            w.end_object();
        }
        w.end_array();

        // commands
        w.key("commands", false);
        w.begin_array();
        let mut first_cmd = true;
        for def in self.command_defs() {
            w.array_sep(first_cmd);
            first_cmd = false;
            w.begin_object();
            w.key("kind", true);
            w.write_u32(def.kind as u32);
            w.key("name", false);
            w.write_str(def.name);
            w.key("version", false);
            w.write_u32(def.version as u32);
            w.key("fields", false);
            w.begin_array();
            let mut first_field = true;
            for field in def.fields {
                w.array_sep(first_field);
                first_field = false;
                w.begin_object();
                w.key("name", true);
                w.write_str(field.name);
                w.key("index", false);
                w.write_u32(field.field_index);
                w.key("type", false);
                field_type_json(
                    &mut w,
                    field.ty,
                    field.rust_type_name,
                    field.enum_type_name,
                    field.fixed_size,
                );
                w.end_object();
            }
            w.end_array();
            w.end_object();
        }
        w.end_array();

        // events
        w.key("events", false);
        w.begin_array();
        let mut first_evt = true;
        for def in self.event_defs() {
            w.array_sep(first_evt);
            first_evt = false;
            w.begin_object();
            w.key("kind", true);
            w.write_u32(def.kind as u32);
            w.key("name", false);
            w.write_str(def.name);
            w.key("version", false);
            w.write_u32(def.version as u32);
            w.key("fields", false);
            w.begin_array();
            let mut first_field = true;
            for field in def.fields {
                w.array_sep(first_field);
                first_field = false;
                w.begin_object();
                w.key("name", true);
                w.write_str(field.name);
                w.key("index", false);
                w.write_u32(field.field_index);
                w.key("type", false);
                field_type_json(
                    &mut w,
                    field.ty,
                    field.rust_type_name,
                    field.enum_type_name,
                    field.fixed_size,
                );
                w.end_object();
            }
            w.end_array();
            w.end_object();
        }
        w.end_array();

        w.end_object();
        w.buf.push('\n');
        w.finish()
    }
}

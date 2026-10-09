use crate::plugin_v3::ffi::*;
use std::mem::{align_of, offset_of, size_of};
use statevec_model::{SchemaIdentity, Version};

#[test]
fn core_tables_match_the_reviewed_64_bit_c_layout() {
    assert_eq!(size_of::<SvBytes>(), 16);
    assert_eq!(size_of::<SvHeader>(), 32);
    assert_eq!(size_of::<SvDiscovery>(), 16);
    assert_eq!(size_of::<SvCoreV3>(), 112);
    assert_eq!(align_of::<SvCoreV3>(), 8);
    assert_eq!(offset_of!(SvCoreV3, build_target), 32);
    assert_eq!(offset_of!(SvCoreV3, configure), 48);
    assert_eq!(offset_of!(SvCoreV3, destroy_configured), 104);
    assert_eq!(size_of::<SvCoreMetadata>(), 128);
    assert_eq!(offset_of!(SvCoreMetadata, tx_rules), 48);
    assert_eq!(offset_of!(SvCoreMetadata, schema), 56);
    assert_eq!(offset_of!(SvCoreMetadata, reserved_schema), 122);
    assert_eq!(size_of::<SvCommand>(), 40);
    assert_eq!(offset_of!(SvCommand, ext_seq), 8);
    assert_eq!(offset_of!(SvCommand, payload), 24);
}

#[test]
fn callback_tables_have_no_implicit_padding_between_fields() {
    assert_eq!(size_of::<SvMutBytes>(), 16);
    assert_eq!(size_of::<SvKey>(), 16);
    assert_eq!(offset_of!(SvKey, sys_id), 8);
    assert_eq!(size_of::<SvWalk>(), 16);
    assert_eq!(offset_of!(SvWalk, count), 8);
    assert_eq!(size_of::<SvTxV3>(), 144);
    assert_eq!(offset_of!(SvTxV3, context), 32);
    assert_eq!(offset_of!(SvTxV3, time_ns), 40);
    assert_eq!(offset_of!(SvTxV3, emit_event), 136);
    assert_eq!(size_of::<SvCandidateChange>(), 64);
    assert_eq!(offset_of!(SvCandidateChange, operation), 16);
    assert_eq!(offset_of!(SvCandidateChange, before), 32);
    assert_eq!(size_of::<SvCandidateV3>(), 72);
    assert_eq!(offset_of!(SvCandidateV3, changes), 64);
}

#[test]
fn schema_wire_identity_preserves_all_independent_fingerprints() {
    let wire = SvSchemaIdentity::from(SchemaIdentity {
        schema_version: Version::new(3, 7),
        types_schema_fingerprint: [1; 16],
        record_schema_fingerprint: [2; 16],
        command_schema_fingerprint: [3; 16],
        event_schema_fingerprint: [4; 16],
    });

    assert_eq!(size_of::<SvSchemaIdentity>(), 66);
    assert_eq!(align_of::<SvSchemaIdentity>(), 1);
    assert_eq!((wire.version_main, wire.version_minor), (3, 7));
    assert_eq!((wire.types, wire.records, wire.commands, wire.events), ([1; 16], [2; 16], [3; 16], [4; 16]));
}

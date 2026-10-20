use crate::v1::ffi::*;
use std::mem::{align_of, offset_of, size_of};
use statevec_model::{SchemaIdentity, Version};

#[test]
fn core_tables_match_the_64_bit_c_layout() {
    assert_eq!(size_of::<SvBytes>(), 16);
    assert_eq!(align_of::<SvBytes>(), 8);
    assert_eq!(offset_of!(SvBytes, ptr), 0);
    assert_eq!(offset_of!(SvBytes, len), 8);
    assert_eq!(size_of::<SvHeader>(), 32);
    assert_eq!(align_of::<SvHeader>(), 8);
    assert_eq!(offset_of!(SvHeader, abi_major), 0);
    assert_eq!(offset_of!(SvHeader, kind), 4);
    assert_eq!(offset_of!(SvHeader, table_bytes), 8);
    assert_eq!(offset_of!(SvHeader, provided_caps), 16);
    assert_eq!(offset_of!(SvHeader, required_caps), 24);
    assert_eq!(size_of::<SvDiscovery>(), 16);
    assert_eq!(align_of::<SvDiscovery>(), 8);
    assert_eq!(offset_of!(SvDiscovery, table), 0);
    assert_eq!(offset_of!(SvDiscovery, table_bytes), 8);
    assert_eq!(CORE_ABI_MAJOR, 5);
    assert_eq!(CORE_LIFECYCLE_CAPABILITY, 128);
    assert_eq!(CORE_MIGRATION_KIND, 6);
    assert_eq!(size_of::<SvCoreV5>(), 128);
    assert_eq!(align_of::<SvCoreV5>(), 8);
    assert_eq!(offset_of!(SvCoreV5, build_target), 32);
    assert_eq!(offset_of!(SvCoreV5, configure), 48);
    assert_eq!(offset_of!(SvCoreV5, initialize_core), 88);
    assert_eq!(offset_of!(SvCoreV5, migrate), 96);
    assert_eq!(offset_of!(SvCoreV5, destroy_configured), 120);
    assert_eq!(size_of::<SvCoreMetadata>(), 128);
    assert_eq!(offset_of!(SvCoreMetadata, tx_rules), 48);
    assert_eq!(offset_of!(SvCoreMetadata, schema), 56);
    assert_eq!(offset_of!(SvCoreMetadata, reserved_schema), 122);
    assert_eq!(size_of::<SvCommand>(), 40);
    assert_eq!(offset_of!(SvCommand, ext_seq), 8);
    assert_eq!(offset_of!(SvCommand, payload), 24);
}

#[test]
fn visitor_slots_represent_c_null_before_checked_dispatch() {
    assert_eq!(size_of::<SvReadVisitor>(), size_of::<*const ()>());
    assert_eq!(size_of::<SvWriteVisitor>(), size_of::<*const ()>());
    assert_eq!(size_of::<SvKeyVisitor>(), size_of::<*const ()>());
    assert_eq!(size_of::<SvChangeVisitor>(), size_of::<*const ()>());

    // SAFETY: all-zero Option<extern "C" fn> is the documented nullable
    // function-pointer representation. No callback or execution bridge runs.
    let (read, write, key, change): (SvReadVisitor, SvWriteVisitor, SvKeyVisitor, SvChangeVisitor) =
        unsafe { std::mem::zeroed() };
    assert!(read.is_none());
    assert!(write.is_none());
    assert!(key.is_none());
    assert!(change.is_none());
}

#[test]
fn callback_tables_have_no_implicit_padding_between_fields() {
    assert_eq!(size_of::<SvMutBytes>(), 16);
    assert_eq!(size_of::<SvKey>(), 16);
    assert_eq!(offset_of!(SvKey, sys_id), 8);
    assert_eq!(size_of::<SvWalk>(), 16);
    assert_eq!(offset_of!(SvWalk, count), 8);
    assert_eq!(size_of::<SvTxV5>(), 144);
    assert_eq!(offset_of!(SvTxV5, context), 32);
    assert_eq!(offset_of!(SvTxV5, time_ns), 40);
    assert_eq!(offset_of!(SvTxV5, emit_event), 136);
    assert_eq!(size_of::<SvCandidateChange>(), 64);
    assert_eq!(offset_of!(SvCandidateChange, operation), 16);
    assert_eq!(offset_of!(SvCandidateChange, before), 32);
    assert_eq!(size_of::<SvCandidateV5>(), 72);
    assert_eq!(offset_of!(SvCandidateV5, changes), 64);
    assert_eq!(size_of::<SvMigrationV5>(), 104);
    assert_eq!(offset_of!(SvMigrationV5, context), 32);
    assert_eq!(offset_of!(SvMigrationV5, source_read), 56);
    assert_eq!(offset_of!(SvMigrationV5, preserve_record), 72);
    assert_eq!(offset_of!(SvMigrationV5, emit_event), 88);
    assert_eq!(offset_of!(SvMigrationV5, delete_record), 96);
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

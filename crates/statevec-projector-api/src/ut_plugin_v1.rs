use super::*;
use ffi::*;
use test_case::test_case;
use std::mem::{offset_of, size_of};
use ProjectorOperation as O;
use ProjectorAbiFailure as A;
use ProjectorFailure as F;
use ProjectorStatusDecodeFailure as D;

#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
#[test]
fn c11_record_change_layout_agrees_with_the_public_contract() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/projector_record_layout.c");
    let result = std::process::Command::new("cc")
        .args(["-std=c11", "-Wpadded", "-Werror", "-fsyntax-only"])
        .arg(fixture)
        .output()
        .expect("the Linux ABI gate requires a C11 compiler");
    assert!(result.status.success(), "C11 ABI assertions: {}", String::from_utf8_lossy(&result.stderr));
}

#[test_case(size_of::<SvProjectorV1>(), 112; "plugin_table")]
#[test_case(size_of::<SvProjectorHostV1>(), 184; "host_table")]
#[test_case(size_of::<SvProjectorId>(), 136; "id")]
#[test_case(size_of::<SvConsumeOffset>(), 184; "offset")]
#[test_case(size_of::<SvRecovery>(), 192; "recovery")]
#[test_case(size_of::<SvProjectionIdentity>(), 104; "identity")]
#[test_case(size_of::<SvPosition>(), 192; "position")]
#[test_case(size_of::<SvDeliveryLimits>(), 16; "limits")]
#[test_case(size_of::<SvProjectorPrepare>(), 296; "prepare")]
#[test_case(size_of::<SvProjectorStart>(), 320; "start")]
#[test_case(size_of::<SvStateInfo>(), 200; "state_info")]
#[test_case(size_of::<SvStateRecord>(), 40; "state_record")]
#[test_case(size_of::<SvCut>(), 264; "cut")]
#[test_case(size_of::<SvRebuildCut>(), 264; "rebuild_cut")]
#[test_case(size_of::<SvOffsetCheckpoint>(), 264; "checkpoint")]
#[test_case(size_of::<SvFrameInfo>(), 480; "frame")]
#[test_case(size_of::<SvTransactionInfo>(), 200; "transaction")]
#[test_case(size_of::<SvEvent>(), 24; "event")]
#[test_case(size_of::<SvRecordChange>(), 64; "change")]
#[test_case(size_of::<SvControlReply>(), 24; "reply")]
#[test_case(size_of::<SvProjectorFailureObservation>(), 40; "observation")]
#[test_case(size_of::<SvProjectorCompletion>(), 240; "completion")]
#[test_case(size_of::<SvStatus>(), 32; "shared_status")]
#[cfg(target_pointer_width = "64")]
fn frozen_v1_extents(actual: usize, expected: usize) { assert_eq!(actual, expected); }

#[test_case(offset_of!(SvConsumeOffset, projector_id), 32; "offset_id")]
#[test_case(offset_of!(SvConsumeOffset, raft_term), 168; "offset_term")]
#[test_case(offset_of!(SvConsumeOffset, raft_index), 176; "offset_index")]
#[test_case(offset_of!(SvProjectorPrepare, recovery_mode), 152; "prepare_mode")]
#[test_case(offset_of!(SvProjectorPrepare, projector_id), 160; "prepare_id")]
#[test_case(offset_of!(SvProjectorStart, projector_id), 112; "start_id")]
#[test_case(offset_of!(SvProjectorStart, recovery_mode), 304; "start_mode")]
#[test_case(offset_of!(SvProjectorStart, initialization), 308; "start_initialization")]
#[test_case(offset_of!(SvProjectorStart, rebuild_target), 312; "start_target")]
#[test_case(offset_of!(SvProjectorCompletion, run_failure), 8; "run")]
#[test_case(offset_of!(SvProjectorCompletion, service_join_failure), 40; "services")]
#[test_case(offset_of!(SvProjectorCompletion, notification_failure), 72; "notification")]
#[test_case(offset_of!(SvProjectorCompletion, worker_join_failure), 104; "worker")]
#[test_case(offset_of!(SvProjectorCompletion, frame_release_failure), 136; "frame_release")]
#[test_case(offset_of!(SvProjectorCompletion, state_release_failure), 168; "state_release")]
#[test_case(offset_of!(SvProjectorCompletion, reply_release_failure), 200; "reply_release")]
#[test_case(offset_of!(SvProjectorCompletion, failed_service_joins), 232; "service_count")]
#[test_case(offset_of!(SvTransactionInfo, ref_tx_time_ns), 176; "transaction_time")]
#[test_case(offset_of!(SvRecordChange, before), 32; "change_before")]
#[test_case(offset_of!(SvRecordChange, after), 48; "change_after")]
#[cfg(target_pointer_width = "64")]
fn frozen_v1_offsets(actual: usize, expected: usize) { assert_eq!(actual, expected); }

fn projector_status(cause: u32, subcategory: u64) -> SvStatus { SvStatus { tag: 12, cause, detail: [subcategory, 0, 0] } }

#[test_case(O::Prepare, 1, 42; "prepare_application")]
#[test_case(O::Prepare, 2, 2; "prepare_store_read")]
#[test_case(O::Start, 2, 8; "start_worker_spawn")]
#[test_case(O::StateInfo, 5, 1; "info_unavailable")]
#[test_case(O::StateNext, 5, 2; "next_invalid_record")]
#[test_case(O::FinishState, 5, 3; "finish_incomplete")]
#[test_case(O::ReleaseState, 5, 5; "release_abandoned")]
fn allowed_returns_cannot_be_disguised_as_illegal(operation: O, cause: u32, subcategory: u64) {
    let status = projector_status(cause, subcategory);
    assert!(decode_call(status, operation).unwrap().is_some());
    let stage = if cause == 2 { ProjectorIoStage::from_code(subcategory as u32) } else { None };
    let rebuild_reason = if cause == 5 { RebuildSourceFailure::from_code(subcategory as u32) } else { None };
    let raw = SvStatus { tag: 9, cause: 102, detail: [operation.code().into(), 12 | (u64::from(cause) << 32),
        u64::from(stage.map_or(0, ProjectorIoStage::code)) | u64::from(rebuild_reason.map_or(0, RebuildSourceFailure::code))] };
    assert_eq!(decode_call(raw, O::Join), Err(A::Status(D::NoncanonicalNestedFailure { cause: 102 })));
}

#[test_case(O::Prepare, 3, 0; "prepare_worker_join")]
#[test_case(O::StateInfo, 2, 2; "info_store_read")]
#[test_case(O::Start, 2, 1; "start_store_open")]
#[test_case(O::OfferCheckpoint, 1, 17; "checkpoint_application")]
#[test_case(O::OfferRebuild, 1, 17; "rebuild_application")]
#[test_case(O::StateInfo, 5, 5; "info_abandoned")]
#[test_case(O::StateNext, 5, 3; "next_incomplete")]
fn illegal_returns_preserve_operation_cause_and_subcategory(operation: O, cause: u32, subcategory: u64) {
    let expected = A::Status(D::IllegalReturn {
        operation, tag: 12, cause,
        stage: if cause == 2 { ProjectorIoStage::from_code(subcategory as u32) } else { None },
        rebuild_reason: if cause == 5 { RebuildSourceFailure::from_code(subcategory as u32) } else { None },
    });
    assert_eq!(decode_call(projector_status(cause, subcategory), operation), Err(expected));
    assert_eq!(decode_call((&expected).into(), O::Join), Ok(Some(ProjectorCallFailure::Failure(F::AbiContract(expected)))));
}

#[test_case(1; "business")]
#[test_case(2; "schema")]
#[test_case(3; "access")]
#[test_case(5; "time")]
#[test_case(6; "position")]
fn core_only_tags_are_illegal_without_interpreting_core_payload(tag: u32) {
    let raw = SvStatus { tag, cause: u32::MAX, detail: [u64::MAX; 3] };
    let expected = A::Status(D::IllegalReturn { operation: O::Receive, tag, cause: u32::MAX, stage: None, rebuild_reason: None });
    assert_eq!(decode_call(raw, O::Receive), Err(expected));
    assert!(decode_call((&expected).into(), O::Receive).is_ok());
}
#[test]
fn structural_failures_precede_operation_legality_and_core_is_unchanged() {
    assert_eq!(decode_call(projector_status(99, 0), O::Start), Err(A::Status(D::UnknownCause { tag: 12, cause: 99 })));
    assert_eq!(decode_call(projector_status(2, 0), O::Start), Err(A::Flag { actual: 0 }));
    assert_eq!(decode_call(projector_status(2, 11), O::Start), Err(A::Flag { actual: 11 }));
    assert_eq!(projector_status(1, 0).decode_for(statevec_api::plugin_v3::CoreOperation::RunTx),
        Err(statevec_api::plugin_v3::StatusDecodeFailure::UnknownTag { tag: 12 }));
}

#[test_case(F::HostPanicked; "stop_host_panic")]
#[test_case(F::Host(HostFailure::Resource(ResourceFailure::Os { code: 5 })); "stop_host_io")]
fn explicit_stop_forwards_host_failure_and_rejects_a_false_illegal_return(failure: F) {
    let raw = SvStatus::from(&failure);
    assert_eq!(decode_call(raw, O::RequestStop), Ok(Some(ProjectorCallFailure::Failure(failure))));
    let falsely_illegal = SvStatus { tag: 9, cause: 102,
        detail: [O::RequestStop.code().into(), u64::from(raw.tag) | (u64::from(raw.cause) << 32), 0] };
    assert_eq!(decode_failure(falsely_illegal), Err(A::Status(D::NoncanonicalNestedFailure { cause: 102 })));
    assert!(matches!(decode_call(raw, O::PollWorker), Err(A::Status(D::IllegalReturn { .. }))));
}

#[test_case(0; "nul_is_not_control_character")]
#[test_case(32; "space")]
#[test_case(160; "nonbreaking_space")]
#[test_case(0x110000; "invalid_unicode")]
fn invalid_control_character_diagnostics_refuse(scalar: u32) {
    let raw = SvStatus { tag: 9, cause: 16, detail: [scalar.into(), 0, 0] };
    assert_eq!(decode_call(raw, O::Event), Err(A::Flag { actual: scalar }));
}

#[test_case(106; "core_storage")]
#[test_case(107; "core_business")]
#[test_case(99; "below_status_range")]
#[test_case(111; "above_status_range")]
fn nested_noncanonical_diagnostic_has_one_closed_level(cause: u32) {
    let raw = SvStatus { tag: 9, cause: 110, detail: [cause.into(), 0, 0] };
    assert_eq!(decode_call(raw, O::Prepare), Err(A::Status(D::NoncanonicalNestedFailure { cause: 110 })));
}

#[test_case(104, [11, 1, 0]; "unexpected_expected_tag")]
#[test_case(104, [12, 4, 0]; "unexpected_known_projector_tag")]
#[test_case(108, [0, 8, 7]; "invented_scalar_bound")]
#[test_case(108, [1, 256, 255]; "bound_on_wrong_slot")]
fn nested_diagnostics_must_describe_a_real_projector_decode_check(cause: u32, detail: [u64; 3]) {
    assert_eq!(decode_failure(SvStatus { tag: 9, cause, detail }), Err(A::Status(D::NoncanonicalNestedFailure { cause })));
}

#[test_case(A::Status(D::IllegalReturn { operation: O::Prepare, tag: 12, cause: 3, stage: Some(ProjectorIoStage::StoreRead), rebuild_reason: None }); "inactive_stage")]
#[test_case(A::Status(D::IllegalReturn { operation: O::StateInfo, tag: 12, cause: 2, stage: Some(ProjectorIoStage::StoreRead), rebuild_reason: Some(RebuildSourceFailure::Unavailable) }); "both_subcategories")]
#[test_case(A::Status(D::IllegalReturn { operation: O::Prepare, tag: 12, cause: 1, stage: None, rebuild_reason: None }); "allowed_tuple")]
#[test_case(A::Status(D::NoncanonicalNestedFailure { cause: 999 }); "invalid_inner_cause")]
#[test_case(A::ControlCharacter { scalar: 32 }; "invalid_control_scalar")]
fn unencodable_author_diagnostics_produce_a_canonical_failure(source: A) {
    let status = SvStatus::from(&source);
    assert!(matches!(decode_failure(status), Ok(Some(ProjectorCallFailure::Failure(F::AbiContract(_))))), "{source:?} encoded as noncanonical {status:?}");
    assert!(status.cause != 102 && status.cause != 16, "do not reinterpret malformed author fields as a different legitimate claim");
}

#[test_case(0; "fresh_inactive_offset")]
#[test_case(2; "latest_inactive_offset")]
fn inactive_recovery_offset_uses_reserved_cause(tag: u32) {
    let raw = SvRecovery { tag, offset: SvConsumeOffset { raft_term: 7, ..Default::default() }, ..Default::default() };
    assert_eq!(raw.decode(), Err(A::Reserved { actual: 7 }));
}

#[test_case(SvStatus { tag: 1, cause: 0, detail: [0; 3] }; "actual_unexpected_tag")]
#[test_case(SvStatus { tag: 9, cause: 108, detail: [256, 0, 0] }; "slot_byte_overflow")]
#[test_case(SvStatus { tag: 12, cause: 2, detail: [1, u64::MAX, 0] }; "kind_word_overflow")]
#[test_case(SvStatus { tag: 12, cause: 2, detail: [1, 0, 1u64 << 33] }; "os_encoding_overflow")]
#[test_case(SvStatus { tag: 9, cause: 102, detail: [1, 12 | (2u64 << 32), u64::MAX] }; "nested_stage_word_overflow")]
fn real_structural_decode_failures_round_trip_as_canonical_diagnostics(raw: SvStatus) {
    let failure = decode_failure(raw).expect_err("actual malformed wire value");
    assert_eq!(decode_failure((&failure).into()), Ok(Some(ProjectorCallFailure::Failure(F::AbiContract(failure)))));
}

#[test_case(None; "no_os_code")]
#[test_case(Some(0); "os_zero")]
#[test_case(Some(-1); "os_negative")]
#[test_case(Some(i32::MIN); "os_minimum")]
#[test_case(Some(i32::MAX); "os_maximum")]
fn os_code_preserves_option_and_signed_bits(os_code: Option<i32>) {
    let failure = F::Io { stage: ProjectorIoStage::StoreRead, kind: ProjectorIoKind::StorageFull, os_code };
    assert_eq!(decode_call((&failure).into(), O::Prepare), Ok(Some(ProjectorCallFailure::Failure(failure))));
}

#[test]
fn independent_cleanup_slots_preserve_late_run_failure_and_earlier_observation() {
    let failure = F::Application { code: 42 };
    let release = F::HostPanicked;
    let report = SvProjectorCompletion {
        joined: 1, run_failure: (&failure).into(),
        frame_release_failure: (&release).into(), state_release_failure: (&release).into(),
        reply_release_failure: (&release).into(), ..SvProjectorCompletion::EMPTY
    };
    let decoded = report.decode(Some(ProjectorWorkerFailure::StateRelease(release))).unwrap();
    assert_eq!(decoded.run_failure, Some(failure));
    assert_eq!(decoded.frame_release_failure, Some(release));
    assert_eq!(decoded.state_release_failure, Some(release));
    assert_eq!(decoded.reply_release_failure, Some(release));
    assert!(decoded.joined);
    assert_eq!(report.decode(Some(ProjectorWorkerFailure::Run(release))), Err(A::InvalidHandle));
}
#[test]
fn unconfirmed_retirement_preserves_every_cause_without_becoming_success() {
    let failure = F::Application { code: 5 };
    let report = SvProjectorCompletion { run_failure: (&failure).into(), reply_release_failure: (&F::HostPanicked).into(), ..SvProjectorCompletion::EMPTY };
    let decoded = report.decode(None).unwrap();
    assert!(!decoded.joined);
    assert_eq!(decoded.run_failure, Some(failure));
    assert_eq!(decoded.reply_release_failure, Some(F::HostPanicked));
    assert!(SvProjectorCompletion { run_failure: (&failure).into(), ..SvProjectorCompletion::EMPTY }.decode(None).is_err());
}
#[test]
fn completion_checks_slot_families_and_service_join_count() {
    let service: SvStatus = (&ProjectorCallFailure::ServiceJoinPanicked).into();
    let report = SvProjectorCompletion { joined: 1, service_join_failure: service, failed_service_joins: 2, ..SvProjectorCompletion::EMPTY };
    assert_eq!(report.decode(None).unwrap().failed_service_joins, 2);
    assert!(SvProjectorCompletion { failed_service_joins: 0, ..report }.decode(None).is_err());
    assert!(SvProjectorCompletion { service_join_failure: SvStatus::SUCCESS, ..report }.decode(None).is_err());
    let abandonment: SvStatus = (&F::RebuildSource(RebuildSourceFailure::RebuildAbandoned)).into();
    assert!(decode_observation(SvProjectorFailureObservation { slot: 3, reserved: 0, cause: abandonment }).is_err());
    assert_eq!(decode_observation(SvProjectorFailureObservation { slot: 1, reserved: 0, cause: abandonment }), Ok(Some(ProjectorWorkerFailure::Run(F::RebuildSource(RebuildSourceFailure::RebuildAbandoned)))));
    assert!(decode_observation(SvProjectorFailureObservation { slot: 1, reserved: 0, cause: service }).is_err());
}

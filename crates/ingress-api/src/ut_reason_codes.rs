use super::*;
use statevec_frame::read_protocol::{CLIENT_READ_LIMITS, ReadRefusal};
use statevec_frame::stream_batch_protocol::REPLY_OBSERVATION_BYTES;
use statevec_frame::{CLIENT_STREAM_BATCH_COMMITMENT_BYTES, EngineCommandPayload, InputRef, PayloadCodec};

const FRAME_LIMIT: usize = 4096;

fn context() -> RequestContext {
    RequestContext {
        cluster_identity: Digest32::new([1; 32]),
        genesis_identity: Digest32::new([2; 32]),
        execution_lineage: Digest32::new([3; 32]),
        session_incarnation: NonZeroU128::new(4).unwrap(),
        request_id: NonZeroU64::new(5).unwrap(),
    }
}

fn batch_binding() -> ReplyBinding {
    let batch = ClientStreamBatch::try_new(
        InputRef { client_id: 7, stream_id: 9, client_seq: 42, request_id: None },
        vec![EngineCommandPayload { payload_codec: PayloadCodec::runtime_binary_v0(), payload_bytes: [1].into() }],
        FRAME_LIMIT,
        1,
    )
    .unwrap();
    ReplyBinding::for_request(&Request::Submit { context: context(), batch })
}

#[test]
fn batch_reply_decodes_each_refusal_code() {
    use BatchSubmitRefusal::*;
    let assigned = [
        NotReady, Busy, TooLarge, InvalidInput, Unauthorized, ShuttingDown, IncompatibleContext, InternalFailure,
        CoreNotPrepared, CoreRuntimeFailure,
    ];
    let known = Reply { binding: batch_binding(), observation: None, disposition: ReplyDisposition::Refused(Busy) };
    let mut bytes = encode_reply(&known, FRAME_LIMIT, 1).unwrap();
    let refusal = bytes.len() - 17;
    for observed in 0..=u8::MAX {
        bytes[refusal] = observed;
        bytes = finish_frame(bytes, reply::REPLY).unwrap();
        let result = decode_reply(&bytes, FRAME_LIMIT, 1);
        if observed == 0 {
            assert_eq!(result, Err(FrameError::UnsupportedRefusal { observed }));
            continue;
        }
        let expected = assigned.get(usize::from(observed - 1)).copied().unwrap_or(Unrecognized { observed });
        let decoded = result.unwrap_or_else(|error| panic!("batch refusal code {observed}: {error}"));
        assert_eq!(decoded.disposition, ReplyDisposition::Refused(expected), "batch refusal code {observed}");
        assert_eq!(encode_reply(&decoded, FRAME_LIMIT, 1).unwrap(), bytes, "batch refusal code {observed}");
    }
}

#[test]
fn batch_reply_decodes_each_unavailable_reason_code() {
    use IngressUnavailableReason::*;
    let assigned = [
        NotLeader, LeaderNotReady, EngineOccupied, ReadyBacklog, SnapshotInstalling, HistoryScanLimited, Draining,
        Failed,
    ];
    let known = Reply {
        binding: batch_binding(),
        observation: Some(ReplyObservation::new(9, 80, 6, None, None).unwrap()),
        disposition: ReplyDisposition::Unknown,
    };
    let mut bytes = encode_reply(&known, FRAME_LIMIT, 1).unwrap();
    let reason = HEADER_BYTES + CONTEXT_BYTES + CLIENT_STREAM_BATCH_COMMITMENT_BYTES + REPLY_OBSERVATION_BYTES - 1;
    for observed in 0..=u8::MAX {
        bytes[reason] = observed;
        bytes = finish_frame(bytes, reply::REPLY).unwrap();
        let expected = observed.checked_sub(1).map(|index| {
            assigned.get(usize::from(index)).copied().unwrap_or(Unrecognized { observed })
        });
        let decoded = decode_reply(&bytes, FRAME_LIMIT, 1)
            .unwrap_or_else(|error| panic!("batch unavailable reason code {observed}: {error}"));
        assert_eq!(decoded.observation.unwrap().unavailable_reason(), expected, "unavailable code {observed}");
        assert_eq!(encode_reply(&decoded, FRAME_LIMIT, 1).unwrap(), bytes, "unavailable code {observed}");
    }
}

#[test]
fn read_reply_decodes_each_refusal_code_and_keeps_its_argument() {
    use ReadRefusal::*;
    const ARGUMENT: u64 = 0x8877_6655_4433_2211;
    let assigned = [
        IncompatibleContext, Fatal, Draining, ReadsDisabled, NotVoter, LeaderReadsDisabled, RecoveryPending,
        SnapshotInstalling, RequestTooLarge, Busy, NotYetVisible { applied_tx_seq: ARGUMENT }, UnknownKind,
        UnknownUniqueKey, ReplyTooLarge, Unavailable,
    ];
    let mut bytes = encode_read_reply(&context(), None, ReadReplyBody::Refused(Busy), FRAME_LIMIT).unwrap();
    let refusal = bytes.len() - 9;
    for observed in 0..=u8::MAX {
        let argument = if observed == 11 || usize::from(observed) > assigned.len() { ARGUMENT } else { 0 };
        bytes[refusal] = observed;
        bytes[refusal + 1..].copy_from_slice(&argument.to_le_bytes());
        bytes = finish_frame(bytes, read::READ_REPLY).unwrap();
        let result = decode_read_reply(&bytes, FRAME_LIMIT, CLIENT_READ_LIMITS);
        if observed == 0 {
            assert_eq!(result, Err(FrameError::UnsupportedReadRefusal { observed }));
            continue;
        }
        let expected = assigned.get(usize::from(observed - 1)).copied().unwrap_or(Unrecognized { observed, argument });
        let decoded = result.unwrap_or_else(|error| panic!("read refusal code {observed}: {error}"));
        assert_eq!(decoded.outcome, ReadOutcome::Refused(expected), "read refusal code {observed}");
        assert_eq!(
            encode_read_reply(&decoded.context, decoded.observation, ReadReplyBody::Refused(expected), FRAME_LIMIT)
                .unwrap(),
            bytes,
            "read refusal code {observed}"
        );
        if argument == 0 {
            bytes[refusal + 1..].copy_from_slice(&ARGUMENT.to_le_bytes());
            bytes = finish_frame(bytes, read::READ_REPLY).unwrap();
            assert_eq!(
                decode_read_reply(&bytes, FRAME_LIMIT, CLIENT_READ_LIMITS),
                Err(FrameError::InvalidReadRefusal),
                "assigned read refusal {observed} still requires a zero argument"
            );
        }
    }
}

#[test]
fn encoders_refuse_unrecognized_values_that_alias_known_codes() {
    for observed in [0, 1] {
        let reply = Reply {
            binding: batch_binding(),
            observation: None,
            disposition: ReplyDisposition::Refused(BatchSubmitRefusal::Unrecognized { observed }),
        };
        assert_eq!(encode_reply(&reply, FRAME_LIMIT, 1), Err(FrameError::UnsupportedRefusal { observed }));

        let reason = IngressUnavailableReason::Unrecognized { observed };
        let observation = ReplyObservation::new(9, 80, 6, None, Some(reason)).unwrap();
        let reply =
            Reply { binding: reply.binding, observation: Some(observation), disposition: ReplyDisposition::Unknown };
        assert_eq!(encode_reply(&reply, FRAME_LIMIT, 1), Err(FrameError::UnsupportedUnavailableReason { observed }));

        let read = ReadReplyBody::Refused(ReadRefusal::Unrecognized { observed, argument: 0 });
        assert_eq!(
            encode_read_reply(&context(), None, read, FRAME_LIMIT),
            Err(FrameError::UnsupportedReadRefusal { observed })
        );
    }
}

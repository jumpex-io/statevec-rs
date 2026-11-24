use super::*;
use test_case::test_case;

// Large enough to reveal eager allocation without a GiB-scale regression probe.
const DECLARED_BYTES: usize = 4 * 1024 * 1024;
const INITIAL_READ_AHEAD: usize = 64 * 1024;

fn header(direction: StreamDirection, length: usize) -> [u8; HEADER_BYTES] {
    let mut bytes = [0; HEADER_BYTES];
    bytes[..4].copy_from_slice(b"SVIN");
    bytes[4..6].copy_from_slice(&super::super::VERSION.to_le_bytes());
    let kind: u16 = match direction { StreamDirection::Requests => 1, StreamDirection::Replies => 3 };
    bytes[6..8].copy_from_slice(&kind.to_le_bytes());
    bytes[8..12].copy_from_slice(&u32::try_from(length - HEADER_BYTES).unwrap().to_le_bytes());
    bytes
}

#[test_case(StreamDirection::Requests; "request")]
#[test_case(StreamDirection::Replies; "reply")]
fn header_alone_cannot_allocate_the_declared_frame(direction: StreamDirection) {
    let mut reader = FrameReader::new(direction, DECLARED_BYTES, 1).unwrap();
    let bytes = header(direction, DECLARED_BYTES);
    assert_eq!(reader.poll_bytes(&mut bytes.as_slice()).unwrap(), ReadProgress::Runnable);
    let PartialFrame::Body { bytes, filled, .. } = &reader.partial else { panic!("validated header") };
    assert_eq!(*filled, HEADER_BYTES);
    assert!(bytes.capacity() <= INITIAL_READ_AHEAD,
        "a header cannot reserve the advertised body: capacity={} declared={DECLARED_BYTES}", bytes.capacity());

    let mut stalled = io::repeat(0).take(0); // EOF before even one body byte.
    assert!(matches!(reader.poll_bytes(&mut stalled), Err(ReadError::Frame { source: FrameError::Wire {
        source: WireError::IncompleteBody { received: 0, declared }
    } }) if declared == DECLARED_BYTES - HEADER_BYTES));
    assert!(matches!(reader.partial, PartialFrame::Closed), "terminal drops the partial buffer");
}

struct Fragments {
    bytes: io::Cursor<Vec<u8>>,
    calls: usize,
    fragment: usize,
    next_error: Option<io::ErrorKind>,
}

impl Read for Fragments {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.calls += 1;
        if let Some(kind) = self.next_error.take() { return Err(kind.into()); }
        let offered = output.len().min(self.fragment);
        self.bytes.read(&mut output[..offered])
    }
}

fn encoded_request() -> Vec<u8> {
    use statevec_frame::{ClientStreamBatch, Digest32, EngineCommandPayload, InputRef, PayloadCodec};
    let batch = ClientStreamBatch::try_new(
        InputRef { client_id: 7, stream_id: 1, client_seq: 19, request_id: None },
        vec![EngineCommandPayload {
            payload_codec: PayloadCodec { codec_id: "runtime_binary".into(), codec_version: 0 },
            payload_bytes: vec![0x5a; 3 * INITIAL_READ_AHEAD + 11].into(),
        }], DECLARED_BYTES, 1,
    ).unwrap();
    let request = Request::Submit {
        context: super::super::RequestContext {
            cluster_identity: Digest32::ZERO, genesis_identity: Digest32::ZERO, execution_lineage: Digest32::ZERO,
            session_incarnation: std::num::NonZeroU128::new(1).unwrap(),
            request_id: std::num::NonZeroU64::new(2).unwrap(),
        }, batch,
    };
    super::super::encode_request(&request, DECLARED_BYTES, 1).unwrap()
}

#[test]
fn progressive_growth_preserves_whole_frames_and_one_read_per_poll() {
    let expected = encoded_request();
    let mut stream = Fragments {
        bytes: io::Cursor::new([expected.as_slice(), expected.as_slice()].concat()),
        calls: 0, fragment: 7001, next_error: None,
    };
    let mut reader = FrameReader::new(StreamDirection::Requests, DECLARED_BYTES, 1).unwrap();
    let mut frames = 0;
    for _ in 0..128 { // Fixture bound for two fragmented frames, not a protocol timeout.
        let before = stream.calls;
        match reader.poll_bytes(&mut stream).unwrap() {
            ReadProgress::Frame(bytes) => {
                assert!(bytes == expected, "whole frame changed: length={} expected={} first_diff={:?}",
                    bytes.len(), expected.len(), bytes.iter().zip(&expected).position(|(a,b)| a != b));
                super::super::decode_request(&bytes, DECLARED_BYTES, 1).unwrap();
                frames += 1;
            }
            ReadProgress::Runnable => {},
            ReadProgress::Eof => { assert_eq!(frames, 2); return; }
            other => panic!("unexpected read outcome: {other:?}"),
        }
        assert_eq!(stream.calls, before + 1);
    }
    panic!("bounded byte fixture must finish both frames");
}

#[test_case(io::ErrorKind::WouldBlock; "wait")]
#[test_case(io::ErrorKind::Interrupted; "runnable")]
fn temporary_read_failure_at_growth_boundary_keeps_exact_prefix(kind: io::ErrorKind) {
    let expected = encoded_request();
    let mut stream = Fragments {
        bytes: io::Cursor::new(expected.clone()), calls: 0, fragment: INITIAL_READ_AHEAD, next_error: None,
    };
    let mut reader = FrameReader::new(StreamDirection::Requests, DECLARED_BYTES, 1).unwrap();
    assert_eq!(reader.poll_bytes(&mut stream).unwrap(), ReadProgress::Runnable); // Header.
    assert_eq!(reader.poll_bytes(&mut stream).unwrap(), ReadProgress::Runnable); // First chunk filled.
    assert_eq!(stream.bytes.position(), INITIAL_READ_AHEAD as u64);
    stream.next_error = Some(kind);
    let progress = reader.poll_bytes(&mut stream).unwrap();
    assert_eq!(progress, if kind == io::ErrorKind::WouldBlock { ReadProgress::WouldBlock } else { ReadProgress::Runnable });
    let PartialFrame::Body { bytes, filled, .. } = &reader.partial else { panic!("retained body") };
    assert_eq!(*filled, INITIAL_READ_AHEAD);
    assert!(bytes[..*filled] == expected[..*filled], "growth must preserve the received prefix");
    for _ in 0..8 {
        if let ReadProgress::Frame(bytes) = reader.poll_bytes(&mut stream).unwrap() {
            assert!(bytes == expected, "resumed frame must remain exact");
            return;
        }
    }
    panic!("fixture should finish after resuming");
}

#[test]
fn reservation_failure_is_typed_without_destroying_received_prefix() {
    let mut bytes = vec![1, 2, 3];
    let failure = grow_body(&mut bytes, usize::MAX).unwrap_err(); // Capacity overflow, no huge allocation.
    assert!(matches!(failure, ReadError::Allocation { requested: usize::MAX, .. }));
    assert_eq!(bytes, [1, 2, 3]);
}

#[derive(Clone, Copy)]
enum EmptyFrame {
    BadChecksum,
    ChangedNumber,
    WrongDirection,
    UnsupportedNumber,
    MissingRequestBody,
}

#[test_case(EmptyFrame::BadChecksum; "changed checksum")]
#[test_case(EmptyFrame::ChangedNumber; "changed number still fails checksum first")]
#[test_case(EmptyFrame::WrongDirection; "verified reply on a request stream")]
#[test_case(EmptyFrame::UnsupportedNumber; "verified unknown format number")]
#[test_case(EmptyFrame::MissingRequestBody; "verified request with an empty body")]
fn an_empty_envelope_is_checked_on_the_header_read_without_consuming_its_successor(case: EmptyFrame) {
    let number = if matches!(case, EmptyFrame::UnsupportedNumber) { 18 } else { super::super::VERSION };
    let kind = if matches!(case, EmptyFrame::WrongDirection) { 3 } else { 1 };
    let format = statevec_frame::wire::WireFormat { magic: *b"SVIN", number, kinds: &[1, 3] };
    let mut prefix = format.encode(kind, &[], 0).unwrap();
    match case {
        EmptyFrame::BadChecksum => prefix[12] ^= 1,
        EmptyFrame::ChangedNumber => prefix[4] ^= 1,
        _ => (),
    }
    let mut input = io::Cursor::new([prefix.as_slice(), encoded_request().as_slice()].concat());
    let mut reader = FrameReader::new(StreamDirection::Requests, DECLARED_BYTES, 1).unwrap();
    let Err(ReadError::Frame { source }) = reader.poll_frame(&mut input) else { panic!("expected frame refusal") };
    match case {
        EmptyFrame::BadChecksum | EmptyFrame::ChangedNumber => assert!(matches!(source, FrameError::Wire {
            source: WireError::Checksum { stored, computed }
        } if stored != computed)),
        EmptyFrame::WrongDirection => assert_eq!(source, WireError::UnsupportedKind { observed: 3 }.into()),
        EmptyFrame::UnsupportedNumber => assert_eq!(source, WireError::UnsupportedNumber { observed: 18 }.into()),
        EmptyFrame::MissingRequestBody => assert_eq!(source, FrameError::InvalidLength),
    }
    assert_eq!(input.position(), HEADER_BYTES as u64);
    assert!(matches!(reader.poll_frame(&mut input), Err(ReadError::Closed)));
}

#[test]
fn the_old_v18_reserved_word_is_refused_by_both_the_stream_and_exact_decoders() {
    let mut previous = encoded_request();
    let old_length = u32::try_from(previous.len() - HEADER_BYTES).unwrap();
    previous[4..6].copy_from_slice(&18u16.to_le_bytes());
    previous[8..12].fill(0);
    previous[12..16].copy_from_slice(&old_length.to_le_bytes());

    let mut cursor = io::Cursor::new(&previous);
    let mut reader = FrameReader::new(StreamDirection::Requests, DECLARED_BYTES, 1).unwrap();
    let Err(ReadError::Frame { source }) = reader.poll_read(&mut cursor) else { panic!("old ingress refused") };
    assert_eq!(cursor.position(), HEADER_BYTES as u64);
    assert!(matches!(source, FrameError::Wire { source: WireError::Checksum { stored, computed } }
        if stored == old_length && stored != computed));
    assert_eq!(super::super::decode_request(&previous, DECLARED_BYTES, 1).unwrap_err(), source);
}

#[test_case(0; "empty body still needs the whole header")]
#[test_case(1; "positive body exceeds the zero-byte body bound")]
fn a_total_limit_shorter_than_the_header_cannot_admit_any_frame(body_length: usize) {
    let format = statevec_frame::wire::WireFormat { magic: *b"SVIN", number: super::super::VERSION, kinds: &[1] };
    let body = vec![0x43; body_length];
    let header = format.encode(1, &body, body_length).unwrap();
    let bytes = [header.as_slice(), &body].concat();
    let maximum = HEADER_BYTES - 1;
    let expected = if body_length == 0 {
        FrameError::EncodedBytesExceeded { observed: HEADER_BYTES, limit: maximum }
    } else {
        WireError::BodyTooLarge { observed: body_length, limit: 0 }.into()
    };
    assert_eq!(super::super::request_frame_len(&header, maximum), Err(expected.clone()));
    assert_eq!(super::super::decode_request(&bytes, maximum, 1), Err(expected));
}

#[test]
fn a_verified_envelope_still_obeys_its_consumers_direction_and_byte_limit() {
    let bytes = encoded_request();
    let mut input = bytes.as_slice();
    let mut reader = FrameReader::new(StreamDirection::Requests, DECLARED_BYTES, 1).unwrap();
    let frame = loop {
        match reader.poll_frame(&mut input).unwrap() {
            ReadProgress::Runnable => (),
            ReadProgress::Frame(frame) => break frame,
            other => panic!("whole request available: {other:?}"),
        }
    };
    let wrong_kind = FrameError::from(WireError::UnsupportedKind { observed: super::super::SUBMIT });
    assert_eq!(frame.decode_reply(DECLARED_BYTES, 1), Err(wrong_kind.clone()));
    assert_eq!(frame.decode_read_reply(DECLARED_BYTES, CLIENT_READ_LIMITS), Err(wrong_kind));
    assert_eq!(frame.decode_reply(HEADER_BYTES, 1), Err(WireError::BodyTooLarge {
        observed: bytes.len() - HEADER_BYTES, limit: 0,
    }.into()));
}

const ROUTE_EXTENSION: [u8; 3] = [0xa5, 0x3c, 0xf0];

fn route_context() -> super::super::RequestContext {
    use statevec_frame::Digest32;

    super::super::RequestContext {
        cluster_identity: Digest32::new([1; 32]),
        genesis_identity: Digest32::new([2; 32]),
        execution_lineage: Digest32::new([3; 32]),
        session_incarnation: std::num::NonZeroU128::MIN,
        request_id: std::num::NonZeroU64::MIN,
    }
}

#[test]
fn route_reply_decoders_ignore_a_checksummed_extension() {
    let context = route_context();
    let observation = routing::RouteObservation {
        peer_id: std::num::NonZeroU64::new(2).unwrap(),
        term: 4,
        leader: std::num::NonZeroU64::new(2),
        applied_index: 25,
        writable: true,
        auto_leave: false,
        voters: vec![1, 2, 3],
        outgoing_voters: vec![1, 3, 4],
        learners: vec![5],
        next_learners: vec![4],
    };
    let original = routing::encode_route_reply(&context, &observation, DECLARED_BYTES).unwrap();
    let mut extended = original.clone();
    extended.extend_from_slice(&ROUTE_EXTENSION);
    assert_eq!(routing::decode_route_reply(&extended, DECLARED_BYTES), Err(WireError::TrailingBytes {
        count: ROUTE_EXTENSION.len(),
    }.into()));

    let extended = super::super::finish_frame(extended, routing::ROUTE_REPLY).unwrap();
    let limit = extended.len();
    let expected = (context, observation);
    assert_eq!(routing::decode_route_reply(&extended, limit), Ok(expected.clone()));

    let stream = [extended.as_slice(), original.as_slice()].concat();
    let mut input = stream.as_slice();
    let mut reader = FrameReader::batch_replies(limit, 1).unwrap();
    let verified = loop {
        match reader.poll_frame(&mut input).unwrap() {
            ReadProgress::Runnable => (),
            ReadProgress::Frame(frame) => break frame,
            other => panic!("expected a whole route reply: {other:?}"),
        }
    };
    assert_eq!(verified.decode_route_reply(limit), Ok(expected));
    assert_eq!(input, original.as_slice(), "the next frame remains untouched");
    assert_eq!(verified.decode_route_reply(original.len()), Err(WireError::BodyTooLarge {
        observed: limit - HEADER_BYTES,
        limit: original.len() - HEADER_BYTES,
    }.into()));

    let mut corrupted = extended;
    *corrupted.last_mut().unwrap() ^= 1;
    assert!(matches!(routing::decode_route_reply(&corrupted, limit), Err(FrameError::Wire {
        source: WireError::Checksum { .. },
    })));
}

#[test]
fn route_request_rejects_a_checksummed_extension() {
    let context = route_context();
    let mut request = routing::encode_route_request(&context, DECLARED_BYTES).unwrap();
    assert_eq!(routing::decode_route_request(&request, DECLARED_BYTES), Ok(context));

    request.extend_from_slice(&ROUTE_EXTENSION);
    let extended = super::super::finish_frame(request, routing::ROUTE).unwrap();
    assert_eq!(routing::decode_route_request(&extended, DECLARED_BYTES), Err(FrameError::InvalidLength));
}

#[test_case(0, io::ErrorKind::TimedOut; "idle OS timeout")]
#[test_case(7, io::ErrorKind::TimedOut; "timed out inside the header")]
#[test_case(HEADER_BYTES + 3, io::ErrorKind::UnexpectedEof; "explicit EOF inside the body")]
#[test_case(HEADER_BYTES + 3, io::ErrorKind::ConnectionReset; "physical reset retains its OS reason")]
fn deadline_and_device_errors_observe_the_real_unfinished_prefix(cut: usize, error: io::ErrorKind) {
    let frame = encoded_request();
    let mut prefix = &frame[..cut];
    let mut reader = FrameReader::new(StreamDirection::Requests, DECLARED_BYTES, 1).unwrap();
    while !prefix.is_empty() {
        assert!(matches!(reader.poll_frame(&mut prefix), Ok(ReadProgress::Runnable)));
    }
    let expected = match cut {
        0 => None,
        n if n < HEADER_BYTES => Some(WireError::IncompleteHeader { received: n }.into()),
        n => Some(WireError::IncompleteBody { received: n - HEADER_BYTES, declared: frame.len() - HEADER_BYTES }.into()),
    };
    assert_eq!(reader.incomplete_frame(), expected);
    struct FailedRead(Option<io::Error>);
    impl Read for FailedRead {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(self.0.take().expect("exactly one device read"))
        }
    }
    let mut device = FailedRead(Some(io::Error::new(error, std::fmt::Error)));
    let failure = reader.poll_frame(&mut device).unwrap_err();
    let original = match (expected, error, failure) {
        (Some(wanted), io::ErrorKind::TimedOut | io::ErrorKind::UnexpectedEof, ReadError::FrameRead { frame, source }) => {
            assert_eq!(frame, wanted);
            source
        }
        (None, _, ReadError::Io { source }) | (_, io::ErrorKind::ConnectionReset, ReadError::Io { source }) => source,
        other => panic!("wrong physical frame refusal: {other:?}"),
    };
    assert_eq!(original.kind(), error);
    assert!(original.get_ref().unwrap().is::<std::fmt::Error>(), "the original device error is retained");
    assert_eq!(reader.incomplete_frame(), None);
}

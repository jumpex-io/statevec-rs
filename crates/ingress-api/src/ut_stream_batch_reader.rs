use super::*;
use test_case::test_case;

// Large enough to reveal eager allocation without a GiB-scale regression probe.
const DECLARED_BYTES: usize = 4 * 1024 * 1024;
const INITIAL_READ_AHEAD: usize = 64 * 1024;

fn header(direction: StreamDirection, length: usize) -> [u8; HEADER_BYTES] {
    let mut bytes = [0; HEADER_BYTES];
    bytes[..4].copy_from_slice(b"SVI0");
    bytes[4..6].copy_from_slice(&super::super::VERSION.to_le_bytes());
    let kind: u16 = match direction { StreamDirection::Requests => 1, StreamDirection::Replies => 3 };
    bytes[6..8].copy_from_slice(&kind.to_le_bytes());
    bytes[12..16].copy_from_slice(&u32::try_from(length - HEADER_BYTES).unwrap().to_le_bytes());
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
    assert!(matches!(reader.poll_bytes(&mut stalled), Err(ReadError::Io { source })
        if source.kind() == io::ErrorKind::UnexpectedEof));
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
            payload_codec: PayloadCodec { codec_id: "runtime_binary_v0".into(), codec_version: 0 },
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

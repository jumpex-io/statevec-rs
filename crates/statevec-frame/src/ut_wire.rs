use crate::wire::{HEADER_BYTES, WireError, WireFormat};
use test_case::test_case;

const TEST: WireFormat = WireFormat { magic: *b"TEST", number: 1, kinds: &[1, 2] };

fn append_frame(header: [u8; HEADER_BYTES], body: &[u8]) -> Vec<u8> {
    let mut bytes = header.to_vec();
    bytes.extend_from_slice(body);
    bytes
}

fn hex_header(s: &str) -> [u8; HEADER_BYTES] {
    assert_eq!(s.len(), HEADER_BYTES * 2);
    std::array::from_fn(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap())
}

// Python struct.pack('<4sHHI', magic, number, kind, len(body)) followed by
// struct.pack('<I', zlib.crc32(prefix + body)). These constants do not use
// the Rust encoder or its CRC implementation.
#[test_case(*b"SVSH", 3, 1, "53565348030001005a000000afa357c4"; "hello")]
#[test_case(*b"SVPE", 1, 1, "5356504501000100060000007a993c58"; "peer")]
#[test_case(*b"SVFQ", 2, 1, "5356465102000100040000008c7ef297"; "file request")]
#[test_case(*b"SVFS", 1, 1, "535646530100010008000000f85e2dee"; "available file")]
#[test_case(*b"SVFS", 1, 2, "5356465301000200080000006f49a252"; "unavailable file")]
#[test_case(*b"SVI0", 19, 2, "535649301300020003000000c1a8a05b"; "ingress")]
#[test_case(*b"SVAM", 1, 1, "5356414d01000100020000004db77b19"; "admin")]
fn independent_vectors_round_trip(magic: [u8; 4], number: u16, kind: u16, header: &str) {
    let body = match &magic {
        b"SVSH" => {
            let mut data = vec![1, 2, 1];
            data.extend_from_slice(&[0; 7]);
            data.extend_from_slice(&1_u64.to_le_bytes());
            data.extend_from_slice(&2_u64.to_le_bytes());
            data.extend_from_slice(&[0x11; 32]);
            data.extend_from_slice(&[0x22; 32]);
            data
        }
        b"SVPE" => vec![8, 3, 16, 1, 24, 2],
        b"SVFQ" => vec![1, 0x23, 0x45, 0x67],
        b"SVFS" if kind == 1 => 0x0102_0304_0506_0708_u64.to_le_bytes().to_vec(),
        b"SVFS" => vec![0; 8],
        b"SVI0" => b"abc".to_vec(),
        b"SVAM" => b"{}".to_vec(),
        _ => unreachable!(),
    };
    let format = WireFormat { magic, number, kinds: &[1, 2] };
    let header = hex_header(header);
    let bytes = append_frame(header, &body);
    let (observed_kind, observed_body) = format.decode_exact(&bytes, body.len()).unwrap();
    assert_eq!((observed_kind, observed_body), (kind, body.as_slice()));
    assert_eq!(format.encode(observed_kind, observed_body, body.len()).unwrap(), header);
}

#[test]
fn every_bit_is_checked_in_protocol_order() {
    let body = b"payload";
    let limit = 32;
    let frame = append_frame(TEST.encode(1, body, limit).unwrap(), body);
    for offset in 0..frame.len() {
        for bit in 0..8 {
            let mut changed = frame.clone();
            changed[offset] ^= 1 << bit;
            let result = TEST.decode_exact(&changed, limit);
            let declared = u32::from_le_bytes(changed[8..12].try_into().unwrap()) as usize;
            match offset {
                0..4 => assert!(matches!(result, Err(WireError::WrongMagic { .. }))),
                8..12 if declared > limit => {
                    assert_eq!(result, Err(WireError::BodyTooLarge { observed: declared, limit }));
                }
                8..12 if declared > body.len() => {
                    assert_eq!(result, Err(WireError::IncompleteBody { received: body.len(), declared }));
                    changed.resize(HEADER_BYTES + declared, 0x6d);
                    assert!(matches!(TEST.decode_exact(&changed, limit), Err(WireError::Checksum { .. })));
                }
                _ => assert!(matches!(result, Err(WireError::Checksum { .. })), "byte {offset}, bit {bit}: {result:?}"),
            }
        }
    }
}

#[test_case(256; "hello including extensions")]
#[test_case(16 * 1024 * 1024 - HEADER_BYTES; "maximum peer")]
#[test_case(2 * 1024 * 1024 - HEADER_BYTES; "shipping ingress")]
#[test_case(4096; "file and admin request")]
#[test_case(8; "file response")]
#[test_case(65_536; "admin reply")]
fn bounded_length_is_checked_before_reading_the_body(limit: usize) {
    let mut body = vec![0x6d; limit];
    let header = TEST.encode(1, &body, limit).unwrap();
    let frame = append_frame(header, &body);
    assert_eq!(TEST.decode_exact(&frame, limit).unwrap(), (1, body.as_slice()));
    assert_eq!(TEST.decode_exact(&frame[..frame.len() - 1], limit),
               Err(WireError::IncompleteBody { received: limit - 1, declared: limit }));
    body.push(0x6d);
    let over = TEST.encode(1, &body, limit + 1).unwrap();
    let too_large = WireError::BodyTooLarge { observed: limit + 1, limit };
    assert_eq!(TEST.header(&over, limit), Err(too_large));
    assert_eq!(TEST.encode(1, &body, limit), Err(too_large));
}

#[test]
fn only_a_verified_frame_exposes_its_tags_and_exact_end() {
    let body = b"abc";
    let header = TEST.encode(1, body, 3).unwrap();
    for end in 0..HEADER_BYTES {
        assert_eq!(TEST.header(&header[..end], 3), Err(WireError::IncompleteHeader { received: end }));
    }
    let other = WireFormat { number: 2, kinds: &[3], ..TEST };
    let foreign = append_frame(other.encode(3, body, 3).unwrap(), body);
    assert_eq!(TEST.decode_exact(&foreign, 3), Err(WireError::UnsupportedNumber { observed: 2 }));
    let wrong_kind = WireFormat { kinds: &[3], ..TEST };
    assert_eq!(TEST.encode(3, body, 3), Err(WireError::UnsupportedKind { observed: 3 }));
    let foreign = append_frame(wrong_kind.encode(3, body, 3).unwrap(), body);
    assert_eq!(TEST.decode_exact(&foreign, 3), Err(WireError::UnsupportedKind { observed: 3 }));

    let empty = TEST.encode(2, &[], 0).unwrap();
    let mut frames = append_frame(header, body);
    frames.extend_from_slice(&empty);
    assert_eq!(TEST.decode_exact(&frames, 3), Err(WireError::TrailingBytes { count: HEADER_BYTES }));
    let first = TEST.header(&frames, 3).unwrap();
    let (one, two) = frames.split_at(HEADER_BYTES + first.body_len());
    assert_eq!(first.verify(&one[HEADER_BYTES..]), Ok(1));
    assert_eq!(TEST.decode_exact(two, 0), Ok((2, [].as_slice())));
}

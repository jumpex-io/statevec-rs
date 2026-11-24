use super::*;
use ingress_api::stream_batch::{
    BatchSubmitRefusal, HEADER_BYTES, Reply, ReplyBinding, ReplyDisposition, ReplyObservation, Request,
    decode_request, encode_reply, request_frame_len,
};
use statevec_frame::outcome::CommitOutcome;
use std::fmt::Write as _;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::thread;
use test_case::test_case;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FirstRequest {
    Reconcile,
    Submit,
}

const PEER_TIMEOUT: Duration = Duration::from_secs(3);

fn sample_path(variable: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(variable).unwrap_or_else(|| panic!("release corpus test requires {variable}")))
}

fn frozen_entry() -> EntryBinding {
    // SHA-256 of the fixture entry "first applied deposit".
    EntryBinding {
        index: NonZeroU64::new(8).unwrap(),
        term: NonZeroU64::new(3).unwrap(),
        digest: EntryDigest::from_untrusted_wire([
            135, 251, 224, 133, 170, 197, 210, 184, 145, 87, 242, 143, 107, 197, 18, 60,
            51, 8, 250, 34, 93, 26, 94, 135, 85, 89, 94, 47, 251, 49, 191, 99,
        ]),
    }
}

fn accept_peer(listener: TcpListener) -> TcpStream {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + PEER_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((peer, _)) => {
                peer.set_nonblocking(false).unwrap();
                peer.set_read_timeout(Some(PEER_TIMEOUT)).unwrap();
                peer.set_write_timeout(Some(PEER_TIMEOUT)).unwrap();
                return peer;
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                thread::park_timeout(Duration::from_millis(10));
            }
            Err(error) => panic!("writer did not connect to the recovery endpoint: {error}"),
        }
    }
}

fn read_recovery_request(peer: &mut TcpStream) -> Request {
    let mut header = [0; HEADER_BYTES];
    peer.read_exact(&mut header).expect("read recovery request header");
    let frame_len = request_frame_len(&header, FRAME_BYTES as usize).expect("bounded recovery request frame");
    let mut frame = header.to_vec();
    frame.resize(frame_len, 0);
    peer.read_exact(&mut frame[HEADER_BYTES..]).expect("read recovery request body");
    decode_request(&frame, FRAME_BYTES as usize, COMMANDS_PER_BATCH as usize)
        .expect("decode recovery request with ingress codec")
}

fn answer_recovery(listener: TcpListener, entry: EntryBinding) -> Request {
    let mut peer = accept_peer(listener);
    let request = read_recovery_request(&mut peer);
    let observation = ReplyObservation::new(entry.term.get(), entry.index.get(), entry.term.get(), None, None).unwrap();
    let outcome = CommitOutcome {
        applied_by: 1,
        tx_seq: 13,
        tx_id: [4; 32],
        tx_chain_hash: [5; 32],
        raft_index: entry.index.get(),
        raft_term: entry.term.get(),
        system_code: 0,
        business_code: 0,
        emitted_event_count: 0,
        inline_events: Vec::new(),
        inline_events_truncated: false,
    };
    let reply = Reply {
        binding: ReplyBinding::for_request(&request),
        observation: Some(observation),
        disposition: ReplyDisposition::Completed { entry, results: vec![MemberResult::Outcome(outcome)] },
    };
    let bytes = encode_reply(&reply, FRAME_BYTES as usize, COMMANDS_PER_BATCH as usize).expect("encode completed reply");
    peer.write_all(&bytes).expect("return the frozen commit binding");
    request
}

#[test_case("STATEVEC_BANK_WRITER_JOURNAL", FirstRequest::Reconcile; "canonical reconciles known binding")]
#[test_case("STATEVEC_BANK_WRITER_EXTENDED_JOURNAL", FirstRequest::Reconcile; "extended reconciles known binding")]
#[test_case("STATEVEC_BANK_WRITER_JOURNAL", FirstRequest::Submit; "canonical submits without binding")]
#[test_case("STATEVEC_BANK_WRITER_EXTENDED_JOURNAL", FirstRequest::Submit; "extended submits without binding")]
#[ignore = "requires runtime release-1 corpus paths from just check-release-corpus"]
fn recover_frozen_journal(sample: &str, expected_request: FirstRequest) {
    let journal_bytes = fs::read(sample_path(sample)).expect("read frozen journal");
    let saved: Journal = serde_json::from_slice(&journal_bytes).expect("decode frozen journal");
    let original = saved.pending.as_ref().expect("fixture has a pending deposit");
    let entry = frozen_entry();
    assert_eq!(original.committed.as_ref().map(SavedCommit::binding), Some(entry));

    let directory = tempfile::tempdir().unwrap();
    let journal_path = directory.path().join("writer.json");
    if expected_request == FirstRequest::Submit {
        let mut control: serde_json::Value = serde_json::from_slice(&journal_bytes).unwrap();
        *control.pointer_mut("/pending/committed").unwrap() = serde_json::Value::Null;
        fs::write(&journal_path, serde_json::to_vec(&control).unwrap()).unwrap();
    } else {
        fs::write(&journal_path, &journal_bytes).unwrap();
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let deployment_path = directory.path().join("deployment.json");
    let deployment = serde_json::json!({
        "identity": saved.identity,
        "endpoints": [[1, listener.local_addr().unwrap()]],
        "raft_tick_millis": 15,
        "max_outcome_bytes": 256,
    });
    fs::write(&deployment_path, serde_json::to_vec(&deployment).unwrap()).unwrap();
    let args = [
        "recover".to_owned(),
        deployment_path.display().to_string(),
        directory.path().display().to_string(),
    ];

    let mut requested_acknowledgement = false;
    let (result, request) = thread::scope(|scope| {
        let endpoint = scope.spawn(move || answer_recovery(listener, entry));
        let result = run(&args, |_| {
            requested_acknowledgement = true;
            Ok(0)
        });
        (result, endpoint.join().unwrap())
    });

    let observed_request = match request {
        Request::Reconcile { .. } => FirstRequest::Reconcile,
        Request::Submit { .. } => FirstRequest::Submit,
    };
    assert_eq!(observed_request, expected_request, "first request for {sample}");
    let mut commitment = String::new();
    for byte in request.batch().commitment_v1().encode_v1() {
        write!(commitment, "{byte:02x}").unwrap();
    }
    let expected_commitment = fs::read_to_string(sample_path("STATEVEC_BANK_WRITER_COMMITMENT")).unwrap();
    assert_eq!(commitment, expected_commitment.trim(), "recovered wire intent for {sample}");

    assert!(requested_acknowledgement, "Completed reply did not reach the upstream boundary: {result:?}");
    assert!(result.is_err(), "EOF supplies no upstream acknowledgement");
    let recovered_bytes = fs::read(&journal_path).unwrap();
    let recovered: Journal = serde_json::from_slice(&recovered_bytes).unwrap();
    assert_eq!(recovered.cursor, saved.cursor);
    let pending = recovered.pending.expect("without acknowledgement the original remains pending");
    assert_eq!(pending.first, original.first);
    assert_eq!(pending.command, original.command);
    assert_eq!(pending.committed.as_ref().map(SavedCommit::binding), Some(entry));
    if expected_request == FirstRequest::Reconcile {
        assert_eq!(recovered_bytes, journal_bytes, "the saved binding needs no journal rewrite");
    }
}

#[test_case(FirstRequest::Submit; "prior result uncertain")]
#[test_case(FirstRequest::Reconcile; "commit binding already known")]
fn suspended_writer_stops_and_retains_its_journal(first: FirstRequest) {
    let directory = tempfile::tempdir().unwrap();
    let identity = Identity {
        cluster_identity: "01".repeat(32),
        genesis_identity: "02".repeat(32),
        execution_lineage: "03".repeat(32),
    };
    let mut original = deposit(1, 128, 255).unwrap();
    original.committed = (first == FirstRequest::Reconcile).then(|| frozen_entry().into());
    Journal {
        identity: identity.clone(),
        cursor: 0,
        pending: Some(original),
        last_reconciliation: None,
    }
    .save(directory.path())
    .unwrap();
    let journal_path = directory.path().join("writer.json");
    let original_journal = fs::read(&journal_path).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let config = directory.path().join("deployment.json");
    let deployment = serde_json::json!({
        "identity": identity,
        "endpoints": [[1, listener.local_addr().unwrap()]],
        "raft_tick_millis": 15,
        "max_outcome_bytes": 256,
    });
    fs::write(&config, serde_json::to_vec(&deployment).unwrap()).unwrap();
    let args = ["recover".to_owned(), config.display().to_string(), directory.path().display().to_string()];

    let result = thread::scope(|scope| {
        let endpoint = scope.spawn(move || {
            let mut peer = accept_peer(listener);
            let request = read_recovery_request(&mut peer);
            assert_eq!(matches!(request, Request::Reconcile { .. }), first == FirstRequest::Reconcile);
            let refusal = Reply {
                binding: ReplyBinding::for_request(&request),
                observation: None,
                disposition: ReplyDisposition::Refused(BatchSubmitRefusal::Unrecognized { observed: u8::MAX }),
            };
            peer.write_all(&encode_reply(&refusal, FRAME_BYTES as usize, COMMANDS_PER_BATCH as usize).unwrap())
                .expect("send the correlated future refusal");
            assert_eq!(
                peer.read(&mut [0]).expect("writer must stop within the peer timeout"),
                0,
                "the suspended writer closes its socket without another request"
            );
        });
        let result = run(&args, |_| panic!("a suspended writer cannot acknowledge an outcome"));
        endpoint.join().unwrap();
        result
    });

    let failure = result.expect_err("the suspended writer must exit with an error");
    assert!(matches!(
        failure.get_ref().and_then(|cause| cause.downcast_ref::<SessionError>()),
        Some(SessionError(cause)) if matches!(**cause, ClientError::Stopped)
    ));
    assert_eq!(fs::read(&journal_path).unwrap(), original_journal);
}

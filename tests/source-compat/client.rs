use sdk_source_consumer::schema::Ping;
use statevec::{CommandSchema, GeneratedCommandAccess};
use statevec_ingress_client::{
    BatchClient, BatchClientSettings, BatchDriverWorker, BatchEvent, BatchRetryConfig, BatchTcpWorker, BatchTerminal,
    ClientError, ClientStreamBatch, Command, DeploymentIdentity, Digest32, InputRef, ReadAnswer, ReadRecord,
    ReadRefusal, RouteDiscoveryCounts, RouteObservation, batch_monotonic_now, encode_runtime_binary_command_payload,
};
use std::num::NonZeroU128;

fn main() {
    const COMMANDS_PER_BATCH: u32 = 1;
    const FRAME_BYTES: u32 = 4096;
    const OUTCOME_BYTES: u32 = 512;
    const UNAVAILABLE_REPLIES_PER_PEER: u32 = 3;

    let first = InputRef { client_id: 1, stream_id: 1, client_seq: 1, request_id: None };
    let payload = Ping::builder().set_value(42).build().expect("valid ping");
    let command = encode_runtime_binary_command_payload(&Command::new(Ping::KIND, 1, 0, payload)).expect("encode ping");
    let batch =
        ClientStreamBatch::try_new(first.clone(), vec![command], FRAME_BYTES as usize, COMMANDS_PER_BATCH as usize)
            .expect("one command fits");
    let mut retry = BatchRetryConfig::from(100);
    assert_eq!(retry.routing_poll_millis, 150);
    assert!((50..=500).contains(&retry.routing_poll_millis));
    retry.routing_poll_millis = 50;
    let mut client = BatchClient::bounded(
        BatchClientSettings::new(
            DeploymentIdentity::new(Digest32::new([1; 32]), Digest32::new([2; 32]), Digest32::new([3; 32])),
            NonZeroU128::MIN,
            first,
            vec![(1, ([127, 0, 0, 1], 7000).into())],
            COMMANDS_PER_BATCH,
            FRAME_BYTES,
            OUTCOME_BYTES,
            UNAVAILABLE_REPLIES_PER_PEER,
        )
        .with_retry(retry)
        .with_operation_millis(5000)
        .with_lifetime_millis(120000),
        batch_monotonic_now,
    )
    .expect("valid client settings");
    let observation: Option<&RouteObservation> = client.route_observation();
    assert!(observation.is_none());
    let discovery: RouteDiscoveryCounts = client.route_discovery();
    assert_eq!((discovery.visits, discovery.successors), (0, 0));
    let returned = client.try_submit(batch).expect_err("the client has no connection yet");
    assert!(matches!(returned.reason, ClientError::NotConnected));
    if let Some(BatchEvent::Terminal(terminal)) = client.poll_event() {
        let label = match terminal {
            BatchTerminal::Applied { .. } => "applied",
            BatchTerminal::ExactAppliedResultUnavailable { .. } => "applied without member results",
            BatchTerminal::NotAdmitted { .. } => "not admitted",
            BatchTerminal::OutcomeUnknown { .. } => "outcome unknown",
        };
        panic!("a locally refused submit published a terminal: {label}");
    }

    let record = ReadRecord::new(1, 7, 2, vec![42]);
    let answer = ReadAnswer::new(9, Digest32::new([4; 32]), vec![Some(record)]);
    let ReadAnswer { applied_tx_seq, records, .. } = answer;
    assert_eq!(applied_tx_seq, 9);
    assert!(matches!(records.as_slice(), [Some(ReadRecord { sys_id: 7, .. })]));

    let refusal = ReadRefusal::Unrecognized { observed: u8::MAX, argument: 7 };
    let diagnostic = match refusal {
        ReadRefusal::Unrecognized { observed, argument } => Some((observed, argument)),
        _ => None,
    };
    assert_eq!(diagnostic, Some((u8::MAX, 7)));

    let mut driver = BatchDriverWorker::new(BatchTcpWorker::new().expect("TCP readiness port"));
    let _ = driver.step(&mut client).expect("drive the batch session");
}

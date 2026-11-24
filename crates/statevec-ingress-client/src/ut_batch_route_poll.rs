use super::*;
use ::test_case::test_case;
use ingress_api::stream_batch::{
    FrameReader, ReadProgress, RouteObservation, StreamDirection, StreamFrame, decode_route_request, encode_route_reply,
};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

fn routing_client(clock: &Clock) -> BatchClient {
    let peers = (1..=3).map(|peer| (peer, SocketAddr::from(([127, 0, 0, 1], 12000 + peer as u16)))).collect();
    construct(clock, idle_start(), peers, (RETRY, 800, 3000)).unwrap()
}

fn connected(client: &mut BatchClient, peer: u64) -> u64 {
    let ClientResult::Connect { operation_id, peer_id, .. } = client.drive(ClientEvent::Drive) else {
        panic!("probe must dial peer {peer}");
    };
    assert_eq!(peer_id, peer);
    assert!(matches!(
        client.drive(ClientEvent::Connected { operation_id, result: Ok(()) }),
        ClientResult::Waiting { .. }
    ));
    operation_id
}

fn probe(client: &mut BatchClient, socket: u64) -> RequestContext {
    let ClientResult::Write { operation_id, request_id, bytes } = client.drive(ClientEvent::Drive) else {
        panic!("expected route probe on {socket}");
    };
    assert_eq!(operation_id, socket);
    let context = decode_route_request(&bytes, FRAME_LIMIT as usize).unwrap();
    assert_eq!(request_id, context.request_id);
    assert!(matches!(
        client.drive(ClientEvent::Written { operation_id, request_id, result: Ok(()) }),
        ClientResult::Waiting { .. }
    ));
    context
}

fn route_reply(client: &mut BatchClient, socket: u64, context: RequestContext, observation: RouteObservation) -> ClientResult {
    let bytes = encode_route_reply(&context, &observation, FRAME_LIMIT as usize).unwrap();
    client.drive(ClientEvent::Read { operation_id: socket, result: Ok(bytes) })
}

fn observed_route(peer_id: u64, leader: Option<u64>, writable: bool) -> RouteObservation {
    let mut observation = RouteObservation::new(NonZeroU64::new(peer_id).unwrap());
    observation.term = 4;
    observation.leader = leader.and_then(NonZeroU64::new);
    observation.applied_index = 25;
    observation.writable = writable;
    observation.voters = vec![2, 3];
    observation.outgoing_voters = vec![1, 2, 3];
    observation.learners = vec![4];
    observation.next_learners = vec![1];
    observation
}

#[test]
fn a_slow_reply_settles_on_the_original_connection_during_discovery() {
    let clock = Clock::new();
    let mut client = routing_client(&clock);
    let leader = connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (_, original) = take_write(&mut client);
    write_return(&mut client, leader, &original);

    let interval = crate::ROUTING_POLL_DEFAULT_MS;
    clock.set(INITIAL_TIME + interval);
    let discovery = client.drive(ClientEvent::Drive);
    let ClientResult::Connect { operation_id: follower, peer_id: 2, .. } = discovery else {
        panic!("silence must start discovery while keeping the original route: {discovery:?}");
    };
    assert!(matches!(
        client.drive(ClientEvent::Connected { operation_id: follower, result: Ok(()) }),
        ClientResult::Waiting { .. }
    ));
    probe(&mut client, follower);

    clock.set(INITIAL_TIME + interval * 3 / 2);
    let bytes = stream_batch::encode_reply(&completed(&original), FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    assert!(matches!(
        client.drive(ClientEvent::Read { operation_id: leader, result: Ok(bytes) }),
        ClientResult::Close { operation_id } if operation_id == follower
    ));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { .. }))));
    assert!(matches!(
        client.drive(ClientEvent::Closed { operation_id: follower }),
        ClientResult::Waiting { wake_at: None }
    ));
    assert!(matches!(client.status(), ClientStatus::Batch { occupied: 0, connected: true, .. }));
    assert_eq!(client.route_discovery(), crate::RouteDiscoveryCounts { visits: 1, successors: 0 });
}

#[test]
fn original_tcp_reply_wakes_the_driver_while_the_probe_socket_is_silent() {
    let clock = Clock::new();
    let listeners = [TcpListener::bind("127.0.0.1:0").unwrap(), TcpListener::bind("127.0.0.1:0").unwrap()];
    let endpoints = listeners.iter().enumerate().map(|(index, listener)| {
        listener.set_nonblocking(true).unwrap();
        ((index + 1) as u64, listener.local_addr().unwrap())
    }).collect();
    let mut client = construct(&clock, idle_start(), endpoints, (RETRY, 800, 3000)).unwrap();
    let mut driver = BatchDriverWorker::new(BatchTcpWorker::new().unwrap());
    let mut leader = accept_tcp(&mut driver, &mut client, &listeners[0]);
    drive_tcp_until(&mut driver, &mut client, |client| {
        matches!(client.status(), ClientStatus::Batch { connected: true, .. }).then_some(())
    });
    client.try_submit(batch(1)).unwrap();
    let StreamFrame::Request(original @ Request::Submit { .. }) = read_tcp_request(&mut driver, &mut client, &mut leader)
    else { panic!("exact original Submit on the first socket") };
    drive_tcp_until(&mut driver, &mut client, |client| {
        matches!(client.status(), ClientStatus::Batch { next_deadline: Some(at), .. }
            if at.get() == INITIAL_TIME + crate::ROUTING_POLL_DEFAULT_MS).then_some(())
    });

    clock.set(INITIAL_TIME + crate::ROUTING_POLL_DEFAULT_MS);
    let mut scout = accept_tcp(&mut driver, &mut client, &listeners[1]);
    assert!(matches!(read_tcp_request(&mut driver, &mut client, &mut scout), StreamFrame::Route(_)));
    assert!(matches!(driver.step(&mut client).unwrap(), BatchDriverStep::Waiting));
    clock.set(INITIAL_TIME + 225);
    let reply = stream_batch::encode_reply(&completed(&original), FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    leader.write_all(&reply).unwrap();
    driver.wait(&mut client, Duration::ZERO).unwrap();
    let terminal = drive_tcp_until(&mut driver, &mut client, BatchClient::poll_event);
    assert!(matches!(terminal, BatchEvent::Terminal(BatchTerminal::Applied { handle, .. })
        if handle.intent == original.batch().commitment_v1()));

    drive_tcp_until(&mut driver, &mut client, |_| match scout.read(&mut [0]) {
        Ok(0) => Some(()),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => None,
        other => panic!("the unneeded probe must retire without exchanging another frame: {other:?}"),
    });
    assert!(matches!(leader.read(&mut [0]), Err(error) if error.kind() == io::ErrorKind::WouldBlock));
}

fn drive_tcp_until<T>(
    driver: &mut BatchDriverWorker<BatchTcpWorker>,
    client: &mut BatchClient,
    mut observe: impl FnMut(&mut BatchClient) -> Option<T>,
) -> T {
    let harness_deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(observed) = observe(client) {
            return observed;
        }
        assert!(Instant::now() < harness_deadline, "loopback driver stalled: {:?}", client.status());
        match driver.step(client).unwrap() {
            BatchDriverStep::Waiting => { driver.wait(client, Duration::from_millis(5)).unwrap(); }
            BatchDriverStep::Runnable => (),
            BatchDriverStep::Diagnostic { cause } => panic!("unexpected transport failure: {cause:?}"),
        }
    }
}

fn accept_tcp(
    driver: &mut BatchDriverWorker<BatchTcpWorker>, client: &mut BatchClient, listener: &TcpListener,
) -> TcpStream {
    drive_tcp_until(driver, client, |_| match listener.accept() {
        Ok((stream, _)) => {
            stream.set_nonblocking(true).unwrap();
            Some(stream)
        }
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => None,
        Err(error) => panic!("accept loopback stream: {error}"),
    })
}

fn read_tcp_request(
    driver: &mut BatchDriverWorker<BatchTcpWorker>, client: &mut BatchClient, socket: &mut TcpStream,
) -> StreamFrame {
    let mut reader = FrameReader::new(StreamDirection::Requests, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    drive_tcp_until(driver, client, |_| match reader.poll_read(socket).unwrap() {
        ReadProgress::Frame(frame) => Some(frame),
        ReadProgress::Runnable | ReadProgress::WouldBlock => None,
        ReadProgress::Eof => panic!("client retired before the peer got its request"),
    })
}

#[test]
fn silent_submit_discovers_the_new_leader_and_reconciles_without_another_interval() {
    let clock = Clock::new();
    let mut client = routing_client(&clock);
    let original_socket = connect(&mut client);
    let intent = batch(1);
    client.try_submit(intent).unwrap();
    let (_, original) = take_write(&mut client);
    write_return(&mut client, original_socket, &original);

    clock.set(INITIAL_TIME + crate::ROUTING_POLL_DEFAULT_MS - 1);
    assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { .. }));
    clock.set(INITIAL_TIME + crate::ROUTING_POLL_DEFAULT_MS);
    let follower = connected(&mut client, 2);
    let context = probe(&mut client, follower);
    let wrong_peer = route_reply(&mut client, follower, context, observed_route(1, Some(3), false));
    assert!(matches!(wrong_peer, ClientResult::Waiting { .. }));
    assert!(client.route_observation().is_none());
    assert!(matches!(
        route_reply(&mut client, follower, context, observed_route(2, Some(3), false)),
        ClientResult::Close { operation_id } if operation_id == follower
    ));
    assert!(matches!(
        client.drive(ClientEvent::Closed { operation_id: follower }),
        ClientResult::Waiting { .. }
    ));
    let successor = connected(&mut client, 3);
    let context = probe(&mut client, successor);
    assert!(matches!(
        route_reply(&mut client, successor, context, observed_route(3, Some(3), true)),
        ClientResult::Close { operation_id } if operation_id == original_socket
    ));
    assert_eq!(client.route_discovery(), crate::RouteDiscoveryCounts { visits: 2, successors: 1 });
    assert!(matches!(
        client.drive(ClientEvent::Closed { operation_id: original_socket }),
        ClientResult::Waiting { .. }
    ));
    let (socket, reconcile) = take_query(&mut client);
    assert_eq!(socket, successor);
    assert_eq!(wire(reconcile.batch()), wire(original.batch()));
    write_return(&mut client, socket, &reconcile);
    let mut retryable = reply(&reconcile, ReplyDisposition::RetryableNext);
    retryable.observation = Some(ReplyObservation::new(4, 25, 4, NonZeroU64::new(3), None).unwrap());
    receive(&mut client, socket, retryable);
    let (_, retried) = take_write(&mut client);
    assert_eq!(wire(retried.batch()), wire(original.batch()));
    assert_eq!(client.route_observation().unwrap().next_learners, vec![1]);
    assert_eq!(clock.sample().unwrap().get(), INITIAL_TIME + crate::ROUTING_POLL_DEFAULT_MS);
}

#[test_case(None; "no_successor_observed")]
#[test_case(Some(2); "last_follower_names_a_new_leader_after_a_split_vote")]
fn unready_cluster_paces_probes_then_waits_for_the_original_exchange_deadline(last_leader: Option<u64>) {
    let clock = Clock::new();
    let mut client = routing_client(&clock);
    let original_socket = connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (_, original) = take_write(&mut client);
    write_return(&mut client, original_socket, &original);
    let interval = crate::ROUTING_POLL_DEFAULT_MS;
    clock.set(INITIAL_TIME + interval);

    for (index, peer) in [2, 3, 2, 3].into_iter().enumerate() {
        let socket = connected(&mut client, peer);
        let context = probe(&mut client, socket);
        let (term, leader) = match index {
            0 => (35, Some(1)),
            3 if last_leader.is_some() => (37, last_leader),
            _ => (36, None),
        };
        let mut observation = observed_route(peer, leader, false);
        observation.term = term;
        observation.voters = vec![1, 2, 3];
        observation.outgoing_voters.clear();
        observation.next_learners.clear();
        assert!(matches!(
            route_reply(&mut client, socket, context, observation),
            ClientResult::Close { .. }
        ));
        assert!(matches!(
            client.drive(ClientEvent::Closed { operation_id: socket }),
            ClientResult::Waiting { .. }
        ));
        let next = INITIAL_TIME + (index as u64 + 2) * interval;
        clock.set(next - 1);
        assert!(matches!(client.drive(ClientEvent::Drive), ClientResult::Waiting { .. }));
        clock.set(next);
    }
    assert_eq!(client.route_discovery(), crate::RouteDiscoveryCounts { visits: 4, successors: 0 });
    let last = client.route_observation().expect("the fourth peer answered");
    assert_eq!(last.peer_id.get(), 3);
    assert_eq!(last.term, if last_leader.is_some() { 37 } else { 36 });
    assert_eq!(last.leader.map(NonZeroU64::get), last_leader);
    assert!(matches!(
        client.drive(ClientEvent::Drive),
        ClientResult::Waiting { wake_at: Some(deadline) } if deadline.get() == INITIAL_TIME + 800
    ));
    clock.set(INITIAL_TIME + 800);
    assert!(matches!(
        client.drive(ClientEvent::Drive), ClientResult::Close { operation_id } if operation_id == original_socket
    ));
    assert!(matches!(
        client.drive(ClientEvent::Closed { operation_id: original_socket }),
        ClientResult::Waiting { .. }
    ));
    clock.set(INITIAL_TIME + 800 + RETRY);
    let fallback = connected(&mut client, 2);
    let (socket, retry) = take_write(&mut client);
    assert_eq!(socket, fallback);
    assert_eq!(wire(retry.batch()), wire(original.batch()));
}

#[test_case(false; "before_discovery")]
#[test_case(true; "during_discovery")]
fn a_partial_reply_defers_to_the_original_exchange_deadline(discovering: bool) {
    let clock = Clock::new();
    let mut client = routing_client(&clock);
    let socket = connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (_, request) = take_write(&mut client);
    write_return(&mut client, socket, &request);
    let bytes = stream_batch::encode_reply(&completed(&request), FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    let mut reader = FrameReader::new(StreamDirection::Replies, FRAME_LIMIT as usize, COMMAND_LIMIT as usize).unwrap();
    assert!(matches!(reader.poll_frame(&mut &bytes[..1]).unwrap(), ReadProgress::Runnable));
    let cursor = reader.incomplete_cursor().unwrap();
    clock.set(INITIAL_TIME + crate::ROUTING_POLL_DEFAULT_MS);
    let scout = discovering.then(|| {
        let scout = connected(&mut client, 2);
        probe(&mut client, scout);
        scout
    });
    let progress = client.drive(ClientEvent::PartialReply { operation_id: socket, written: None, cursor });
    let waiting = match scout {
        Some(scout) => {
            assert!(matches!(progress, ClientResult::Close { operation_id } if operation_id == scout));
            client.drive(ClientEvent::Closed { operation_id: scout })
        }
        None => progress,
    };
    assert!(matches!(
        waiting,
        ClientResult::Waiting { wake_at: Some(deadline) } if deadline.get() == INITIAL_TIME + 800
    ));
    receive(&mut client, socket, completed(&request));
    assert!(matches!(client.poll_event(), Some(BatchEvent::Terminal(BatchTerminal::Applied { .. }))));
}

#[test]
fn the_original_exchange_deadline_wins_over_an_in_flight_probe_reply() {
    let clock = Clock::new();
    let mut client = routing_client(&clock);
    let leader = connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (_, original) = take_write(&mut client);
    write_return(&mut client, leader, &original);
    clock.set(INITIAL_TIME + 799);
    let scout = connected(&mut client, 2);
    let context = probe(&mut client, scout);

    clock.set(INITIAL_TIME + 800);
    assert!(matches!(
        route_reply(&mut client, scout, context, observed_route(2, Some(2), true)),
        ClientResult::Close { operation_id } if operation_id == scout
    ));
    assert!(matches!(
        client.drive(ClientEvent::Closed { operation_id: scout }),
        ClientResult::Close { operation_id } if operation_id == leader
    ));
    assert!(matches!(
        client.drive(ClientEvent::Closed { operation_id: leader }),
        ClientResult::Waiting { wake_at: Some(deadline) } if deadline.get() == INITIAL_TIME + 800 + RETRY
    ));
    assert!(client.route_observation().is_none());
}

#[test]
fn stopping_during_discovery_retires_both_connections_and_returns_the_original() {
    let clock = Clock::new();
    let mut client = routing_client(&clock);
    let leader = connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (_, original) = take_write(&mut client);
    write_return(&mut client, leader, &original);
    clock.set(INITIAL_TIME + crate::ROUTING_POLL_DEFAULT_MS);
    let scout = connected(&mut client, 2);
    probe(&mut client, scout);

    assert!(matches!(
        client.drive(ClientEvent::Stop), ClientResult::Close { operation_id } if operation_id == scout
    ));
    assert!(matches!(
        client.drive(ClientEvent::Closed { operation_id: scout }),
        ClientResult::Close { operation_id } if operation_id == leader
    ));
    assert!(matches!(
        client.drive(ClientEvent::Closed { operation_id: leader }),
        ClientResult::Waiting { wake_at: None }
    ));
    let Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { batch, cause: ClientError::Stopped, .. })) =
        client.poll_event() else { panic!("stop returns custody of the unresolved batch exactly once") };
    assert_eq!(wire(&batch), wire(original.batch()));
    assert!(client.poll_event().is_none());
}

#[test]
fn stop_during_successor_selection_waits_for_the_issued_close() {
    let clock = Clock::new();
    let mut client = routing_client(&clock);
    let leader = connect(&mut client);
    client.try_submit(batch(1)).unwrap();
    let (_, original) = take_write(&mut client);
    write_return(&mut client, leader, &original);
    clock.set(INITIAL_TIME + crate::ROUTING_POLL_DEFAULT_MS);
    let scout = connected(&mut client, 2);
    let context = probe(&mut client, scout);

    assert!(matches!(
        route_reply(&mut client, scout, context, observed_route(2, Some(2), true)),
        ClientResult::Close { operation_id } if operation_id == leader
    ));
    let stopped = client.drive(ClientEvent::Stop);
    assert!(matches!(stopped, ClientResult::Waiting { wake_at: None }),
        "a second Close would overwrite the driver's unconsumed primary return: {stopped:?}");
    assert!(matches!(
        client.drive(ClientEvent::Closed { operation_id: leader }),
        ClientResult::Close { operation_id } if operation_id == scout
    ));
    assert!(matches!(
        client.drive(ClientEvent::Closed { operation_id: scout }),
        ClientResult::Waiting { wake_at: None }
    ));
    assert!(matches!(client.status(), ClientStatus::Batch { stopped: true, .. }));
}

#[test_case(49, false; "below_minimum")]
#[test_case(50, true; "minimum")]
#[test_case(150, true; "default")]
#[test_case(500, true; "maximum")]
#[test_case(501, false; "above_maximum")]
fn validity_of_poll_interval(interval: u64, accepted: bool) {
    let mut retry = crate::BatchRetryConfig::from(RETRY);
    assert_eq!(retry.routing_poll_millis, 150);
    retry.routing_poll_millis = interval;
    assert_eq!(construct(&Clock::new(), idle_start(), endpoints(), (retry, 800, 3000)).is_ok(), accepted);
}

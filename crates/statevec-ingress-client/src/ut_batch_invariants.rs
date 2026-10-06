//! Challenge surviving value relations, not removed query-permit machinery.
use super::*;
use test_case::test_case;

#[derive(Clone, Copy)]
enum CorrelationFault {
    UncommittedQuery,
    FutureSubmitId,
    FalseCommittedNotice,
}

#[test_case(CorrelationFault::UncommittedQuery; "uncommitted query")]
#[test_case(CorrelationFault::FutureSubmitId; "unallocated submit id")]
#[test_case(CorrelationFault::FalseCommittedNotice; "false committed notice")]
fn inconsistent_correlation_cannot_publish_a_wire_effect(fault: CorrelationFault) {
    let clock = Clock::new();
    let mut client = client(&clock);
    let (connection, request) = sent(&mut client);
    let ClientState::Batch { slot, context, .. } = &mut client.state;
    let Some(BatchSlot::Pending { handle, attempt, submit_route, notice, .. }) = slot else { unreachable!() };
    match fault {
        CorrelationFault::UncommittedQuery => {
            *attempt = BatchAttempt::Querying {
                operation_id: connection,
                request_id: handle.context.request_id,
                deadline: MonotonicMillis::new(INITIAL_TIME + OPERATION),
            }
        }
        CorrelationFault::FutureSubmitId => *submit_route = Some((connection, context.request_id, context.request_id)),
        CorrelationFault::FalseCommittedNotice => {
            *notice = Some(BatchProgress::CommittedAwaitingApply { handle: *handle, entry: entry() })
        }
    }
    let result = client.drive(ClientEvent::Drive);
    assert!(
        matches!(result, ClientResult::DriveRejected { cause: ClientFailure::BatchReservationInvariant, .. }),
        "inconsistent correlation must fail closed before any wire effect: {result:?}"
    );
    assert!(matches!(client.drive(ClientEvent::Stop), ClientResult::Close { .. }));
    let Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { batch, committed, .. })) = client.poll_event() else {
        panic!("original intent stays drainable after invariant rejection")
    };
    assert_eq!(wire(&batch), wire(request.batch()));
    assert_eq!(committed, None);
}

// Copyright 2026 Jumpex Technology.
// SPDX-License-Identifier: Apache-2.0

use std::collections::VecDeque;
use std::convert::Infallible;

use statevec_api::{
    COMMITTED_RESULT_RECORD_VERSION, CommittedResultQuery, CommittedResultRecord, CommittedStatus,
    QueueConsumer, QueueConsumerResume, QueueProducer, QueueRecord, RejectedErrorCode,
    SUBMIT_REQUEST_RECORD_VERSION, SubmitRequest, decode_committed_result_record,
    decode_submit_request, encode_committed_result_record, encode_submit_request,
};

#[derive(Default)]
struct TestQueue {
    next_ext_seq: u64,
    committed_through: Option<u64>,
    records: VecDeque<(u64, Vec<u8>)>,
}

impl QueueProducer for TestQueue {
    type Error = Infallible;

    fn append(&mut self, record: &[u8]) -> Result<u64, Self::Error> {
        let ext_seq = self.next_ext_seq.max(1);
        self.next_ext_seq = ext_seq + 1;
        self.records.push_back((ext_seq, record.to_vec()));
        Ok(ext_seq)
    }
}

impl QueueConsumer for TestQueue {
    type Record = Vec<u8>;
    type Error = Infallible;

    fn poll(&mut self) -> Result<Option<QueueRecord<Self::Record>>, Self::Error> {
        Ok(self.records.front().map(|(ext_seq, record)| QueueRecord {
            ext_seq: *ext_seq,
            ref_ext_time_us: *ext_seq,
            record: record.clone(),
        }))
    }

    fn commit_through(&mut self, ext_seq: u64) -> Result<(), Self::Error> {
        self.committed_through = Some(
            self.committed_through
                .map_or(ext_seq, |cur| cur.max(ext_seq)),
        );
        while self
            .records
            .front()
            .is_some_and(|(queued_ext_seq, _)| *queued_ext_seq <= ext_seq)
        {
            self.records.pop_front();
        }
        Ok(())
    }
}

impl QueueConsumerResume for TestQueue {
    type Error = Infallible;

    fn resume_next_ext_seq(&mut self) -> Result<Option<u64>, Self::Error> {
        Ok(self.committed_through.map(|ext_seq| ext_seq + 1))
    }
}

#[test]
// CE-01..CE-06: queue / command encoding boundaries.
fn queue_traits_can_model_append_poll_commit() {
    let mut queue = TestQueue::default();

    assert_eq!(queue.append(b"a").unwrap(), 1);
    assert_eq!(queue.append(b"b").unwrap(), 2);

    assert_eq!(
        queue.poll().unwrap(),
        Some(QueueRecord {
            ext_seq: 1,
            ref_ext_time_us: 1,
            record: b"a".to_vec()
        })
    );
    queue.commit_through(1).unwrap();
    assert_eq!(
        queue.poll().unwrap(),
        Some(QueueRecord {
            ext_seq: 2,
            ref_ext_time_us: 2,
            record: b"b".to_vec()
        })
    );
    queue.commit_through(2).unwrap();
    assert_eq!(queue.poll().unwrap(), None);
}

#[test]
// CE-01..CE-06: queue / command encoding boundaries.
fn queue_consumer_resume_reports_next_ext_seq() {
    let mut queue = TestQueue::default();
    assert_eq!(queue.resume_next_ext_seq().unwrap(), None);
    assert_eq!(queue.append(b"a").unwrap(), 1);
    queue.commit_through(1).unwrap();
    assert_eq!(queue.resume_next_ext_seq().unwrap(), Some(2));
}

#[test]
// CE-01..CE-06: queue / command encoding boundaries.
fn submit_request_roundtrip() {
    let request = SubmitRequest::new(9, b"hello".to_vec());
    let encoded = encode_submit_request(&request).unwrap();
    let decoded = decode_submit_request(&encoded).unwrap();
    assert_eq!(decoded, request);
}

#[test]
// CE-01..CE-06: queue / command encoding boundaries.
fn committed_result_record_roundtrip() {
    let record = CommittedResultRecord {
        ext_seq: 7,
        status: CommittedStatus::Committed { tx_seq: 11 },
    };
    let encoded = encode_committed_result_record(record);
    let decoded = decode_committed_result_record(&encoded).unwrap();
    assert_eq!(decoded, record);
}

#[test]
// CE-01..CE-06: queue / command encoding boundaries.
fn rejected_result_record_roundtrip() {
    let record = CommittedResultRecord {
        ext_seq: 8,
        status: CommittedStatus::Rejected {
            error_code: RejectedErrorCode::CommandRejected,
        },
    };
    let encoded = encode_committed_result_record(record);
    let decoded = decode_committed_result_record(&encoded).unwrap();
    assert_eq!(decoded, record);
}

#[test]
// CE-01..CE-06: queue / command encoding boundaries.
fn submit_request_encoding_uses_v1_header() {
    let encoded = encode_submit_request(&SubmitRequest::new(9, b"hello".to_vec())).unwrap();
    assert_eq!(encoded[0], SUBMIT_REQUEST_RECORD_VERSION);
    assert_eq!(encoded[1], 9);
}

#[test]
// CE-01..CE-06: queue / command encoding boundaries.
fn committed_result_record_encoding_uses_v1_header() {
    let encoded = encode_committed_result_record(CommittedResultRecord {
        ext_seq: 1,
        status: CommittedStatus::Committed { tx_seq: 2 },
    });
    assert_eq!(encoded[0], COMMITTED_RESULT_RECORD_VERSION);
}

#[test]
// CE-01..CE-06: queue / command encoding boundaries.
fn committed_result_query_closure_can_be_used_directly() {
    let mut query = |ext_seq| {
        Ok::<_, Infallible>(Some(CommittedStatus::Committed {
            tx_seq: ext_seq + 10,
        }))
    };
    assert_eq!(
        query.query_committed_by_ext_seq(5).unwrap(),
        Some(CommittedStatus::Committed { tx_seq: 15 })
    );
}

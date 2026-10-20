use super::*;
use std::cell::Cell;
use test_case::test_case;

#[derive(Clone, Copy)]
enum Fault {
    None,
    InvalidOperation,
    UnexpectedBefore,
    MissingAfter,
    Count,
    NullRead,
    VisitorPanic,
}
struct Host {
    fault: Fault,
    before_reads: Cell<usize>,
    after_reads: Cell<usize>,
}
unsafe fn host<'a>(raw: *const c_void) -> &'a Host {
    unsafe { &*raw.cast::<Host>() }
}
unsafe extern "C" fn time(_: *const c_void, out: *mut u64) -> SvStatus {
    unsafe { out.write(0) };
    SvStatus::SUCCESS
}
unsafe extern "C" fn read(
    raw: *const c_void,
    view: u32,
    key: SvKey,
    visitor: SvReadVisitor,
    context: *mut c_void,
    found: *mut u32,
) -> SvStatus {
    let host = unsafe { host(raw) };
    let count = if view == 0 {
        &host.before_reads
    } else {
        &host.after_reads
    };
    count.set(count.get() + 1);
    if key.sys_id != 10 {
        unsafe { found.write(0) };
        return SvStatus::SUCCESS;
    }
    unsafe { found.write(1) };
    let bytes = if matches!(host.fault, Fault::NullRead) {
        SvBytes {
            ptr: std::ptr::null(),
            len: 1,
        }
    } else {
        SvBytes::borrowed(if view == 0 { &[3] } else { &[5] })
    };
    unsafe { visitor.unwrap()(context, bytes) }
}
unsafe extern "C" fn resolve(
    _: *const c_void,
    view: u32,
    kind: u16,
    unique_key_id: u8,
    key: SvBytes,
    found: *mut u32,
    id: *mut u64,
) -> SvStatus {
    assert_eq!((kind, unique_key_id), (7, 2));
    assert_eq!(unsafe { bytes(key).unwrap() }, &[42]);
    unsafe {
        found.write(1);
        id.write(10 + u64::from(view))
    };
    SvStatus::SUCCESS
}
unsafe extern "C" fn changes(
    raw: *const c_void,
    after_only: u32,
    visit: SvChangeVisitor,
    context: *mut c_void,
    count: *mut u64,
) -> SvStatus {
    let host = unsafe { host(raw) };
    unsafe { count.write(0) };
    for operation in 0..3 {
        let mut change = SvCandidateChange {
            key: wire_key(RecordKey {
                kind: 7,
                sys_id: operation.into(),
            }),
            operation,
            has_before: u32::from(after_only == 0 && operation != 0),
            has_after: u32::from(operation != 2),
            reserved: 0,
            before: if after_only == 0 && operation != 0 {
                SvBytes::borrowed(&[3])
            } else {
                SvBytes::EMPTY
            },
            // A present, zero-length value is different from an absent view.
            after: if operation != 2 {
                SvBytes::borrowed(&[])
            } else {
                SvBytes::EMPTY
            },
        };
        match host.fault {
            Fault::InvalidOperation => change.operation = 8,
            Fault::UnexpectedBefore => {
                change.before = SvBytes {
                    ptr: std::ptr::dangling(),
                    len: 0,
                }
            }
            Fault::MissingAfter => change.has_after = 0,
            _ => {}
        }
        let status = unsafe { visit.unwrap()(context, &change) };
        if !status.is_success() {
            return SvStatus::SUCCESS;
        } // cannot hide visitor refusal
        unsafe { count.write(u64::from(operation) + 1) };
    }
    if matches!(host.fault, Fault::Count) {
        unsafe { count.write(8) };
    }
    SvStatus::SUCCESS
}
fn table(host: &Host) -> SvCandidateV4 {
    SvCandidateV4 {
        h: SvHeader {
            abi_major: CORE_ABI_MAJOR,
            kind: 4,
            table_bytes: std::mem::size_of::<SvCandidateV4>() as u64,
            provided_caps: CORE_CAPABILITIES,
            required_caps: 0,
        },
        context: (host as *const Host).cast(),
        time_ns: Some(time),
        read: Some(read),
        resolve_uk: Some(resolve),
        changes: Some(changes),
    }
}

#[test]
fn candidate_borrows_unchanged_records_and_after_only_never_fetches_before() {
    let host = Host {
        fault: Fault::None,
        before_reads: Cell::new(0),
        after_reads: Cell::new(0),
    };
    let table = table(&host);
    let candidate = unsafe { Candidate::from_raw(&table).unwrap() };
    assert_eq!(candidate.ref_tx_time_ns(), Ok(0));
    let mut seen = Vec::new();
    candidate
        .for_each_after_change(&mut |change| {
            match change {
                CandidateAfterChange::New { key, after } => {
                    assert!(after.is_empty());
                    seen.push((key.sys_id, 0));
                }
                CandidateAfterChange::Update { key, after } => {
                    assert!(after.is_empty());
                    seen.push((key.sys_id, 1));
                }
                CandidateAfterChange::Delete { key } => seen.push((key.sys_id, 2)),
            }
            // Shared visitor -> unchanged record -> another shared read.
            assert!(
                candidate
                    .read_after(
                        RecordKey {
                            kind: 7,
                            sys_id: 10
                        },
                        &mut |after| {
                            assert_eq!(after, &[5]);
                            assert!(
                                !candidate
                                    .read_after(
                                        RecordKey {
                                            kind: 7,
                                            sys_id: 11
                                        },
                                        &mut |_| panic!("absent record")
                                    )
                                    .unwrap()
                            );
                        }
                    )
                    .unwrap()
            );
        })
        .unwrap();
    assert_eq!(seen, [(0, 0), (1, 1), (2, 2)]);
    assert_eq!(host.before_reads.get(), 0);
    assert_eq!(host.after_reads.get(), 6);
    let mut seen = Vec::new();
    candidate
        .for_each_change(&mut |change| {
            seen.push(match change {
                CandidateChange::New { key, after } => {
                    assert!(after.is_empty());
                    (key.sys_id, 0)
                }
                CandidateChange::Update { key, before, after } => {
                    assert_eq!(before, &[3]);
                    assert!(after.is_empty());
                    (key.sys_id, 1)
                }
                CandidateChange::Delete { key, before } => {
                    assert_eq!(before, &[3]);
                    (key.sys_id, 2)
                }
            })
        })
        .unwrap();
    assert_eq!(seen, [(0, 0), (1, 1), (2, 2)]);
    assert_eq!(candidate.resolve_unique_key_before(7, 2, &[42]).unwrap(), Some(10));
    assert_eq!(candidate.resolve_unique_key_after(7, 2, &[42]).unwrap(), Some(11));
    assert!(
        candidate
            .read_before(
                RecordKey {
                    kind: 7,
                    sys_id: 10
                },
                &mut |before| assert_eq!(before, &[3])
            )
            .unwrap()
    );
}

#[test_case(Fault::InvalidOperation; "operation tag")]
#[test_case(Fault::UnexpectedBefore; "after-only cannot borrow before")]
#[test_case(Fault::MissingAfter; "missing after")]
#[test_case(Fault::Count; "delivered count")]
#[test_case(Fault::VisitorPanic; "visitor unwind")]
fn candidate_callback_failure_cannot_be_hidden_by_host_success(fault: Fault) {
    let host = Host {
        fault,
        before_reads: Cell::new(0),
        after_reads: Cell::new(0),
    };
    let table = table(&host);
    let candidate = unsafe { Candidate::from_raw(&table).unwrap() };
    let failure = candidate
        .for_each_after_change(&mut |_| {
            if matches!(fault, Fault::VisitorPanic) {
                panic!("candidate visitor");
            }
        })
        .unwrap_err();
    match fault {
        Fault::VisitorPanic => assert_eq!(failure, CoreExecutionError::PluginPanicked),
        _ => assert!(matches!(failure, CoreExecutionError::AbiContract(_))),
    }
    assert_eq!(candidate.failure.finish(Ok(())), SvStatus::from(&failure));
    assert_eq!(
        candidate.read_after(
            RecordKey {
                kind: 7,
                sys_id: 10
            },
            &mut |_| panic!("poisoned read")
        ),
        Err(failure)
    );
    assert_eq!(host.after_reads.get(), 0);
}

#[test]
fn candidate_null_read_is_rejected_before_borrow() {
    let host = Host {
        fault: Fault::NullRead,
        before_reads: Cell::new(0),
        after_reads: Cell::new(0),
    };
    let table = table(&host);
    let candidate = unsafe { Candidate::from_raw(&table).unwrap() };
    assert_eq!(
        candidate.read_after(
            RecordKey {
                kind: 7,
                sys_id: 10
            },
            &mut |_| panic!("invalid borrow")
        ),
        Err(CoreExecutionError::AbiContract(CoreAbiError::NullPointer))
    );
}

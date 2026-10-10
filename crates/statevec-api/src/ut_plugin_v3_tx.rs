//! A synchronous host-table fixture for the public SDK facade, not an engine.
use super::*;
use crate::plugin_v3::{CoreFailure, CoreOperation, HostFailure, ResourceFailure};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use test_case::test_case;

#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum Fault {
    #[default]
    None,
    Twice,
    MissingVisitor,
    FoundFlag,
    NullData,
    LongData,
    EventFailure,
    HostPanicked,
    UnknownStatus,
    ShortTable,
    MissingCallback,
    WrongKind,
    WrongVersion,
    UnknownCaps,
}
#[derive(Default)]
pub(crate) struct Host {
    pub fault: Fault,
    pub time: u64,
    records: RefCell<BTreeMap<u64, Vec<u8>>>,
    reads: Cell<usize>,
    events: RefCell<Vec<(u16, Vec<u8>)>>,
}
unsafe fn host<'a>(raw: *const c_void) -> &'a Host {
    unsafe { &*raw.cast::<Host>() }
}
unsafe extern "C" fn time(raw: *const c_void, out: *mut u64) -> SvStatus {
    unsafe {
        out.write(host(raw).time);
    }
    SvStatus::SUCCESS
}
unsafe extern "C" fn seq(_: *const c_void, out: *mut u64) -> SvStatus {
    unsafe {
        out.write(17);
    }
    SvStatus::SUCCESS
}
unsafe extern "C" fn read(
    raw: *const c_void,
    key: SvKey,
    visit: SvReadVisitor,
    context: *mut c_void,
    out: *mut u32,
) -> SvStatus {
    let host = unsafe { host(raw) };
    host.reads.set(host.reads.get() + 1);
    unsafe { out.write(0) };
    if matches!(host.fault, Fault::HostPanicked) {
        return SvStatus {
            tag: 8,
            cause: 0,
            detail: [0; 3],
        };
    }
    if matches!(host.fault, Fault::UnknownStatus) {
        return SvStatus {
            tag: 1234,
            cause: 0,
            detail: [0; 3],
        };
    }
    let records = host.records.borrow();
    let Some(data) = records.get(&key.sys_id) else {
        return SvStatus::SUCCESS;
    };
    unsafe {
        out.write(if matches!(host.fault, Fault::FoundFlag) {
            2
        } else {
            1
        })
    };
    if matches!(host.fault, Fault::MissingVisitor) {
        return SvStatus::SUCCESS;
    }
    let data = match host.fault {
        Fault::NullData => SvBytes {
            ptr: std::ptr::null(),
            len: 1,
        },
        Fault::LongData => SvBytes {
            ptr: std::ptr::null(),
            len: u64::MAX,
        },
        _ => SvBytes::borrowed(data),
    };
    let result = unsafe { visit.unwrap()(context, data) };
    if matches!(host.fault, Fault::Twice) {
        let _ = unsafe { visit.unwrap()(context, data) };
    }
    // Deliberately hide a visitor failure. SDK must retain its actual return.
    if result.is_success() {
        result
    } else {
        SvStatus::SUCCESS
    }
}
unsafe extern "C" fn resolve(
    raw: *const c_void,
    _: u16,
    _: u8,
    key: SvBytes,
    found: *mut u32,
    id: *mut u64,
) -> SvStatus {
    let bytes = unsafe { bytes(key).unwrap() };
    let key = bytes.first().copied().unwrap_or(0) as u64;
    unsafe {
        found.write(u32::from(host(raw).records.borrow().contains_key(&key)));
        id.write(key);
    }
    SvStatus::SUCCESS
}
unsafe extern "C" fn read_uk(
    raw: *const c_void,
    kind: u16,
    uk_id: u8,
    key: SvBytes,
    visit: SvReadVisitor,
    context: *mut c_void,
    out: *mut u32,
) -> SvStatus {
    let mut id = 0;
    let status = unsafe { resolve(raw, kind, uk_id, key, out, &mut id) };
    if !status.is_success() || unsafe { out.read() } == 0 {
        return status;
    }
    unsafe {
        read(
            raw,
            wire_key(RecordKey { kind, sys_id: id }),
            visit,
            context,
            out,
        )
    }
}
unsafe extern "C" fn keys(
    raw: *const c_void,
    kind: u16,
    visit: SvKeyVisitor,
    context: *mut c_void,
    out: *mut SvWalk,
) -> SvStatus {
    let keys: Vec<_> = unsafe { host(raw) }
        .records
        .borrow()
        .keys()
        .copied()
        .collect();
    let mut count = 0;
    for id in keys {
        let mut flow = 0;
        let status =
            unsafe { visit.unwrap()(context, wire_key(RecordKey { kind, sys_id: id }), &mut flow) };
        if !status.is_success() {
            unsafe {
                out.write(SvWalk {
                    tag: 1,
                    reserved: 0,
                    count,
                });
            }
            return status;
        }
        count += 1;
    }
    unsafe {
        out.write(SvWalk {
            tag: 0,
            reserved: 0,
            count,
        });
    }
    SvStatus::SUCCESS
}
unsafe extern "C" fn count(
    raw: *const c_void,
    _: u16,
    _: u8,
    _: SvBytes,
    cap: u64,
    out: *mut SvWalk,
) -> SvStatus {
    let length = unsafe { host(raw) }.records.borrow().len() as u64;
    unsafe {
        out.write(SvWalk {
            tag: u32::from(length >= cap),
            reserved: 0,
            count: length.min(cap),
        });
    }
    SvStatus::SUCCESS
}
unsafe extern "C" fn create(
    raw: *mut c_void,
    kind: u16,
    supplied: u32,
    id: u64,
    input: SvBytes,
    out: *mut SvKey,
) -> SvStatus {
    let host = unsafe { host(raw) };
    let id = if supplied == 1 {
        id
    } else {
        host.records.borrow().keys().copied().max().unwrap_or(0) + 1
    };
    let key = wire_key(RecordKey { kind, sys_id: id });
    let data = unsafe { bytes(input) }.expect("fixture raw create input").to_vec();
    host.records.borrow_mut().insert(id, data);
    unsafe { out.write(key) };
    SvStatus::SUCCESS
}
unsafe extern "C" fn update(
    raw: *mut c_void,
    key: SvKey,
    visit: SvWriteVisitor,
    context: *mut c_void,
    found: *mut u32,
) -> SvStatus {
    let host = unsafe { host(raw) };
    let mut records = host.records.borrow_mut();
    let Some(data) = records.get_mut(&key.sys_id) else {
        unsafe { found.write(0) };
        return SvStatus::SUCCESS;
    };
    unsafe {
        found.write(1);
        visit.unwrap()(
            context,
            key,
            SvMutBytes {
                ptr: data.as_mut_ptr(),
                len: data.len() as u64,
            },
        )
    }
}
unsafe extern "C" fn update_uk(
    raw: *mut c_void,
    kind: u16,
    uk_id: u8,
    key: SvBytes,
    visit: SvWriteVisitor,
    context: *mut c_void,
    found: *mut u32,
) -> SvStatus {
    let mut id = 0;
    unsafe {
        resolve(raw, kind, uk_id, key, found, &mut id);
        update(
            raw,
            wire_key(RecordKey { kind, sys_id: id }),
            visit,
            context,
            found,
        )
    }
}
unsafe extern "C" fn delete(raw: *mut c_void, key: SvKey, found: *mut u32) -> SvStatus {
    let existed = unsafe { host(raw) }
        .records
        .borrow_mut()
        .remove(&key.sys_id)
        .is_some();
    unsafe { found.write(u32::from(existed)) };
    SvStatus::SUCCESS
}
unsafe extern "C" fn delete_uk(
    raw: *mut c_void,
    kind: u16,
    uk_id: u8,
    key: SvBytes,
    found: *mut u32,
) -> SvStatus {
    let mut id = 0;
    unsafe {
        resolve(raw, kind, uk_id, key, found, &mut id);
        delete(raw, wire_key(RecordKey { kind, sys_id: id }), found)
    }
}
unsafe extern "C" fn event(raw: *mut c_void, kind: u16, value: SvBytes) -> SvStatus {
    let host = unsafe { host(raw) };
    if matches!(host.fault, Fault::EventFailure) {
        return SvStatus::from(&ExecutionFailure::Host(HostFailure::Resource(
            ResourceFailure::Os { code: 5 },
        )));
    }
    host.events
        .borrow_mut()
        .push((kind, unsafe { bytes(value).unwrap() }.to_vec()));
    SvStatus::SUCCESS
}
pub(crate) fn table(host: &Host) -> SvTxV3 {
    let mut table = SvTxV3 {
        h: SvHeader {
            abi_major: 3,
            kind: 3,
            table_bytes: std::mem::size_of::<SvTxV3>() as u64,
            provided_caps: CORE_CAPABILITIES,
            required_caps: 0,
        },
        context: (host as *const Host).cast_mut().cast(),
        time_ns: Some(time),
        tx_seq: Some(seq),
        read: Some(read),
        read_uk: Some(read_uk),
        resolve_uk: Some(resolve),
        for_each_key: Some(keys),
        count_prefix: Some(count),
        create: Some(create),
        update: Some(update),
        update_uk: Some(update_uk),
        delete_record: Some(delete),
        delete_uk: Some(delete_uk),
        emit_event: Some(event),
    };
    match host.fault {
        Fault::ShortTable => table.h.table_bytes = 32,
        Fault::MissingCallback => table.read = None,
        Fault::WrongKind => table.h.kind = 4,
        Fault::WrongVersion => table.h.abi_major = 2,
        Fault::UnknownCaps => table.h.required_caps = 1 << 63,
        _ => {}
    }
    table
}

#[test_case(100; "time 100")]
#[test_case(5; "decreasing time")]
#[test_case(0; "epoch zero")]
fn transaction_views_support_shared_reentry_and_exact_time(time: u64) {
    let host = Host {
        time,
        ..Default::default()
    };
    host.records
        .borrow_mut()
        .extend([(1, vec![1, 2]), (2, vec![3, 4])]);
    let table = table(&host);
    let tx = unsafe { PluginTx::from_raw(&table).unwrap() };
    assert_eq!(tx.ref_tx_time_ns_raw().unwrap(), time);
    assert_eq!(tx.tx_seq_raw().unwrap(), 17);
    let mut observed = Vec::new();
    tx.for_each_record_key(7, &mut |key| {
        tx.with_read_raw(key, |outer| {
            let inner = tx
                .with_read_raw(RecordKey { kind: 7, sys_id: 2 }, |bytes| bytes.to_vec())
                .unwrap()
                .unwrap();
            observed.push((key.sys_id, outer.to_vec(), inner));
        })
        .unwrap();
    })
    .unwrap();
    assert_eq!(
        observed,
        [(1, vec![1, 2], vec![3, 4]), (2, vec![3, 4], vec![3, 4])]
    );
    assert_eq!(tx.resolve_uk_id(7, 1, &[2]).unwrap(), Some(2));
    assert_eq!(
        tx.with_read_raw(RecordKey { kind: 7, sys_id: 3 }, |_| panic!(
            "absent visitor"
        ))
        .unwrap(),
        None::<()>
    );
    assert_eq!(
        tx.count_index_prefix_capped_raw(7, 0, &[], 1).unwrap(),
        CanonicalIndexCount::AtLeast(1)
    );
    assert_eq!(
        tx.count_index_prefix_capped_raw(7, 0, &[], 3).unwrap(),
        CanonicalIndexCount::Exact(2)
    );
    assert_eq!(
        tx.count_index_prefix_capped_raw(7, 0, &[], 0).unwrap(),
        CanonicalIndexCount::AtLeast(0)
    );
    assert!(tx.finish(Ok(())).is_success());
}

#[test]
fn mutation_facade_preserves_supplied_id_and_returns_visitor_result() {
    let host = Host::default();
    let table = table(&host);
    let mut tx = unsafe { PluginTx::from_raw(&table).unwrap() };
    let key = tx.create_with_sys_id_raw(7, 42, vec![1, 2]).unwrap();
    assert_eq!(
        key,
        RecordKey {
            kind: 7,
            sys_id: 42
        }
    );
    assert_eq!(
        tx.update_raw(key, |bytes| {
            bytes[1] += 1;
            bytes[1]
        })
        .unwrap(),
        Some(3)
    );
    tx.emit_event_raw(9, vec![3]);
    assert_eq!(host.records.borrow()[&42], [1, 3]);
    assert_eq!(*host.events.borrow(), [(9, vec![3])]);
    assert!(tx.delete_raw(key).unwrap());
    assert!(!tx.delete_raw(key).unwrap());
    assert_eq!(
        tx.update_raw(key, |_| panic!("absent update visitor"))
            .unwrap(),
        None::<()>
    );
}

#[test_case(Fault::Twice, AbiFailure::CallbackCount { expected: 1, actual: 2 }; "second visitor")]
#[test_case(Fault::MissingVisitor, AbiFailure::CallbackCount { expected: 1, actual: 0 }; "missing visitor")]
#[test_case(Fault::FoundFlag, AbiFailure::Flag { actual: 2 }; "bad found flag")]
#[test_case(Fault::NullData, AbiFailure::NullPointer; "null before borrow")]
#[test_case(Fault::LongData, AbiFailure::Length { actual: u64::MAX, maximum: isize::MAX as u64 }; "extent before borrow")]
fn malformed_read_is_latched_even_if_host_reports_success(fault: Fault, expected: AbiFailure) {
    let host = Host {
        fault,
        ..Default::default()
    };
    host.records.borrow_mut().insert(1, vec![1]);
    let table = table(&host);
    let tx = unsafe { PluginTx::from_raw(&table).unwrap() };
    assert_eq!(
        tx.with_read_raw(RecordKey { kind: 7, sys_id: 1 }, |_| ()),
        Err(ExecutionFailure::AbiContract(expected))
    );
    assert_eq!(
        tx.with_read_raw(RecordKey { kind: 7, sys_id: 1 }, |_| panic!(
            "poisoned callback"
        )),
        Err(ExecutionFailure::AbiContract(expected))
    );
    assert_eq!(host.reads.get(), 1);
    assert_eq!(tx.finish(Ok(())), SvStatus::from(&expected));
}

#[test]
fn visitor_create_draft_host_is_rejected_before_reading_callbacks() {
    let host = Host::default();
    let mut table = table(&host);
    table.h.provided_caps &= !CORE_RAW_CREATE_CAPABILITY;
    table.create = None;
    // SAFETY: the fixture still provides a complete readable header/table.
    let failure = unsafe { PluginTx::from_raw(&table) }.err().expect("old create ABI must refuse");
    assert_eq!(failure, AbiFailure::UnsupportedCapabilities { required: CORE_RAW_CREATE_CAPABILITY });
    assert_eq!(host.reads.get(), 0);
}

#[test_case(Fault::ShortTable; "short table")]
#[test_case(Fault::MissingCallback; "missing function")]
#[test_case(Fault::WrongKind; "wrong kind")]
#[test_case(Fault::WrongVersion; "wrong version")]
#[test_case(Fault::UnknownCaps; "unknown capability")]
fn incompatible_host_table_is_refused_before_any_callback(fault: Fault) {
    let host = Host {
        fault,
        ..Default::default()
    };
    let table = table(&host);
    assert!(unsafe { PluginTx::from_raw(&table) }.is_err());
    assert_eq!(host.reads.get(), 0);
}

#[test_case(Fault::HostPanicked; "host panic")]
#[test_case(Fault::UnknownStatus; "unknown host status")]
fn unusable_host_failure_survives_later_business_rejection(fault: Fault) {
    let host = Host {
        fault,
        ..Default::default()
    };
    let table = table(&host);
    let tx = unsafe { PluginTx::from_raw(&table).unwrap() };
    let source = tx
        .with_read_raw(RecordKey { kind: 1, sys_id: 1 }, |_| ())
        .unwrap_err();
    let result = tx.finish(Err(TxFailure::Rejected(
        crate::BusinessRejectCode::new(42).unwrap(),
    )));
    assert_eq!(
        result.decode_for(CoreOperation::RunTx).unwrap(),
        Some(CoreFailure::Transaction(TxFailure::Execution(source)))
    );
}

#[test]
fn read_visitor_panic_is_not_hidden_by_host_success() {
    let host = Host::default();
    host.records.borrow_mut().insert(1, Vec::new());
    let table = table(&host);
    let tx = unsafe { PluginTx::from_raw(&table).unwrap() };
    assert_eq!(
        tx.with_read_raw(RecordKey { kind: 7, sys_id: 1 }, |_| panic!(
            "visitor panic"
        )),
        Err::<Option<()>, _>(ExecutionFailure::PluginPanicked)
    );
    assert_eq!(tx.finish(Ok(())).tag, 7);
}

#[derive(Clone, Copy)]
enum ScalarFailure {
    Time,
    Position,
}

unsafe extern "C" fn ordinary_scalar_failure(_: *const c_void, _: *mut u64) -> SvStatus {
    SvStatus::from(&ExecutionFailure::Host(HostFailure::Resource(ResourceFailure::Os { code: 5 })))
}

#[test_case(ScalarFailure::Time; "time failure before visitor panic")]
#[test_case(ScalarFailure::Position; "position failure before visitor panic")]
fn nested_scalar_error_cannot_mask_later_visitor_unusability(scalar: ScalarFailure) {
    let host = Host::default();
    host.records.borrow_mut().insert(1, Vec::new());
    let mut table = table(&host);
    match scalar {
        ScalarFailure::Time => table.time_ns = Some(ordinary_scalar_failure),
        ScalarFailure::Position => table.tx_seq = Some(ordinary_scalar_failure),
    }
    let tx = unsafe { PluginTx::from_raw(&table).unwrap() };
    let key = RecordKey { kind: 7, sys_id: 1 };

    let result: Result<Option<()>, _> = tx.with_read_raw(key, |_| {
        assert!(match scalar {
            ScalarFailure::Time => tx.ref_tx_time_ns_raw().is_err(),
            ScalarFailure::Position => tx.tx_seq_raw().is_err(),
        });
        panic!("visitor fails after handling an ordinary scalar error");
    });
    assert_eq!(result, Err(ExecutionFailure::PluginPanicked));
    assert_eq!(tx.with_read_raw(key, |_| ()), Err(ExecutionFailure::PluginPanicked));
    assert_eq!(host.reads.get(), 1, "unusable scope never calls the host again");
    assert_eq!(tx.finish(Ok(())), SvStatus::from(&ExecutionFailure::PluginPanicked));
}

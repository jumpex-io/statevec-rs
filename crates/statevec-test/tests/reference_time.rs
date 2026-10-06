//! Public-prelude usage, generated dispatch and call-scoped test execution time.
use statevec::prelude::*;
use statevec_test::TestHost;
use std::panic::{AssertUnwindSafe, catch_unwind};
use test_case::test_case;

#[schema_module(version = "1.0")]
mod schema {
    use super::*;

    #[record(kind = 1, record_len = 64, uk(id = 0, fields = [id]))]
    pub struct Row {
        #[field(index = 1, immutable)]
        pub id: u64,
    }

    #[command(kind = 1)]
    pub struct Stamp {
        #[field(index = 1)]
        pub value: u64,
    }

    #[event(kind = 1)]
    pub struct Stamped {
        #[field(index = 1)]
        pub time_ns: u64,
    }
}
use schema::*;

#[test]
fn guide_transaction_time_example() {
    let mut engine = TestHost::new(SchemaRegistry::with_records(Version::new(1, 0), &[]));
    assert_eq!(engine.ref_tx_time_ns(), Err(ReferenceTimeUnavailable));
    assert_eq!(engine.transaction_at(0, |tx| tx.ref_tx_time_ns()), Ok(0));
    assert_eq!(engine.transaction_at(90, |tx| tx.ref_tx_time_ns()), Ok(90));
    assert_eq!(engine.transaction_at(5, |tx| tx.ref_tx_time_ns()), Ok(5));
    assert_eq!(engine.ref_tx_time_ns(), Err(ReferenceTimeUnavailable));
}

#[derive(Debug, PartialEq, Eq)]
enum Failure {
    Host(RuntimeHostError),
    MissingTime(ReferenceTimeUnavailable),
}
impl From<RuntimeHostError> for Failure {
    fn from(source: RuntimeHostError) -> Self { Self::Host(source) }
}
impl From<ReferenceTimeUnavailable> for Failure {
    fn from(source: ReferenceTimeUnavailable) -> Self { Self::MissingTime(source) }
}

struct ClockRuntime;
impl ClockRuntime {
    fn stamp<Tx: TypedTxContext + ?Sized>(&self, tx: &mut Tx, command: StampAccess<'_>) -> Result<(), Failure>
    where Failure: From<Tx::Error> {
        // Write before reading time so an unavailable input exercises rollback.
        tx.create_typed::<Row, _>(|row| { row.init_id(command.value()); })?;
        let time = tx.ref_tx_time_ns()?;
        tx.emit_typed_event::<Stamped>(Stamped::builder().set_time_ns(time).build().unwrap());
        Ok(())
    }
}
command_dispatch! {
    fn try_dispatch_clock;
    runtime = ClockRuntime;
    error = Failure;
    Stamp => Self::stamp,
}

impl RuntimePlugin for ClockRuntime {
    fn name(&self) -> &'static str { "clock-test" }
    fn schema_registry(&self) -> SchemaRegistry { registry() }
    fn run_tx(&self, host: &mut dyn RuntimeHostContext, command: &dyn RuntimeCommandEnvelope) -> Result<(), RuntimePluginError> {
        self.preflight_try_dispatch_clock(command.command_kind(), command.payload())
            .map_err(|error| RuntimePluginError::new(format!("{error:?}")))?;
        host.debug_log(format!("{}:{}", command.ext_seq(), command.ref_ext_time_us()))
            .map_err(|error| RuntimePluginError::new(error.to_string()))?;
        assert!(self.try_dispatch_clock(host, command)
            .map_err(|error| RuntimePluginError::new(format!("{error:?}")))?);
        Ok(())
    }
    fn validate_biz_invariants(&self, _: &dyn BizInvariantReadContext) -> Result<(), String> { Ok(()) }
    fn on_unload(&mut self) -> Result<(), RuntimePluginUnloadError> { Ok(()) }
}

#[test]
fn public_prelude_and_generated_handler_read_the_exact_call_time() {
    let mut host = TestHost::new(registry());
    assert_eq!(host.ref_tx_time_ns(), Err(ReferenceTimeUnavailable));
    for (id, time) in [(1, 90), (2, 5), (3, 0)] {
        let command = Command::new(Stamp::KIND, id, 123_456,
            Stamp::builder().set_value(id).build().unwrap());
        host.transaction_at(time, |tx| {
            assert_eq!(tx.ref_tx_time_ns(), Ok(time));
            assert_eq!(TxReadContext::ref_tx_time_ns_raw(tx), Ok(time));
            assert_eq!(RuntimeHostContext::ref_tx_time_ns_raw(tx), Ok(time));
            ClockRuntime.try_dispatch_clock(tx, &command)
        }).unwrap();
        assert_eq!(host.ref_tx_time_ns(), Err(ReferenceTimeUnavailable));
    }
    let times: Vec<_> = host.events().iter().map(|event| Stamped::wrap(&event.payload).time_ns()).collect();
    assert_eq!(times, [90, 5, 0], "time is neither monotonic nor taken from the external envelope");
    let command = Command::new(Stamp::KIND, 4, 999, Stamp::builder().set_value(4).build().unwrap());
    assert_eq!(host.transaction(|tx| ClockRuntime.try_dispatch_clock(tx, &command)),
        Err(Failure::MissingTime(ReferenceTimeUnavailable)));
    assert_eq!(host.record_count(), 3, "unavailable time rolls back provisional writes");
    assert_eq!(host.events().len(), 3);
}

#[derive(Clone, Copy)]
enum Exit { Success, Error, Panic }

#[test_case(Exit::Error; "error")]
#[test_case(Exit::Panic; "panic")]
fn failed_outer_transaction_clears_time_before_the_next_call(exit: Exit) {
    let mut host = TestHost::new(registry());
    let result = catch_unwind(AssertUnwindSafe(|| host.transaction_at(71, |tx| {
        assert_eq!(tx.ref_tx_time_ns(), Ok(71));
        match exit {
            Exit::Success => Ok(()),
            Exit::Error => Err("refused"),
            Exit::Panic => panic!("test handler unwind"),
        }
    })));
    match exit {
        Exit::Error => assert!(matches!(result, Ok(Err("refused")))),
        Exit::Panic => assert!(result.is_err()),
        Exit::Success => unreachable!(),
    }
    assert_eq!(host.ref_tx_time_ns(), Err(ReferenceTimeUnavailable));
    assert_eq!(host.transaction(|tx| tx.ref_tx_time_ns()), Err(ReferenceTimeUnavailable));
}

#[test_case(Exit::Success, Some(0); "timed_success")]
#[test_case(Exit::Error, Some(0); "timed_error")]
#[test_case(Exit::Panic, Some(0); "timed_panic")]
#[test_case(Exit::Success, None; "untimed_success")]
#[test_case(Exit::Error, None; "untimed_error")]
#[test_case(Exit::Panic, None; "untimed_panic")]
fn nested_transaction_restores_outer_time_and_preserves_rollback(exit: Exit, inner_time: Option<u64>) {
    let mut host = TestHost::new(registry());
    host.transaction_at(71, |outer| {
        let nested = catch_unwind(AssertUnwindSafe(|| {
            let operation = |inner: &mut TestHost| {
                assert_eq!(inner.ref_tx_time_ns(), inner_time.ok_or(ReferenceTimeUnavailable));
                TypedTxContext::create_typed::<Row, _>(inner, |row| { row.init_id(1); }).unwrap();
                RuntimeHostContext::emit_typed_event_raw(inner, Stamped::KIND, &[0; 8]).unwrap();
                match exit {
                    Exit::Success => Ok(()),
                    Exit::Error => Err("refused"),
                    Exit::Panic => panic!("test handler unwind"),
                }
            };
            match inner_time {
                Some(time) => outer.transaction_at(time, operation),
                None => outer.transaction(operation),
            }
        }));
        match exit {
            Exit::Success => assert!(matches!(nested, Ok(Ok(())))),
            Exit::Error => assert!(matches!(nested, Ok(Err("refused")))),
            Exit::Panic => assert!(nested.is_err()),
        }
        assert_eq!(outer.ref_tx_time_ns(), Ok(71), "restore the outer call even after unwind");
        let surviving = usize::from(matches!(exit, Exit::Success));
        assert_eq!(outer.record_count(), surviving);
        assert_eq!(outer.events().len(), surviving);
        assert_eq!(outer.read::<Row, _>(Row::uk(1), |row| row.id()), if surviving == 1 { Some(1) } else { None });
        let next = TypedTxContext::create_typed::<Row, _>(outer, |row| { row.init_id(2); }).unwrap();
        assert_eq!(next.sys_id, surviving as u64 + 1, "rollback restores id allocation too");
        Ok::<_, ()>(())
    }).unwrap();
    assert_eq!(host.ref_tx_time_ns(), Err(ReferenceTimeUnavailable));
}

#[test]
fn plugin_run_at_keeps_external_time_and_sequence_separate_and_preflights_payload() {
    let mut host = TestHost::for_plugin(ClockRuntime).with_ref_ext_time_us(123_456).with_starting_ext_seq(20);
    for (id, time) in [(1, 90), (2, 0)] {
        host.run_at::<Stamp>(time, Stamp::builder().set_value(id).build().unwrap()).unwrap();
        assert_eq!(host.ref_tx_time_ns(), Err(ReferenceTimeUnavailable));
    }
    assert_eq!(host.debug_logs(), ["20:123456", "21:123456"]);
    assert_eq!(host.events().iter().map(|event| Stamped::wrap(&event.payload).time_ns()).collect::<Vec<_>>(), [90, 0]);
    assert!(host.run_at::<Stamp>(17, [0; 7]).is_err(), "truncated payload never enters the handler");
    assert!(host.run::<Stamp>(Stamp::builder().set_value(3).build().unwrap()).is_err(), "untimed run cannot inherit the last time");
    assert_eq!(host.record_count(), 2);
    assert_eq!(host.events().len(), 2);
    assert_eq!(host.debug_logs().len(), 2, "failed calls roll back diagnostics too");
    assert_eq!(host.ref_tx_time_ns(), Err(ReferenceTimeUnavailable));
}

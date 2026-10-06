use bank::{Account, BankError, BankRuntime, Deposit, Withdraw, registry};
use statevec::{Command, CommandSchema, GeneratedCommandAccess};
use statevec_test::TestHost;

#[test]
fn withdrawal_is_refused_without_changing_state() {
    let runtime = BankRuntime;
    let mut engine = TestHost::new(registry());
    let deposit = Command::new(
        Deposit::KIND, 1, 0,
        Deposit::builder().set_account_id(42).set_amount(100).build().unwrap(),
    );
    engine.transaction(|tx| runtime.dispatch(tx, &deposit)).unwrap();
    let events_before = engine.events().to_vec();

    let withdrawal = Command::new(
        Withdraw::KIND, 2, 0,
        Withdraw::builder().set_account_id(42).set_amount(150).build().unwrap(),
    );
    let error = engine.transaction(|tx| runtime.dispatch(tx, &withdrawal)).unwrap_err();

    assert_eq!(error, BankError::InsufficientFunds);
    assert_eq!(error.rejection_code().unwrap().get(), 1);
    assert_eq!(engine.expect::<Account, _>(Account::uk(42), |r| r.total_credit()), 100);
    assert_eq!(engine.events(), events_before);
}

#[test]
fn malformed_payload_is_refused_before_the_handler_runs() {
    let runtime = BankRuntime;
    let mut engine = TestHost::new(registry());
    let payload = Deposit::builder().set_account_id(42).set_amount(100).build().unwrap();
    let command = Command::new(Deposit::KIND, 1, 0, payload[..payload.len() - 1].to_vec());

    let error = engine.transaction(|tx| runtime.dispatch(tx, &command)).unwrap_err();

    assert!(matches!(error, BankError::InvalidInput(_)), "{error:?}");
    assert_eq!(error.rejection_code(), None);
    assert_eq!(engine.record_count(), 0);
    assert!(engine.events().is_empty());
}

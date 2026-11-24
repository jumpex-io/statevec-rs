//! One upstream deposit per batch on the installed Bank stream (client 7, stream 9).
//! See the cluster client guide for the journal, process restart and acknowledgement.
use serde::{Deserialize, Serialize};
use statevec_ingress_client::{
    BatchClientSettings,
    ClientStreamBatch, Command, DeploymentIdentity, Digest32, EngineCommandPayload, EntryBinding, EntryDigest, InputRef,
    MemberResult, encode_runtime_binary_command_payload,
    BatchClient, BatchDriverStep, BatchDriverWorker, BatchEvent, BatchProgress, BatchRetryConfig,
    BatchTcpWorker, BatchTerminal, ClientError, ClientStatus, batch_monotonic_now,
};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::net::SocketAddr;
use std::num::{NonZeroU64, NonZeroU128};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tempfile::NamedTempFile;

const COMMANDS_PER_BATCH: u32 = 1;
const FRAME_BYTES: u32 = 2 * 1024 * 1024;
const DISCOVERY_RETRY_MS: u64 = 25;
const OPERATION_TIMEOUT_MS: u64 = 1_000;
const BATCH_LIFETIME_MS: u64 = 20_000;
const UNAVAILABLE_REPLIES_PER_PEER: u32 = 20;
// Leave time to return and persist a terminal after the SDK's batch lifetime.
const WRITER_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Clone, Deserialize, PartialEq, Serialize)]
struct Identity {
    cluster_identity: String,
    genesis_identity: String,
    execution_lineage: String,
}

impl Identity {
    fn digests(&self) -> io::Result<DeploymentIdentity> {
        fn digest(text: &str) -> io::Result<Digest32> {
            if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Err(io::Error::other("deployment identity must contain 64 hexadecimal digits"));
            }
            let mut bytes = [0; 32];
            for (position, byte) in bytes.iter_mut().enumerate() {
                let start = 2 * position;
                *byte = u8::from_str_radix(&text[start..start + 2], 16).map_err(io::Error::other)?;
            }
            Ok(Digest32::new(bytes))
        }
        Ok(DeploymentIdentity::new(digest(&self.cluster_identity)?, digest(&self.genesis_identity)?, digest(&self.execution_lineage)?))
    }
}

#[derive(Deserialize)]
struct Deployment {
    identity: Identity,
    endpoints: Vec<(u64, SocketAddr)>,
    raft_tick_millis: NonZeroU64,
    max_outcome_bytes: u32,
}

#[derive(Deserialize, PartialEq, Serialize)]
struct SavedCommit {
    index: NonZeroU64,
    term: NonZeroU64,
    digest: EntryDigest,
}

impl SavedCommit {
    fn binding(&self) -> EntryBinding {
        EntryBinding { index: self.index, term: self.term, digest: self.digest }
    }
}

impl From<EntryBinding> for SavedCommit {
    fn from(entry: EntryBinding) -> Self {
        Self { index: entry.index, term: entry.term, digest: entry.digest }
    }
}

#[derive(Deserialize, Serialize)]
struct Original {
    first: InputRef,
    command: EngineCommandPayload,
    committed: Option<SavedCommit>,
    #[serde(default)]
    exact_result_unavailable: bool,
}

impl Original {
    fn observe_commit(&mut self, entry: EntryBinding) -> io::Result<bool> {
        if let Some(known) = &self.committed {
            if known.binding() != entry {
                return Err(io::Error::other("commit binding changed"));
            }
            return Ok(false);
        }
        self.committed = Some(entry.into());
        Ok(true)
    }
}

#[derive(Deserialize, PartialEq, Serialize)]
struct BusinessReceipt {
    identity: Identity,
    input: InputRef,
    entry: SavedCommit,
    upstream_receipt: String,
}

#[derive(Deserialize, Serialize)]
struct Journal {
    identity: Identity,
    cursor: u64,
    pending: Option<Original>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_reconciliation: Option<BusinessReceipt>,
}

impl Journal {
    fn save(&self, directory: &Path) -> io::Result<()> {
        let mut file = NamedTempFile::new_in(directory)?;
        serde_json::to_writer(&mut file, self)?;
        file.as_file().sync_all()?;
        file.persist(directory.join("writer.json")).map_err(|e| e.error)?;
        File::open(directory)?.sync_all()
    }

    fn keep_commit(&mut self, directory: &Path, entry: EntryBinding) -> io::Result<()> {
        let original = self.pending.as_mut().ok_or_else(|| io::Error::other("missing saved batch"))?;
        if original.observe_commit(entry)? {
            self.save(directory)?;
        }
        Ok(())
    }

    fn unavailable(
        &mut self,
        directory: &Path,
        entry: EntryBinding,
        problem: Option<Arc<ClientError>>,
    ) -> io::Result<()> {
        let original = self.pending.as_mut().ok_or_else(|| io::Error::other("missing saved batch"))?;
        original.observe_commit(entry)?;
        original.exact_result_unavailable = true;
        let request = BusinessReceipt {
            identity: self.identity.clone(),
            input: original.first.clone(),
            entry: entry.into(),
            upstream_receipt: String::new(),
        };
        self.save(directory)?;
        println!("{}", serde_json::json!({ "needs_reconciliation": request }));
        if let Some(cause) = problem {
            eprintln!("{}", session_error(cause));
        }
        Err(io::Error::other("batch applied; verify its business result and use resolve with the upstream receipt"))
    }

    fn resolve(&mut self, directory: &Path, receipt: BusinessReceipt) -> io::Result<()> {
        if receipt.upstream_receipt.trim().is_empty() || receipt.identity.digests()? != self.identity.digests()? {
            return Err(io::Error::other("a receipt for this deployment with an upstream reference is required"));
        }
        let sequence = receipt.input.client_seq;
        if self.cursor == sequence && self.last_reconciliation.as_ref() == Some(&receipt) {
            return Ok(());
        }
        let original = self.pending.as_ref().ok_or_else(|| io::Error::other("no pending batch to resolve"))?;
        if self.cursor.checked_add(1) != Some(sequence)
            || (receipt.input.client_id, receipt.input.stream_id) != (7, 9)
            || original.first != receipt.input
            || !original.exact_result_unavailable
            || original.committed.as_ref() != Some(&receipt.entry)
        {
            return Err(io::Error::other("receipt must match the pending applied batch with an unavailable exact result"));
        }
        self.last_reconciliation = Some(receipt);
        self.pending = None;
        self.cursor = sequence;
        self.save(directory)
    }
}

fn number(value: &str) -> io::Result<u64> {
    value.parse().map_err(io::Error::other)
}

#[derive(Debug)]
struct SessionError(Arc<ClientError>);

impl std::fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "bank session: {:?}", self.0)
    }
}

impl std::error::Error for SessionError {}

fn session_error(cause: impl Into<Arc<ClientError>>) -> io::Error {
    io::Error::other(SessionError(cause.into()))
}

fn deposit(sequence: u64, account: u64, amount: u64) -> io::Result<Original> {
    let payload = [account.to_le_bytes(), amount.to_le_bytes()].concat();
    let command = encode_runtime_binary_command_payload(&Command::new(1, sequence, 0, payload))
        .map_err(io::Error::other)?;
    Ok(Original {
        first: InputRef { client_id: 7, stream_id: 9, client_seq: sequence, request_id: None },
        command,
        committed: None,
        exact_result_unavailable: false,
    })
}

fn drive(
    config: &Deployment,
    directory: &Path,
    journal: &mut Journal,
    recover: bool,
    read_acknowledgement: impl FnOnce(&mut String) -> io::Result<usize>,
) -> io::Result<()> {
    let original = journal.pending.as_ref().ok_or_else(|| io::Error::other("no pending batch to recover"))?;
    let first = original.first.clone();
    let sequence = first.client_seq;
    if (first.client_id, first.stream_id) != (7, 9) || journal.cursor.checked_add(1) != Some(sequence) {
        return Err(io::Error::other("saved batch does not follow the durable cursor on stream 7/9"));
    }
    let committed = original.committed.as_ref().map(SavedCommit::binding);
    let batch = ClientStreamBatch::try_new(
        first.clone(),
        vec![original.command.clone()],
        FRAME_BYTES as usize,
        COMMANDS_PER_BATCH as usize,
    )
    .map_err(|(cause, _, _)| io::Error::other(cause))?;
    let mut nonce = [0; 16];
    File::open("/dev/urandom")?.read_exact(&mut nonce)?;
    let incarnation = NonZeroU128::new(u128::from_le_bytes(nonce)).ok_or_else(|| io::Error::other("zero incarnation"))?;
    let mut client = BatchClient::bounded(
        BatchClientSettings::new(
            config.identity.digests()?,
            incarnation,
            first,
            config.endpoints.clone(),
            COMMANDS_PER_BATCH,
            FRAME_BYTES,
            config.max_outcome_bytes,
            UNAVAILABLE_REPLIES_PER_PEER,
        )
        .with_retry(BatchRetryConfig::for_raft_tick(DISCOVERY_RETRY_MS, config.raft_tick_millis))
        .with_operation_millis(OPERATION_TIMEOUT_MS)
        .with_lifetime_millis(BATCH_LIFETIME_MS),
        batch_monotonic_now,
    )
    .map_err(session_error)?;
    let mut driver = BatchDriverWorker::new(BatchTcpWorker::new().map_err(io::Error::other)?);
    let deadline = Instant::now() + WRITER_TIMEOUT;
    let mut batch = Some(batch);
    let outcome = loop {
        if Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "writer deadline; original remains pending"));
        }
        if matches!(client.status(), ClientStatus::Batch { connected: true, .. })
            && let Some(batch) = batch.take()
        {
            let admission = if recover { client.try_import(batch, committed) } else { client.try_submit(batch) };
            admission.map_err(|e| session_error(e.reason))?;
        }
        let event = match client.poll_event() {
            Some(BatchEvent::Progress(BatchProgress::Problem { cause, .. })) => {
                eprintln!("{}", session_error(cause));
                driver.stop(&mut client).map_err(session_error)?;
                client.poll_event()
            }
            event => event,
        };
        match event {
            Some(BatchEvent::Progress(BatchProgress::CommittedAwaitingApply { entry, .. })) => {
                journal.keep_commit(directory, entry)?;
            }
            Some(BatchEvent::Terminal(BatchTerminal::Applied { entry, results, problem, .. })) => {
                if matches!(results.as_slice(), [MemberResult::Unavailable { .. }]) {
                    return journal.unavailable(directory, entry, problem);
                }
                journal.keep_commit(directory, entry)?;
                if let Some(cause) = problem { return Err(session_error(cause)); }
                match <[_; 1]>::try_from(results) {
                    Ok([MemberResult::Outcome(outcome)]) => break outcome,
                    _ => return Err(io::Error::other("expected one member result for this deposit")),
                }
            }
            Some(BatchEvent::Terminal(BatchTerminal::ExactAppliedResultUnavailable { binding, problem, .. })) => {
                let entry = EntryBinding {
                    index: NonZeroU64::new(binding.raft_index()).ok_or_else(|| io::Error::other("zero applied index"))?,
                    term: NonZeroU64::new(binding.raft_term()).ok_or_else(|| io::Error::other("zero applied term"))?,
                    digest: binding.entry_digest(),
                };
                return journal.unavailable(directory, entry, problem);
            }
            Some(BatchEvent::Terminal(BatchTerminal::OutcomeUnknown { committed, cause, .. })) => {
                if let Some(entry) = committed { journal.keep_commit(directory, entry)?; }
                return Err(session_error(cause));
            }
            Some(BatchEvent::Terminal(terminal)) => return Err(io::Error::other(format!("cursor retained: {terminal:?}"))),
            _ => {}
        }
        if matches!(driver.step(&mut client).map_err(session_error)?, BatchDriverStep::Waiting) {
            driver.wait(&mut client, Duration::from_millis(10)).map_err(session_error)?;
        }
    };
    println!("{}", serde_json::json!({"applied":outcome, "sequence":sequence}));
    io::stdout().flush()?;
    // The upstream consumer explicitly acknowledges the result before the cursor moves.
    let mut reply = String::new();
    read_acknowledgement(&mut reply)?;
    if reply.trim_end() != format!("ack {sequence}") {
        return Err(io::Error::other("upstream acknowledgement missing; original remains pending"));
    }
    journal.cursor = sequence;
    journal.pending = None;
    journal.save(directory)?;
    println!("{}", serde_json::json!({"cursor":sequence}));
    Ok(())
}

fn run(args: &[String], read_acknowledgement: impl FnOnce(&mut String) -> io::Result<usize>) -> io::Result<ExitCode> {
    let [action, config, directory, tail @ ..] = args else { return Err(io::Error::other("use init, submit, recover or resolve with CONFIG DIR")); };
    let config: Deployment = serde_json::from_reader(File::open(config)?)?;
    let identity = config.identity.digests()?;
    let directory = Path::new(directory);
    if action == "init" && tail.is_empty() {
        fs::create_dir(directory)?;
        Journal { identity: config.identity, cursor: 0, pending: None, last_reconciliation: None }.save(directory)?;
        let parent = directory.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        File::open(parent)?.sync_all()?;
        return Ok(ExitCode::SUCCESS);
    }
    let lock = File::options().write(true).create(true).truncate(false).open(directory.join("writer.lock"))?;
    lock.try_lock().map_err(io::Error::other)?;
    let mut journal: Journal = serde_json::from_reader(File::open(directory.join("writer.json"))?)?;
    if journal.identity.digests()? != identity {
        eprintln!("saved_deployment_mismatch");
        return Ok(ExitCode::from(4));
    }
    match (action.as_str(), tail) {
        ("submit", [sequence, account, amount]) => {
            let sequence = number(sequence)?;
            if journal.pending.is_some() || journal.cursor.checked_add(1) != Some(sequence) {
                return Err(io::Error::other("submit requires the next sequence and an empty journal slot"));
            }
            journal.pending = Some(deposit(sequence, number(account)?, number(amount)?)?);
            journal.save(directory)?;
            drive(&config, directory, &mut journal, false, read_acknowledgement)?;
        }
        ("recover", []) => drive(&config, directory, &mut journal, true, read_acknowledgement)?,
        ("resolve", [receipt]) => {
            journal.resolve(directory, serde_json::from_reader(File::open(receipt)?)?)?;
            println!("{}", serde_json::json!({ "cursor": journal.cursor }));
        }
        _ => return Err(io::Error::other("expected init CONFIG DIR, submit CONFIG DIR SEQ ACCOUNT AMOUNT, recover CONFIG DIR, or resolve CONFIG DIR RECEIPT.json")),
    }
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    run(&args, |reply| io::stdin().read_line(reply)).unwrap_or_else(|error| {
        eprintln!("{error}");
        ExitCode::FAILURE
    })
}

#[cfg(all(test, unix))]
#[path = "bank_writer/recovery_tests.rs"]
mod recovery_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use test_case::test_case;

    fn pending_deposit() -> (Journal, EntryBinding) {
        let journal = Journal {
            identity: Identity {
                cluster_identity: "01".repeat(32),
                genesis_identity: "02".repeat(32),
                execution_lineage: "03".repeat(32),
            },
            cursor: 0,
            pending: Some(deposit(1, 7, 10).unwrap()),
            last_reconciliation: None,
        };
        let entry = EntryBinding {
            index: NonZeroU64::new(8).unwrap(),
            term: NonZeroU64::new(3).unwrap(),
            digest: EntryDigest::from_encoded_entry(b"first applied deposit"),
        };
        (journal, entry)
    }

    fn receipt(journal: &Journal, entry: EntryBinding) -> BusinessReceipt {
        BusinessReceipt {
            identity: journal.identity.clone(),
            input: journal.pending.as_ref().unwrap().first.clone(),
            entry: entry.into(),
            upstream_receipt: "bank-ledger/confirmed-decision-41".into(),
        }
    }

    #[test]
    fn a_legacy_pending_journal_needs_applied_evidence_before_a_receipt_can_advance_it() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("writer.json");
        let (mut journal, entry) = pending_deposit();
        journal.keep_commit(tmp.path(), entry).unwrap();
        let mut legacy = serde_json::to_value(&journal).unwrap();
        legacy["pending"].as_object_mut().unwrap().remove("exact_result_unavailable");
        fs::write(&path, legacy.to_string()).unwrap();
        let mut journal: Journal = serde_json::from_reader(File::open(&path).unwrap()).unwrap();
        let pending = fs::read(&path).unwrap();
        assert!(journal.resolve(tmp.path(), receipt(&journal, entry)).is_err());
        assert_eq!(fs::read(&path).unwrap(), pending);

        assert!(journal.unavailable(tmp.path(), entry, None).is_err());
        let evidence = serde_json::to_vec(&receipt(&journal, entry)).unwrap();
        let resolve = || serde_json::from_slice(&evidence).unwrap();
        let mut journal: Journal = serde_json::from_reader(File::open(&path).unwrap()).unwrap();
        journal.resolve(tmp.path(), resolve()).unwrap();
        let saved = fs::read(&path).unwrap();
        let mut reopened: Journal = serde_json::from_slice(&saved).unwrap();
        assert_eq!(reopened.cursor, 1);
        assert!(reopened.pending.is_none());
        assert_eq!(reopened.last_reconciliation.as_ref().unwrap().upstream_receipt, "bank-ledger/confirmed-decision-41");
        reopened.resolve(tmp.path(), resolve()).unwrap();
        assert_eq!(fs::read(path).unwrap(), saved);
    }

    #[test_case("/identity/cluster_identity", serde_json::json!("ff".repeat(32)); "other deployment")]
    #[test_case("/input/client_id", serde_json::json!(8); "other client")]
    #[test_case("/input/stream_id", serde_json::json!(10); "other stream")]
    #[test_case("/input/client_seq", serde_json::json!(2); "other sequence")]
    #[test_case("/entry/index", serde_json::json!(9); "other index")]
    #[test_case("/entry/term", serde_json::json!(4); "other term")]
    #[test_case("/entry/digest/0", serde_json::json!(255); "other digest")]
    #[test_case("/upstream_receipt", serde_json::json!("  "); "missing business receipt")]
    fn evidence_for_another_deposit_keeps_the_original_pending(field: &str, replacement: serde_json::Value) {
        let tmp = tempfile::tempdir().unwrap();
        let (mut journal, entry) = pending_deposit();
        assert!(journal.unavailable(tmp.path(), entry, None).is_err());
        let path = tmp.path().join("writer.json");
        let pending = fs::read(&path).unwrap();
        let mut evidence = serde_json::to_value(receipt(&journal, entry)).unwrap();
        let original = evidence.pointer_mut(field).unwrap();
        assert_ne!(*original, replacement);
        *original = replacement;
        assert!(journal.resolve(tmp.path(), serde_json::from_value(evidence).unwrap()).is_err());
        assert_eq!(fs::read(path).unwrap(), pending);
    }

    #[test_case("cluster_identity"; "cluster")]
    #[test_case("genesis_identity"; "genesis")]
    #[test_case("execution_lineage"; "lineage")]
    fn a_foreign_original_is_retained_before_connecting(field: &str) {
        let tmp = tempfile::tempdir().unwrap();
        let directory = tmp.path().join("writer");
        let config = tmp.path().join("deployment.json");
        let socket = TcpListener::bind("127.0.0.1:0").unwrap();
        socket.set_nonblocking(true).unwrap();
        let mut deployment = serde_json::json!({
            "identity": {"cluster_identity":"01".repeat(32), "genesis_identity":"02".repeat(32),
                         "execution_lineage":"03".repeat(32)},
            "endpoints":[[1,socket.local_addr().unwrap()]], "raft_tick_millis":15, "max_outcome_bytes":256
        });
        let invoke = |action: &str| {
            let args = [action.to_owned(), config.display().to_string(), directory.display().to_string()];
            run(&args, |_| panic!("foreign journal cannot reach upstream acknowledgement"))
        };
        fs::write(&config, serde_json::to_vec(&deployment).unwrap()).unwrap();
        assert_eq!(invoke("init").unwrap(), ExitCode::SUCCESS);
        let path = directory.join("writer.json");
        let mut journal: Journal = serde_json::from_reader(File::open(&path).unwrap()).unwrap();
        journal.pending = Some(deposit(1, 7, 10).unwrap());
        journal.save(&directory).unwrap();
        let original = fs::read(&path).unwrap();

        deployment["identity"][field] = "04".repeat(32).into();
        fs::write(&config, serde_json::to_vec(&deployment).unwrap()).unwrap();
        assert_eq!(invoke("recover").unwrap(), ExitCode::from(4));
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(socket.accept().unwrap_err().kind(), io::ErrorKind::WouldBlock);
        deployment["identity"].as_object_mut().unwrap().remove(field);
        fs::write(&config, serde_json::to_vec(&deployment).unwrap()).unwrap();
        assert!(invoke("recover").is_err());
        assert_eq!(fs::read(path).unwrap(), original);
        assert_eq!(socket.accept().unwrap_err().kind(), io::ErrorKind::WouldBlock);
    }
}

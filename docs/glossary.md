# StateVec SDK glossary

One concept uses one term. The qualified terms below identify distinct contracts.

| Term | Exact meaning and public expression | Must not imply |
| --- | --- | --- |
| Raft entry | One item in the replicated Raft log. It can contain several transactions or no transactions; it is the complete Projector delivery boundary. | One transaction, a network frame, or an exposed raw Raft protocol object. |
| Tx | One business transaction executed within an entry. `TxView` exposes its result, events and record deltas. A rejected Tx still has its result and empty output. | A whole Raft entry, or a guaranteed successful business operation. |
| Command | Input that requests a business operation. Existing `Command` names keep this meaning. | The result, its record deltas, or the whole replicated Raft entry. |
| Result | The reported outcome of an operation. `TxResult` describes a Tx outcome; Rust `Result<T, E>` distinguishes a returned value from a typed error. | Queue admission as final acceptance, or a business rejection as an ABI fault. |
| View | Read-only borrowed access to the named data. `RaftEntryView`, `TxView`, `RecordDeltaView` and `EventView` borrow data; their names identify what is read. | Ownership, a new copy, or consistency and durability guarantees beyond the named data's contract. |
| Delta | Output describing the effects at a specified Tx or Raft entry boundary. `TxDelta` contains the owned Tx output; `RecordDelta` contains one record's net change. | A byte-level patch or only a changed-field subset. Update and delete still include the full required before image. |
| Raft entry view | `RaftEntryView` reads the source position and complete ordered Tx outputs for one applied entry, including an empty progress entry. | Raw log bytes, an independently owned copy, or proof of application-side processing or saving. |
| Raft entry delivery | `RaftEntryDelivery` is the received item's lifetime handle. Its `view()` borrows the data; dropping it releases the native item. | Permanent ownership of record bytes or automatic durable acknowledgement. |
| Record delta | `RecordDelta::{Insert, Update, Delete}` owns one record's net change; `RecordDeltaView` borrows that same kind of data. | Changes accumulated across several transactions or a new record identity. |
| Update | The `Update` variant replaces an existing record and supplies its key, revision and complete before/after bytes. | Insert, delete, or a general name for every kind of delivery. |
| Event view | `EventView` borrows an event's ordinal, kind and encoded payload from the entry. | A domain-typed decoded event. |
| Typed event view | `statevec_test::TypedEventView` borrows a captured test event and exposes its generated domain accessor. | A separate decoded copy or a Projector delivery handle. |
| Raft entry token | Host-issued `RaftEntryToken`, obtained from `entry.token()`, binds the delivered entry to its exact source position for control requests. | A token manufactured from a log index, completed business processing, durability or host acceptance. |
| Source identity | `SourceIdentity` binds cluster, genesis, execution lineage and manifest. | A log position, active Core version or an ordering between different namespaces. |
| Source position | `SourcePosition` combines source identity, Raft index/term, complete applied-batch identity and transaction progress. | A live token or a recovery authorization. |
| Raft index | `RaftIndex` means the position of one entry in the Raft log; existing fields remain `raft_index`. | `TxSeq`, a record revision or a byte offset. No new wrapper type is required by this glossary. |
| Raft term | `RaftTerm` means the Raft election term associated with an entry; existing fields remain `raft_term`. | A transaction sequence or log position. No new wrapper type is required by this glossary. |
| Applied Tx progress | `AppliedTxProgress` contains the last applied transaction sequence and chain hash, including genesis progress. | A Raft index; one entry can contain multiple transactions or none. |
| Applied batch identity | `AppliedBatchIdentity` is the opaque identity of a native complete applied batch, including a validated compacted base. | The structured client batch binding below, a transaction sequence or a replaceable synonym for a position. |
| Applied Tx | `native::AppliedTx` reads one native-owned Tx result and output. | The borrowed plugin `TxView` or an unexecuted command. |
| Applied Raft entry | `native::AppliedRaftEntry` reads one native-owned complete entry output and its native entry token. | The plugin's borrowed `RaftEntryView` or delivery handle. |
| Native entry token | `native::NativeEntryToken` is the native trait for an entry's source token. Its declaration method is infallible and does not check the history mode. | The plugin `RaftEntryToken`, whose declaration checks continuous-history mode locally. |
| Native durable offset report | `native::NativeDurableOffsetReport` is the native report trait; the owner checks admissibility. Its offset can be absent. | A plugin `DurableOffsetReport`, which can only be constructed for continuous history. |
| Startup position | `StartupPosition` contains source identity, peer, applied Raft index/term and transaction progress at worker startup. | All fields of `SourcePosition`, or the later rebuild target. |
| Consume offset | `ConsumeOffset` is the persisted cluster/projector namespace and Raft term/index used for continuous-history recovery. `RaftEntryToken::is_at(&ConsumeOffset)` compares those four fields only. | Content equality, model equality or a live token. |
| History mode | `HistoryMode` selects `Continuous` or `LatestState` for one attachment. | Both modes running at once. |
| Supported history modes | `SupportedHistoryModes` declares `Continuous`, `LatestState` or `Both` for a plugin module. `Both` advertises both capabilities. | A mode selection or a third execution mode. |
| Start request | `StartRequest` is the prepared plugin's request: `Continuous { offset }` or `LatestState`. | A capability declaration, a chosen rebuild target or a native-only replay request. |
| Start instruction | `StartInstruction` is the host's `DeliverEntries` or `Rebuild { target }` instruction. | A target derived solely from a history mode. |
| Durable offset report | `DurableOffsetReport` declares that the application has durably saved effects and offset. `declare_durable()` constructs it only for continuous history. | Storage I/O, SDK verification of durability, queue admission or host acceptance. |
| Pending durable offset | `PendingDurableOffset` retains one exact report until its outcome, including exact retry after a lost reply. | Persistence, replacement with a newer report, or an automatic shutdown decision. |
| Offer / queued | Offering submits a control request; `Offer::Queued` means it entered the queue and owns a reply handle. | An accepted request or a storage receipt. |
| Accepted / rejected | `ControlDecision` and `ControlProgress` report the semantic owner's decision. `ProjectorControlRejection` preserves its typed rejection reason. | That the SDK persisted the application state, or that a transport error proves rejection. |
| Projector status | `ProjectorStatus` is a read-only sample of recovery, durable coverage or rebuild progress. | Permission to select an engine checkpoint. |
| Snapshot | A complete consistent data set at one specified source position. For Projector data, it contains all current records at one applied `SourcePosition`, regardless of when each record is read. | A mixture of current reads, an event history, a required file representation, durability or model installation. |
| Snapshot reader | `SnapshotReader` reads the frozen Snapshot incrementally. `SnapshotInfo` gives its position and record count; `SnapshotRecordView` borrows one record. | A required in-memory copy of all records or a read of the changing live engine. |
| Snapshot read | `SnapshotRead::{Record, Progress, End, Stop}` describes one bounded read. `Progress` means no record yet; only `End` means EOF. | Stop or a bounded empty read as successful completion. |
| Snapshot token | `SnapshotToken` is issued only after the complete Snapshot reaches true EOF and `finish()` succeeds. It binds the read Snapshot to its source position. | Model installation, a durable consume offset or host acceptance. |
| Snapshot confirmation | `PendingSnapshotConfirmation` retains the application's declaration that it installed the Snapshot. `offer_snapshot_confirmation()` queues it; the owner decides whether to release the rebuild barrier. | A request to validate arbitrary snapshot bytes, that reading EOF alone installs the model, or continuous offset reporting. |
| Rebuild | The action of reconstructing the application model from a Snapshot. The existing `Rebuild { target }` instruction and rebuild state/rejection names describe that action. | An alternate noun for the Snapshot or a required persistent save. |
| Engine checkpoint | The engine's persisted recovery image. Retained `EngineCheckpointPolicy` controls its capture/selection timing; it does not control application saves. | The Projector Snapshot reader or an application durable-offset report. |
| Raft snapshot | The Raft protocol's recovery/transfer object. Always qualify it as a Raft snapshot when the Projector data is also discussed. | The Projector record-reading handle, identical bytes or shared installation authority. |
| Error | A typed unsuccessful operation; selected Rust error types use `Error`, not the synonymous suffix `Failure`. `NotContinuousHistory` remains an explicit local condition name. | Erasing the cause, merging error families, or classifying every unavailable/pending outcome as an error. |
| Shared plugin error | `ConfigureError`, `HostError` and `ResourceError` are the configure, host-callback and resource error families shared by Core and Projector plugins. They are defined in `statevec_api::v1` and re-exported unchanged by `statevec_projector_api::v1`; they carry no `Core` or `Projector` qualifier. | A Core-only error, a Projector-only error, or an internal host fault outside the plugin ABI. |
| Unique key | A schema-declared lookup key, named `UniqueKey` in Rust identifiers. | `SysId`, record identity or a globally unique application identifier. |
| Business invariant | An application rule checked through `BusinessInvariantReadContext`; `Business` replaces `Biz`. | A host/ABI failure or permission to mutate through a read context. |
| Transaction sequence | `TxSeq` is the canonical execution sequence defined in statevec-model and re-exported by statevec-frame. `Tx` means transaction. | A client sequence or Raft index. |
| Client stream batch | `ClientStreamBatch` is one contiguous sequence range in one client stream. | An arbitrary multi-stream batch or every applied entry. |
| Client stream batch commitment | `ClientStreamBatchCommitment` binds that client's exact batch intent. | Proof of Raft commitment or application. |
| Applied stream batch binding | `applied_binding::AppliedStreamBatchBinding` contains the client stream batch commitment, Raft index/term, entry digest and ending engine progress. It is structured binding data, not `AppliedBatchIdentity`. | A historical result cache, an apply permit or equality with an opaque applied-batch identity. |
| Business code | `business_code` is the result field; zero and reserved framework codes retain their existing meaning. `BusinessRejectCode` remains the checked, narrower application-rejection range. | That every business code is a valid application rejection. |
| System code | `system_code` is the result field for the framework outcome. | The application's business rejection code. |

The selected abbreviations for this packet are `Tx` (transaction), `Seq`
(sequence, as in `TxSeq`), `Id`/`ID`
(identifier), `Io`/`IO` (input/output), `Abi`/`ABI` (application binary interface),
`FFI` (foreign function interface), `SDK` (software development kit), and
`EOF` (end of file), `Sys` (system, as in `SysId`), `IDL` (interface definition
language), `ns` (nanoseconds), and `ref` (reference, as in `ref_tx_time_ns`).
Existing schema scalar and protocol spellings retain their current meaning.

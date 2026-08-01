use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;

use crate::Journal;

/// A fact added earlier in the same guarded transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PendingFact {
    write: u64,
    index: usize,
}

static NEXT_WRITE: AtomicU64 = AtomicU64::new(1);

/// Causal predecessor assigned to a proposed fact.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FactCause {
    /// Start a causal chain.
    Root,
    /// Follow an already durable fact.
    Existing(wire::Seq),
    /// Follow an earlier fact in this transaction.
    Pending(PendingFact),
}

/// One guard evaluated before any proposed effect is written.
pub struct JournalGuard {
    check: Box<GuardCheck>,
}

type GuardCheck = dyn for<'a> FnOnce(&JournalRead<'a>) -> Result<(), GuardError> + Send + 'static;

impl JournalGuard {
    /// Capture a precondition closure for evaluation inside the write
    /// transaction.
    pub fn new<F>(check: F) -> Self
    where
        F: for<'a> FnOnce(&JournalRead<'a>) -> Result<(), GuardError> + Send + 'static,
    {
        Self {
            check: Box::new(check),
        }
    }
}

impl fmt::Debug for JournalGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("JournalGuard(..)")
    }
}

/// Read capability scoped to the transaction evaluating a guard.
#[derive(Clone, Copy, Debug)]
pub struct JournalRead<'a> {
    connection: &'a Connection,
    cursor: Option<wire::Seq>,
}

impl<'a> JournalRead<'a> {
    fn scoped(connection: &'a Connection) -> Result<Self, GuardError> {
        let cursor = visible_cursor(connection)?;
        Ok(Self { connection, cursor })
    }
}

/// One authoritative head body and the fact position that established it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JournalHead<T> {
    position: wire::Seq,
    body: T,
}

/// One task's complete current storage-head bundle.
///
/// `journal` proves that the indexed rows are canonical, consistently keyed,
/// and observed at one cursor. Policy validation of the bodies belongs to
/// `board`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TaskHeads {
    statement: JournalHead<wire::TaskHead>,
    workflow: JournalHead<wire::WorkflowHead>,
    claim: Option<JournalHead<wire::ClaimHead>>,
    lease: Option<JournalHead<wire::LeaseHead>>,
}

impl TaskHeads {
    /// Current task statement head and the position that established it.
    #[must_use]
    pub const fn statement(&self) -> &JournalHead<wire::TaskHead> {
        &self.statement
    }

    /// Current workflow head and the position that established it.
    #[must_use]
    pub const fn workflow(&self) -> &JournalHead<wire::WorkflowHead> {
        &self.workflow
    }

    /// Current claim head, absent before a task's first claim.
    #[must_use]
    pub const fn claim(&self) -> Option<&JournalHead<wire::ClaimHead>> {
        self.claim.as_ref()
    }

    /// Current lease head, absent before possession has minted one.
    #[must_use]
    pub const fn lease(&self) -> Option<&JournalHead<wire::LeaseHead>> {
        self.lease.as_ref()
    }

    /// Consume the storage view and return its typed bodies.
    #[must_use]
    pub fn into_bodies(
        self,
    ) -> (
        wire::TaskHead,
        wire::WorkflowHead,
        Option<wire::ClaimHead>,
        Option<wire::LeaseHead>,
    ) {
        (
            self.statement.into_body(),
            self.workflow.into_body(),
            self.claim.map(JournalHead::into_body),
            self.lease.map(JournalHead::into_body),
        )
    }
}

impl<T> JournalHead<T> {
    /// Fact position stored beside this head.
    #[must_use]
    pub const fn position(&self) -> wire::Seq {
        self.position
    }

    /// Typed current head body.
    #[must_use]
    pub const fn body(&self) -> &T {
        &self.body
    }

    /// Consume the read capability and keep its typed body.
    #[must_use]
    pub fn into_body(self) -> T {
        self.body
    }
}

impl JournalRead<'_> {
    /// Last durable fact position visible to this transaction.
    pub fn cursor(&self) -> Result<Option<wire::Seq>, GuardError> {
        Ok(self.cursor)
    }

    /// Every current task storage bundle in ascending handle order.
    ///
    /// The union makes loose workflow, claim, and lease rows observable rather
    /// than silently treating the statement table as the complete key set.
    pub fn task_heads(&self) -> Result<Vec<TaskHeads>, GuardError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT handle FROM (\
                    SELECT handle FROM task_heads UNION \
                    SELECT handle FROM workflow_heads UNION \
                    SELECT handle FROM claim_heads UNION \
                    SELECT handle FROM lease_heads\
                 ) ORDER BY handle",
            )
            .map_err(GuardError::Store)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(GuardError::Store)?;
        let mut bundles = Vec::new();
        for row in rows {
            let indexed = row.map_err(GuardError::Store)?;
            let handle = indexed.parse::<wire::TaskHandle>().map_err(|error| {
                GuardError::Invalid(format!("task head key {indexed:?} is invalid: {error}"))
            })?;
            let statement = self.task_head(&handle)?;
            let workflow = self.workflow_head(&handle)?;
            let claim = self.claim_head(&handle)?;
            let lease = self.lease_head(&handle)?;
            let loose_planes = [
                workflow.as_ref().map(|_| "workflow"),
                claim.as_ref().map(|_| "claim"),
                lease.as_ref().map(|_| "lease"),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            let statement = statement.ok_or_else(|| {
                GuardError::Invalid(format!(
                    "task {handle} has loose {} head(s) without a statement head",
                    loose_planes.join(", ")
                ))
            })?;
            let workflow = workflow.ok_or_else(|| {
                GuardError::Invalid(format!(
                    "task {handle} has a statement head but no workflow head"
                ))
            })?;
            bundles.push(TaskHeads {
                statement,
                workflow,
                claim,
                lease,
            });
        }
        Ok(bundles)
    }

    /// Current task statement head, when the handle exists.
    pub fn task_head(
        &self,
        handle: &wire::TaskHandle,
    ) -> Result<Option<JournalHead<wire::TaskHead>>, GuardError> {
        let head = self.text_head::<wire::TaskHead>("task_heads", "handle", handle.as_str())?;
        if let Some(head) = &head
            && head.body.handle != *handle
        {
            return Err(GuardError::Invalid(format!(
                "task head indexed as {handle} carries body handle {}",
                head.body.handle
            )));
        }
        if let Some(head) = &head
            && head.body.revision != head.position
        {
            return Err(GuardError::Invalid(format!(
                "task {handle} statement head revision disagrees with head_seq"
            )));
        }
        Ok(head)
    }

    /// Current workflow head, when the handle exists.
    pub fn workflow_head(
        &self,
        handle: &wire::TaskHandle,
    ) -> Result<Option<JournalHead<wire::WorkflowHead>>, GuardError> {
        self.text_head("workflow_heads", "handle", handle.as_str())
    }

    /// Current claim head, when the handle exists.
    pub fn claim_head(
        &self,
        handle: &wire::TaskHandle,
    ) -> Result<Option<JournalHead<wire::ClaimHead>>, GuardError> {
        let head = self.text_head::<wire::ClaimHead>("claim_heads", "handle", handle.as_str())?;
        if let Some(head) = &head
            && head.body.fact != head.position
        {
            return Err(GuardError::Invalid(format!(
                "task {handle} claim head fact disagrees with head_seq"
            )));
        }
        Ok(head)
    }

    /// Current lease head, when the handle exists.
    pub fn lease_head(
        &self,
        handle: &wire::TaskHandle,
    ) -> Result<Option<JournalHead<wire::LeaseHead>>, GuardError> {
        self.text_head("lease_heads", "handle", handle.as_str())
    }

    /// Current named witness head, when the predicate exists.
    pub fn witness_head(
        &self,
        name: &str,
    ) -> Result<Option<JournalHead<wire::WitnessHead>>, GuardError> {
        let head = self.text_head::<wire::WitnessHead>("witness_heads", "name", name)?;
        if let Some(head) = &head
            && head.body.head != head.position
        {
            return Err(GuardError::Invalid(
                "witness head fact disagrees with head_seq".to_owned(),
            ));
        }
        Ok(head)
    }

    /// Current trust head, when the digest exists.
    pub fn trust_head(
        &self,
        digest: &wire::Digest,
    ) -> Result<Option<JournalHead<wire::TrustHead>>, GuardError> {
        self.text_head("trust_heads", "digest", digest.as_str())
    }

    /// Current finding head, when the identity exists.
    pub fn finding_head(
        &self,
        identity: &str,
    ) -> Result<Option<JournalHead<wire::FindingHead>>, GuardError> {
        self.text_head("finding_heads", "identity", identity)
    }

    /// Current resident head, when an epoch has been seated.
    pub fn residency_head(&self) -> Result<Option<JournalHead<wire::ResidencyHead>>, GuardError> {
        let row = self
            .connection
            .query_row(
                "SELECT head_seq, body FROM residency_head WHERE singleton = 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()
            .map_err(GuardError::Store)?;
        row.map_or(Ok(None), |(position, bytes)| {
            let head = decode_head(position, &bytes).map_err(|error| {
                GuardError::Invalid(format!("residency head is invalid: {error}"))
            })?;
            if self.cursor.is_none_or(|cursor| head.position > cursor) {
                return Err(GuardError::Invalid(format!(
                    "residency head position {} is beyond cursor {:?}",
                    head.position,
                    self.cursor.map(wire::Seq::get)
                )));
            }
            Ok(Some(head))
        })
    }

    fn text_head<T>(
        &self,
        table: &'static str,
        key_column: &'static str,
        key: &str,
    ) -> Result<Option<JournalHead<T>>, GuardError>
    where
        T: serde::de::DeserializeOwned + Serialize,
    {
        let statement = format!("SELECT head_seq, body FROM {table} WHERE {key_column} = ?1");
        let row = self
            .connection
            .query_row(&statement, [key], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .optional()
            .map_err(GuardError::Store)?;
        row.map_or(Ok(None), |(position, bytes)| {
            let head = decode_head(position, &bytes).map_err(|error| {
                let detail = match error {
                    GuardError::Invalid(detail) => detail,
                    other => other.to_string(),
                };
                GuardError::Invalid(format!("{table} {key_column} {key:?} is invalid: {detail}"))
            })?;
            if self.cursor.is_none_or(|cursor| head.position > cursor) {
                return Err(GuardError::Invalid(format!(
                    "{table} {key_column} {key:?} head position {} is beyond cursor {:?}",
                    head.position,
                    self.cursor.map(wire::Seq::get)
                )));
            }
            Ok(Some(head))
        })
    }
}

fn decode_head<T>(position: i64, bytes: &[u8]) -> Result<JournalHead<T>, GuardError>
where
    T: serde::de::DeserializeOwned + Serialize,
{
    let head = wire::decode(bytes).map_err(|error| GuardError::Invalid(error.to_string()))?;
    let canonical = wire::encode(&head).map_err(|error| GuardError::Invalid(error.to_string()))?;
    if canonical != bytes {
        return Err(GuardError::Invalid(
            "head is not stored as canonical wire bytes".to_owned(),
        ));
    }
    let position = decode_seq(position).map_err(GuardError::Invalid)?;
    Ok(JournalHead {
        position,
        body: head,
    })
}

/// A named, expected precondition failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuardRefusal {
    reason: String,
}

impl GuardRefusal {
    /// Name the predicate that prevented this transaction.
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    /// Stable refusal text supplied by the guard.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for GuardRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl Error for GuardRefusal {}

/// A guard either refused the transition or could not read authority safely.
#[derive(Debug)]
pub enum GuardError {
    /// The observed authority did not satisfy the command predicate.
    Refused(GuardRefusal),
    /// SQLite could not evaluate the predicate.
    Store(rusqlite::Error),
    /// A stored value was outside the current journal contract.
    Invalid(String),
}

impl GuardError {
    /// Construct a named zero-write refusal.
    #[must_use]
    pub fn refused(reason: impl Into<String>) -> Self {
        Self::Refused(GuardRefusal::new(reason))
    }
}

impl fmt::Display for GuardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(refusal) => refusal.fmt(formatter),
            Self::Store(error) => write!(formatter, "guard read failed: {error}"),
            Self::Invalid(detail) => write!(formatter, "guard read invalid authority: {detail}"),
        }
    }
}

impl Error for GuardError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Refused(refusal) => Some(refusal),
            Self::Store(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}

/// A durable read failed in the journal or in the typed answer builder.
#[derive(Debug)]
pub enum ReadError<E> {
    Journal(GuardError),
    Answer(E),
}

impl<E: fmt::Display> fmt::Display for ReadError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Journal(error) => write!(formatter, "journal read failed: {error}"),
            Self::Answer(error) => write!(formatter, "journal answer refused: {error}"),
        }
    }
}

impl<E> Error for ReadError<E>
where
    E: Error + 'static,
{
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Journal(error) => Some(error),
            Self::Answer(error) => Some(error),
        }
    }
}

/// A complete guarded write proposed to the resident bus.
#[derive(Debug)]
pub struct JournalWrite {
    id: u64,
    actor: wire::Actor,
    origin: wire::Origin,
    at: wire::Instant,
    producer: wire::Producer,
    guards: Vec<JournalGuard>,
    facts: Vec<ProposedFact>,
    heads: Vec<HeadUpdate>,
    receipt: Option<PendingReceipt>,
}

impl JournalWrite {
    /// Begin a write attributed to one actor, origin, instant, and closed
    /// producer class.
    #[must_use]
    pub fn new(
        actor: wire::Actor,
        origin: wire::Origin,
        at: wire::Instant,
        producer: wire::Producer,
    ) -> Self {
        Self {
            id: NEXT_WRITE.fetch_add(1, Ordering::Relaxed),
            actor,
            origin,
            at,
            producer,
            guards: Vec::new(),
            facts: Vec::new(),
            heads: Vec::new(),
            receipt: None,
        }
    }

    /// Evaluate this precondition inside the eventual write transaction.
    pub fn guard(&mut self, guard: JournalGuard) {
        self.guards.push(guard);
    }

    /// Add one registered fact body and return its transaction-local identity.
    ///
    /// # Errors
    ///
    /// Refuses a fact whose registered producer differs from this write, or a
    /// body that cannot cross the current wire encoder.
    pub fn fact<F>(&mut self, body: F, cause: FactCause) -> Result<PendingFact, BuildError>
    where
        F: wire::Body,
    {
        if let FactCause::Pending(pending) = cause
            && pending.write != self.id
        {
            return Err(BuildError::ForeignPendingFact);
        }
        let fact = wire::Fact::new(body);
        if fact.producer() != self.producer {
            return Err(BuildError::WrongProducer {
                kind: fact.kind(),
                expected: self.producer,
                actual: fact.producer(),
            });
        }
        let encoded = wire::encode(&fact).map_err(BuildError::Wire)?;
        let fact = wire::decode::<wire::AnyFact>(&encoded).map_err(BuildError::Wire)?;
        let pending = PendingFact {
            write: self.id,
            index: self.facts.len(),
        };
        self.facts.push(ProposedFact { fact, cause });
        Ok(pending)
    }

    /// Replace the current task statement head at the assigned fact position.
    pub fn task_head<F>(&mut self, fact: PendingFact, build: F)
    where
        F: FnOnce(wire::Seq) -> wire::TaskHead + Send + 'static,
    {
        self.heads.push(HeadUpdate::new(fact, move |seq| {
            let head = build(seq);
            let key = head.handle.as_str().to_owned();
            if head.revision != seq {
                return Err("task head revision does not name its assigned fact".to_owned());
            }
            HeadRow::text("task_heads", "handle", key, seq, &head)
        }));
    }

    /// Replace the current workflow head at the assigned fact position.
    pub fn workflow_head<F>(&mut self, task: wire::TaskHandle, fact: PendingFact, build: F)
    where
        F: FnOnce(wire::Seq) -> wire::WorkflowHead + Send + 'static,
    {
        self.heads.push(HeadUpdate::new(fact, move |seq| {
            HeadRow::text(
                "workflow_heads",
                "handle",
                task.as_str().to_owned(),
                seq,
                &build(seq),
            )
        }));
    }

    /// Replace the current claim head at the assigned fact position.
    pub fn claim_head<F>(&mut self, task: wire::TaskHandle, fact: PendingFact, build: F)
    where
        F: FnOnce(wire::Seq) -> wire::ClaimHead + Send + 'static,
    {
        self.heads.push(HeadUpdate::new(fact, move |seq| {
            let head = build(seq);
            if head.fact != seq {
                return Err("claim head fact does not name its assigned fact".to_owned());
            }
            HeadRow::text(
                "claim_heads",
                "handle",
                task.as_str().to_owned(),
                seq,
                &head,
            )
        }));
    }

    /// Replace the current lease head at the assigned fact position.
    pub fn lease_head<F>(&mut self, task: wire::TaskHandle, fact: PendingFact, build: F)
    where
        F: FnOnce(wire::Seq) -> wire::LeaseHead + Send + 'static,
    {
        self.heads.push(HeadUpdate::new(fact, move |seq| {
            HeadRow::text(
                "lease_heads",
                "handle",
                task.as_str().to_owned(),
                seq,
                &build(seq),
            )
        }));
    }

    /// Replace one named witness head at the assigned fact position.
    pub fn witness_head<F>(&mut self, fact: PendingFact, build: F)
    where
        F: FnOnce(wire::Seq) -> wire::WitnessHead + Send + 'static,
    {
        self.heads.push(HeadUpdate::new(fact, move |seq| {
            let head = build(seq);
            let key = head.name.clone();
            if head.head != seq {
                return Err("witness head does not name its assigned fact".to_owned());
            }
            HeadRow::text("witness_heads", "name", key, seq, &head)
        }));
    }

    /// Replace one executable trust head at the assigned fact position.
    pub fn trust_head<F>(&mut self, fact: PendingFact, build: F)
    where
        F: FnOnce(wire::Seq) -> wire::TrustHead + Send + 'static,
    {
        self.heads.push(HeadUpdate::new(fact, move |seq| {
            let head = build(seq);
            HeadRow::text(
                "trust_heads",
                "digest",
                head.digest.as_str().to_owned(),
                seq,
                &head,
            )
        }));
    }

    /// Replace one gate finding head at the assigned fact position.
    pub fn finding_head<F>(&mut self, fact: PendingFact, build: F)
    where
        F: FnOnce(wire::Seq) -> wire::FindingHead + Send + 'static,
    {
        self.heads.push(HeadUpdate::new(fact, move |seq| {
            let head = build(seq);
            let key = head.identity.clone();
            HeadRow::text("finding_heads", "identity", key, seq, &head)
        }));
    }

    /// Replace the singleton resident head at the assigned fact position.
    pub fn residency_head<F>(&mut self, fact: PendingFact, build: F)
    where
        F: FnOnce(wire::Seq) -> wire::ResidencyHead + Send + 'static,
    {
        self.heads.push(HeadUpdate::new(fact, move |seq| {
            HeadRow::singleton("residency_head", seq, &build(seq))
        }));
    }

    /// Store one successful command result with exactly the witnesses the
    /// canonical request carried.
    ///
    /// # Errors
    ///
    /// Refuses a second receipt, a command outside its witness law, or a body
    /// that cannot cross the current wire encoder.
    pub fn command_result<B, R>(
        &mut self,
        command: wire::Command<B>,
        epoch: wire::CanonicalUuid,
        result: R,
    ) -> Result<(), BuildError>
    where
        B: Serialize,
        R: Serialize,
    {
        if self.receipt.is_some() {
            return Err(BuildError::DuplicateReceipt);
        }
        command.witnessed().map_err(BuildError::Law)?;
        let binding = command.kind.binding();
        let body = wire::encode(&command.body).map_err(BuildError::Wire)?;
        binding.body.decode(&body).map_err(BuildError::Wire)?;
        let result = wire::encode(&result).map_err(BuildError::Wire)?;
        let result = binding.answer.decode(&result).map_err(BuildError::Wire)?;
        let request = command.canonical_json().map_err(BuildError::Wire)?;
        let command = wire::decode::<wire::Command<serde_json::Value>>(request.as_bytes())
            .map_err(BuildError::Wire)?;
        if !command.v.is_current() {
            return Err(BuildError::Other(
                "command result must use the current wire version".to_owned(),
            ));
        }
        let result = serde_json::to_value(result).map_err(BuildError::Wire)?;
        self.receipt = Some(PendingReceipt {
            command,
            request: request.into_bytes(),
            epoch,
            result,
        });
        Ok(())
    }
}

#[derive(Debug)]
struct ProposedFact {
    fact: wire::AnyFact,
    cause: FactCause,
}

type HeadBuilder = dyn FnOnce(wire::Seq) -> Result<HeadRow, String> + Send + 'static;

struct HeadUpdate {
    fact: PendingFact,
    build: Box<HeadBuilder>,
}

impl HeadUpdate {
    fn new<F>(fact: PendingFact, build: F) -> Self
    where
        F: FnOnce(wire::Seq) -> Result<HeadRow, String> + Send + 'static,
    {
        Self {
            fact,
            build: Box::new(build),
        }
    }
}

impl fmt::Debug for HeadUpdate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HeadUpdate")
            .field("fact", &self.fact)
            .finish_non_exhaustive()
    }
}

enum HeadKey {
    Text(String),
    Singleton,
}

struct HeadRow {
    table: &'static str,
    key_column: &'static str,
    key: HeadKey,
    seq: wire::Seq,
    body: Vec<u8>,
}

impl HeadRow {
    fn text<T: Serialize>(
        table: &'static str,
        key_column: &'static str,
        key: String,
        seq: wire::Seq,
        body: &T,
    ) -> Result<Self, String> {
        if key.is_empty() {
            return Err(format!("{table} head key must not be empty"));
        }
        Ok(Self {
            table,
            key_column,
            key: HeadKey::Text(key),
            seq,
            body: wire::encode(body).map_err(|error| error.to_string())?,
        })
    }

    fn singleton<T: Serialize>(
        table: &'static str,
        seq: wire::Seq,
        body: &T,
    ) -> Result<Self, String> {
        Ok(Self {
            table,
            key_column: "singleton",
            key: HeadKey::Singleton,
            seq,
            body: wire::encode(body).map_err(|error| error.to_string())?,
        })
    }
}

#[derive(Debug)]
struct PendingReceipt {
    command: wire::Command<serde_json::Value>,
    request: Vec<u8>,
    epoch: wire::CanonicalUuid,
    result: serde_json::Value,
}

/// A proposal could not be made current-wire shaped before submission.
#[derive(Debug)]
pub enum BuildError {
    /// The fact belongs to another producer class.
    WrongProducer {
        kind: wire::FactKind,
        expected: wire::Producer,
        actual: wire::Producer,
    },
    /// The command's witnesses do not satisfy its closed registry row.
    Law(wire::LawError),
    /// A crossing value did not encode or decode.
    Wire(serde_json::Error),
    /// One command transaction proposed more than one receipt.
    DuplicateReceipt,
    /// A transaction-local fact identity came from another proposal.
    ForeignPendingFact,
    /// Another proposal invariant was not met.
    Other(String),
}

impl fmt::Display for BuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongProducer {
                kind,
                expected,
                actual,
            } => write!(
                formatter,
                "fact {kind} belongs to {actual}, not proposed producer {expected}"
            ),
            Self::Law(error) => error.fmt(formatter),
            Self::Wire(error) => write!(formatter, "journal proposal is not current wire: {error}"),
            Self::DuplicateReceipt => {
                formatter.write_str("one guarded transaction stores at most one command receipt")
            }
            Self::ForeignPendingFact => {
                formatter.write_str("pending fact belongs to another journal transaction")
            }
            Self::Other(detail) => formatter.write_str(detail),
        }
    }
}

impl Error for BuildError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Law(error) => Some(error),
            Self::Wire(error) => Some(error),
            _ => None,
        }
    }
}

/// Durable range returned only after SQLite committed the whole proposal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Applied {
    first: wire::Seq,
    last: wire::Seq,
}

impl Applied {
    /// First fact assigned by this transaction.
    #[must_use]
    pub const fn first(&self) -> wire::Seq {
        self.first
    }

    /// Last fact assigned by this transaction.
    #[must_use]
    pub const fn last(&self) -> wire::Seq {
        self.last
    }
}

/// A guarded transaction either refused before effects or faulted and rolled
/// them back.
#[derive(Debug)]
pub enum CommitError {
    /// An expected predicate or proposal shape was not satisfied.
    Refused(GuardRefusal),
    /// Storage or canonical encoding prevented a durable commit.
    Fault(CommitFault),
}

impl fmt::Display for CommitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(refusal) => write!(formatter, "journal commit refused: {refusal}"),
            Self::Fault(fault) => fault.fmt(formatter),
        }
    }
}

impl Error for CommitError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Refused(refusal) => Some(refusal),
            Self::Fault(fault) => Some(fault),
        }
    }
}

/// A store or encoding defect that left the transaction with zero writes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommitFault {
    detail: String,
}

impl CommitFault {
    fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }

    /// Actionable storage or encoding detail.
    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for CommitFault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "journal commit fault: {}", self.detail)
    }
}

impl Error for CommitFault {}

impl Journal {
    /// Evaluate a typed answer whose own error remains distinct from journal
    /// faults while both are observed inside one immutable SQLite snapshot.
    pub fn read_with<T, E, F>(&self, answer: F) -> Result<T, ReadError<E>>
    where
        F: for<'a> FnOnce(&JournalRead<'a>) -> Result<T, E>,
    {
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(GuardError::Store)
            .map_err(ReadError::Journal)?;
        let read = JournalRead::scoped(&transaction).map_err(ReadError::Journal)?;
        let answer = answer(&read).map_err(ReadError::Answer)?;
        transaction
            .commit()
            .map_err(GuardError::Store)
            .map_err(ReadError::Journal)?;
        Ok(answer)
    }

    /// Evaluate an owned answer against one immutable SQLite snapshot.
    ///
    /// The cursor query is deliberately the first read in the deferred
    /// transaction. It establishes the SQLite snapshot that every subsequent
    /// query through `JournalRead` observes.
    pub fn read<T, F>(&self, answer: F) -> Result<T, GuardError>
    where
        F: for<'a> FnOnce(&JournalRead<'a>) -> Result<T, GuardError>,
    {
        self.read_with(answer).map_err(|error| match error {
            ReadError::Journal(error) | ReadError::Answer(error) => error,
        })
    }

    /// Evaluate guards and apply every proposed effect in one immediate SQLite
    /// transaction.
    pub fn apply(&mut self, mut write: JournalWrite) -> Result<Applied, CommitError> {
        if write.facts.is_empty() {
            return Err(CommitError::Refused(GuardRefusal::new(
                "a mutating journal transaction must append at least one fact",
            )));
        }

        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_fault)?;
        let current = read_cursor(&transaction)?;
        run_guards(&transaction, current, std::mem::take(&mut write.guards))?;
        let assigned = assign_positions(current, write.facts.len())?;
        insert_facts(&transaction, &write, &assigned, current)?;
        apply_heads(&transaction, write.id, write.heads, &assigned)?;
        if let Some(receipt) = write.receipt {
            insert_receipt(
                &transaction,
                &write.actor,
                receipt,
                *assigned.first().expect("nonempty fact assignment"),
                *assigned.last().expect("nonempty fact assignment"),
            )?;
        }
        let last = *assigned.last().expect("nonempty fact assignment");
        transaction
            .execute(
                "UPDATE cursor_head SET seq = ?1 WHERE singleton = 1",
                [to_sql(last)?],
            )
            .map_err(sqlite_fault)?;
        transaction.commit().map_err(sqlite_fault)?;
        Ok(Applied {
            first: assigned[0],
            last,
        })
    }
}

fn run_guards(
    transaction: &Transaction<'_>,
    cursor: Option<wire::Seq>,
    guards: Vec<JournalGuard>,
) -> Result<(), CommitError> {
    let read = JournalRead {
        connection: transaction,
        cursor,
    };
    for guard in guards {
        match (guard.check)(&read) {
            Ok(()) => {}
            Err(GuardError::Refused(refusal)) => return Err(CommitError::Refused(refusal)),
            Err(error) => return Err(CommitError::Fault(CommitFault::new(error.to_string()))),
        }
    }
    Ok(())
}

fn read_cursor(connection: &Connection) -> Result<Option<wire::Seq>, CommitError> {
    visible_cursor(connection)
        .map_err(|error| CommitError::Fault(CommitFault::new(error.to_string())))
}

fn visible_cursor(connection: &Connection) -> Result<Option<wire::Seq>, GuardError> {
    let value = connection
        .query_row(
            "SELECT seq FROM cursor_head WHERE singleton = 1",
            [],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(GuardError::Store)?;
    decode_optional_seq(value).map_err(GuardError::Invalid)
}

fn assign_positions(
    current: Option<wire::Seq>,
    count: usize,
) -> Result<Vec<wire::Seq>, CommitError> {
    let start = match current {
        None => 1,
        Some(seq) => seq.get().checked_add(1).ok_or_else(|| {
            CommitError::Fault(CommitFault::new("journal position range exhausted"))
        })?,
    };
    (0..count)
        .map(|offset| {
            let offset = u64::try_from(offset).map_err(|_| {
                CommitError::Fault(CommitFault::new(
                    "fact count exceeds journal position range",
                ))
            })?;
            start
                .checked_add(offset)
                .and_then(wire::Seq::new)
                .ok_or_else(|| {
                    CommitError::Fault(CommitFault::new("journal position range exhausted"))
                })
        })
        .collect()
}

fn insert_facts(
    transaction: &Transaction<'_>,
    write: &JournalWrite,
    assigned: &[wire::Seq],
    current: Option<wire::Seq>,
) -> Result<(), CommitError> {
    for (index, proposed) in write.facts.iter().enumerate() {
        let seq = assigned[index];
        let cause = resolve_cause(proposed.cause, write.id, index, assigned, current)?;
        let envelope = wire::Envelope {
            v: wire::WireVersion::CURRENT,
            seq,
            at: write.at,
            actor: write.actor.clone(),
            origin: write.origin.clone(),
            cause,
            fact: proposed.fact.clone(),
        };
        let bytes = wire::encode(&envelope).map_err(wire_fault)?;
        transaction
            .execute(
                "INSERT INTO fact_envelopes(seq, wire_version, instant, actor, origin, cause, kind, envelope) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    to_sql(seq)?,
                    i64::from(wire::WireVersion::CURRENT.0),
                    write.at.to_string(),
                    write.actor.to_string(),
                    write.origin.as_str(),
                    cause.map(to_sql).transpose()?,
                    proposed.fact.kind().as_str(),
                    bytes,
                ],
            )
            .map_err(sqlite_fault)?;
        insert_thread_index(transaction, seq, cause, write.origin.as_str())?;
    }
    Ok(())
}

fn resolve_cause(
    cause: FactCause,
    write: u64,
    index: usize,
    assigned: &[wire::Seq],
    current: Option<wire::Seq>,
) -> Result<Option<wire::Seq>, CommitError> {
    match cause {
        FactCause::Root => Ok(None),
        FactCause::Existing(seq) if current.is_some_and(|cursor| seq <= cursor) => Ok(Some(seq)),
        FactCause::Existing(seq) => Err(CommitError::Refused(GuardRefusal::new(format!(
            "cause {seq} is not already durable"
        )))),
        FactCause::Pending(pending) if pending.write != write => Err(CommitError::Refused(
            GuardRefusal::new("pending cause belongs to another journal transaction"),
        )),
        FactCause::Pending(pending) if pending.index < index => Ok(Some(assigned[pending.index])),
        FactCause::Pending(pending) => Err(CommitError::Refused(GuardRefusal::new(format!(
            "pending cause {} must precede fact {index}",
            pending.index
        )))),
    }
}

fn insert_thread_index(
    transaction: &Transaction<'_>,
    seq: wire::Seq,
    cause: Option<wire::Seq>,
    origin: &str,
) -> Result<(), CommitError> {
    let (root, depth) = match cause {
        None => (seq, 0_i64),
        Some(cause) => transaction
            .query_row(
                "SELECT root, depth, origin FROM thread_index WHERE seq = ?1",
                [to_sql(cause)?],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .map_err(sqlite_fault)
            .and_then(|(root, depth, parent_origin)| {
                if parent_origin != origin {
                    return Err(CommitError::Refused(GuardRefusal::new(
                        "a fact cause cannot cross origin boundaries",
                    )));
                }
                let root = decode_seq(root).map_err(|detail| {
                    CommitError::Fault(CommitFault::new(format!(
                        "thread root is invalid: {detail}"
                    )))
                })?;
                let depth = depth.checked_add(1).ok_or_else(|| {
                    CommitError::Fault(CommitFault::new("thread depth exhausted"))
                })?;
                Ok((root, depth))
            })?,
    };
    transaction
        .execute(
            "INSERT INTO thread_index(seq, origin, cause, root, depth) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                to_sql(seq)?,
                origin,
                cause.map(to_sql).transpose()?,
                to_sql(root)?,
                depth,
            ],
        )
        .map_err(sqlite_fault)?;
    Ok(())
}

fn apply_heads(
    transaction: &Transaction<'_>,
    write: u64,
    heads: Vec<HeadUpdate>,
    assigned: &[wire::Seq],
) -> Result<(), CommitError> {
    for head in heads {
        if head.fact.write != write {
            return Err(CommitError::Refused(GuardRefusal::new(
                "head names a fact from another journal transaction",
            )));
        }
        let seq = assigned.get(head.fact.index).copied().ok_or_else(|| {
            CommitError::Refused(GuardRefusal::new(format!(
                "head names missing pending fact {}",
                head.fact.index
            )))
        })?;
        let row = (head.build)(seq).map_err(|detail| {
            CommitError::Refused(GuardRefusal::new(format!("head update refused: {detail}")))
        })?;
        let statement = format!(
            "INSERT INTO {table}({key}, head_seq, body) VALUES (?1, ?2, ?3) ON CONFLICT({key}) DO UPDATE SET head_seq = excluded.head_seq, body = excluded.body",
            table = row.table,
            key = row.key_column,
        );
        match row.key {
            HeadKey::Text(key) => transaction
                .execute(&statement, params![key, to_sql(row.seq)?, row.body])
                .map_err(sqlite_fault)?,
            HeadKey::Singleton => transaction
                .execute(&statement, params![1_i64, to_sql(row.seq)?, row.body])
                .map_err(sqlite_fault)?,
        };
    }
    Ok(())
}

fn insert_receipt(
    transaction: &Transaction<'_>,
    actor: &wire::Actor,
    receipt: PendingReceipt,
    first: wire::Seq,
    last: wire::Seq,
) -> Result<(), CommitError> {
    let response = wire::Response {
        v: wire::WireVersion::CURRENT,
        id: receipt.command.id,
        epoch: receipt.epoch,
        outcome: wire::Outcome::Reply(wire::Reply {
            position: wire::ReplyPosition::Committed(wire::SeqRange { first, last }),
            body: receipt.result,
        }),
    };
    let witnesses = wire::encode(&receipt.command.expected).map_err(wire_fault)?;
    let response = wire::encode(&response).map_err(wire_fault)?;
    transaction
        .execute(
            "INSERT INTO command_receipts(command_id, actor, command_kind, request, evaluated_witnesses, response, first_seq, last_seq, snapshot_seq, epoch) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL, ?9)",
            params![
                receipt.command.id.to_string(),
                actor.to_string(),
                receipt.command.kind.as_str(),
                receipt.request,
                witnesses,
                response,
                to_sql(first)?,
                to_sql(last)?,
                receipt.epoch.to_string(),
            ],
        )
        .map_err(sqlite_fault)?;
    Ok(())
}

fn decode_optional_seq(value: Option<i64>) -> Result<Option<wire::Seq>, String> {
    value.map(decode_seq).transpose()
}

fn decode_seq(value: i64) -> Result<wire::Seq, String> {
    u64::try_from(value)
        .ok()
        .and_then(wire::Seq::new)
        .ok_or_else(|| format!("{value} is not a positive wire position"))
}

fn to_sql(seq: wire::Seq) -> Result<i64, CommitError> {
    i64::try_from(seq.get()).map_err(|_| {
        CommitError::Fault(CommitFault::new(format!(
            "journal position {seq} exceeds SQLite integer range"
        )))
    })
}

fn sqlite_fault(error: rusqlite::Error) -> CommitError {
    CommitError::Fault(CommitFault::new(error.to_string()))
}

fn wire_fault(error: serde_json::Error) -> CommitError {
    CommitError::Fault(CommitFault::new(error.to_string()))
}

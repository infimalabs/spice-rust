use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;

use crate::Journal;
use wire::body::SourceBoundary;

mod claim_index;
mod read_metrics;
mod steering_index;

#[cfg(test)]
pub(crate) use read_metrics::{
    IndexedReadMetrics, indexed_read_metrics, reset_indexed_read_metrics,
};
pub(crate) use steering_index::{canonical_identity, disposition_name};

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

/// Indexed starting boundary for one bounded fact page.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FactPageStart {
    /// Begin strictly after this cursor, or at the first fact when absent.
    After(Option<wire::Seq>),
    /// Begin at this exact continuation cursor.
    At(wire::Seq),
}

impl<'a> JournalRead<'a> {
    fn scoped(connection: &'a Connection) -> Result<Self, GuardError> {
        let cursor = visible_cursor(connection)?;
        Ok(Self { connection, cursor })
    }

    /// Read the canonical stream inside this guard's immutable transaction.
    pub fn fact_snapshot(&self) -> Result<crate::FactSnapshot, crate::SnapshotError> {
        crate::read::read_snapshot(self.connection)
    }

    /// Verify every authority plane inside this exact immutable snapshot.
    pub fn verify(&self) -> Result<crate::Verification, crate::VerifyError> {
        crate::replay::verify_connection(self.connection)
    }

    /// Resolve one causal thread inside this exact immutable snapshot.
    pub fn thread(&self, reference: &str) -> Result<crate::ThreadSnapshot, crate::ThreadError> {
        crate::replay::thread_connection(self.connection, reference)
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
    /// Pair one replayed body with the fact position that established it.
    #[must_use]
    pub const fn new(position: wire::Seq, body: T) -> Self {
        Self { position, body }
    }

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

    /// Read one bounded primary-key fact range through an immutable horizon.
    pub fn fact_page(
        &self,
        start: FactPageStart,
        horizon: wire::Seq,
        limit: usize,
    ) -> Result<wire::Packed<wire::Envelope<wire::AnyFact>>, GuardError> {
        if limit == 0 {
            return Err(GuardError::Invalid(
                "indexed fact page limit must be positive".to_owned(),
            ));
        }
        let Some(cursor) = self.cursor else {
            return Err(GuardError::Invalid(
                "indexed fact page requires a durable cursor".to_owned(),
            ));
        };
        if horizon > cursor {
            return Err(GuardError::Invalid(format!(
                "indexed fact page horizon {horizon} is beyond cursor {cursor}"
            )));
        }
        let first = match start {
            FactPageStart::After(None) => 1,
            FactPageStart::After(Some(after)) => after.get().saturating_add(1),
            FactPageStart::At(position) => position.get(),
        };
        if first > horizon.get() {
            return Ok(wire::Packed {
                rows: Vec::new(),
                overflow: None,
                horizon,
            });
        }
        let total = horizon.get() - first + 1;
        let query_limit = limit.saturating_add(1);
        let first = i64::try_from(first)
            .map_err(|_| GuardError::Invalid("indexed fact start exceeds SQLite".to_owned()))?;
        let horizon_sql = i64::try_from(horizon.get())
            .map_err(|_| GuardError::Invalid("indexed fact horizon exceeds SQLite".to_owned()))?;
        let query_limit = i64::try_from(query_limit)
            .map_err(|_| GuardError::Invalid("indexed fact limit exceeds SQLite".to_owned()))?;
        read_metrics::observe_query();
        let mut statement = self
            .connection
            .prepare(
                "SELECT seq, kind, envelope FROM fact_envelopes \
                 WHERE seq >= ?1 AND seq <= ?2 ORDER BY seq LIMIT ?3",
            )
            .map_err(GuardError::Store)?;
        let rows = statement
            .query_map(params![first, horizon_sql, query_limit], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(GuardError::Store)?;
        let expected = usize::try_from(total)
            .unwrap_or(usize::MAX)
            .min(limit.saturating_add(1));
        let mut facts = Vec::with_capacity(expected);
        for row in rows {
            let (seq, kind, bytes) = row.map_err(GuardError::Store)?;
            read_metrics::observe_row();
            read_metrics::observe_decode(bytes.len());
            facts.push(
                crate::read::decode_fact(seq, &kind, &bytes)
                    .map_err(|error| GuardError::Invalid(error.to_string()))?,
            );
        }
        if facts.len() != expected {
            return Err(GuardError::Invalid(format!(
                "indexed fact range expected {expected} row(s), found {}",
                facts.len()
            )));
        }
        let overflow = if total > u64::try_from(limit).unwrap_or(u64::MAX) {
            let continuation = facts
                .pop()
                .expect("a range beyond the limit returns its continuation")
                .seq;
            Some(wire::Overflow {
                omitted: total - u64::try_from(limit).unwrap_or(u64::MAX),
                continuation,
            })
        } else {
            None
        };
        Ok(wire::Packed {
            rows: facts,
            overflow,
            horizon,
        })
    }

    /// Walk one bounded causal page through the durable thread index.
    pub fn causal_page(
        &self,
        position: wire::Seq,
        limit: usize,
    ) -> Result<Option<wire::Packed<wire::Envelope<wire::AnyFact>>>, GuardError> {
        if limit == 0 {
            return Err(GuardError::Invalid(
                "indexed causal page limit must be positive".to_owned(),
            ));
        }
        let Some(horizon) = self.cursor else {
            return Ok(None);
        };
        let position_sql = i64::try_from(position.get())
            .map_err(|_| GuardError::Invalid("causal position exceeds SQLite".to_owned()))?;
        let query_limit = limit.saturating_add(1);
        let query_limit_sql = i64::try_from(query_limit)
            .map_err(|_| GuardError::Invalid("causal page limit exceeds SQLite".to_owned()))?;
        read_metrics::observe_query();
        let mut statement = self
            .connection
            .prepare(
                "WITH RECURSIVE chain(seq, origin, cause, remaining, step) AS (\
                     SELECT seq, origin, cause, depth, 0 FROM thread_index WHERE seq = ?1 \
                     UNION ALL \
                     SELECT indexed.seq, indexed.origin, indexed.cause, indexed.depth, chain.step + 1 \
                     FROM thread_index AS indexed JOIN chain ON indexed.seq = chain.cause \
                     WHERE chain.step + 1 < ?2\
                 ) \
                 SELECT chain.seq, chain.origin, chain.cause, chain.remaining, \
                        fact.kind, fact.envelope \
                 FROM chain JOIN fact_envelopes AS fact ON fact.seq = chain.seq \
                 ORDER BY chain.step LIMIT ?2",
            )
            .map_err(GuardError::Store)?;
        let rows = statement
            .query_map(params![position_sql, query_limit_sql], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Vec<u8>>(5)?,
                ))
            })
            .map_err(GuardError::Store)?;
        let mut facts = Vec::with_capacity(query_limit);
        let mut total = None::<u64>;
        let mut chain_origin = None;
        for row in rows {
            let (seq, origin, cause, remaining, kind, bytes) = row.map_err(GuardError::Store)?;
            read_metrics::observe_row();
            read_metrics::observe_decode(bytes.len());
            let fact = crate::read::decode_fact(seq, &kind, &bytes)
                .map_err(|error| GuardError::Invalid(error.to_string()))?;
            let indexed_cause = cause
                .map(|cause| {
                    u64::try_from(cause)
                        .ok()
                        .and_then(wire::Seq::new)
                        .ok_or_else(|| {
                            GuardError::Invalid(format!(
                                "thread index cause {cause} is outside the wire range"
                            ))
                        })
                })
                .transpose()?;
            if fact.origin.as_str() != origin || fact.cause != indexed_cause {
                return Err(GuardError::Invalid(format!(
                    "thread index at {} disagrees with its canonical envelope",
                    fact.seq
                )));
            }
            if chain_origin
                .as_ref()
                .is_some_and(|chain_origin| chain_origin != &fact.origin)
            {
                return Err(GuardError::Invalid(format!(
                    "thread index at {} crosses causal origins",
                    fact.seq
                )));
            }
            chain_origin.get_or_insert_with(|| fact.origin.clone());
            let remaining = u64::try_from(remaining).map_err(|_| {
                GuardError::Invalid(format!(
                    "thread index depth {remaining} is outside the wire range"
                ))
            })?;
            if let Some(total) = total {
                let step = u64::try_from(facts.len()).unwrap_or(u64::MAX);
                let expected_remaining = total.saturating_sub(step.saturating_add(1));
                if remaining != expected_remaining {
                    return Err(GuardError::Invalid(format!(
                        "thread index at {} has depth {remaining}, expected {expected_remaining}",
                        fact.seq
                    )));
                }
            } else {
                total = Some(remaining.saturating_add(1));
            }
            facts.push(fact);
        }
        let Some(total) = total else {
            return Ok(None);
        };
        let expected = usize::try_from(total)
            .unwrap_or(usize::MAX)
            .min(query_limit);
        if facts.len() != expected {
            return Err(GuardError::Invalid(format!(
                "indexed causal range expected {expected} row(s), found {}",
                facts.len()
            )));
        }
        let overflow = if total > u64::try_from(limit).unwrap_or(u64::MAX) {
            let continuation = facts
                .pop()
                .expect("a causal range beyond the limit returns its continuation")
                .seq;
            Some(wire::Overflow {
                omitted: total - u64::try_from(limit).unwrap_or(u64::MAX),
                continuation,
            })
        } else {
            None
        };
        Ok(Some(wire::Packed {
            rows: facts,
            overflow,
            horizon,
        }))
    }

    /// Every current task storage bundle in ascending inception order.
    ///
    /// The union makes loose workflow, claim, and lease rows observable rather
    /// than silently treating the statement table as the complete key set.
    pub fn task_heads(&self) -> Result<Vec<TaskHeads>, GuardError> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT stamp FROM (\
                    SELECT stamp FROM task_heads UNION \
                    SELECT stamp FROM workflow_heads UNION \
                    SELECT stamp FROM claim_heads UNION \
                    SELECT stamp FROM lease_heads\
                 ) ORDER BY stamp",
            )
            .map_err(GuardError::Store)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(GuardError::Store)?;
        let mut bundles = Vec::new();
        for row in rows {
            let indexed = row.map_err(GuardError::Store)?;
            let stamp = indexed.parse::<wire::TaskStamp>().map_err(|error| {
                GuardError::Invalid(format!("task head key {indexed:?} is invalid: {error}"))
            })?;
            let statement = self.task_head(&stamp)?;
            let workflow = self.workflow_head(&stamp)?;
            let claim = self.claim_head(&stamp)?;
            let lease = self.lease_head(&stamp)?;
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
                    "task {stamp} has loose {} head(s) without a statement head",
                    loose_planes.join(", ")
                ))
            })?;
            let workflow = workflow.ok_or_else(|| {
                GuardError::Invalid(format!(
                    "task {stamp} has a statement head but no workflow head"
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

    /// Current task statement head, when the inception stamp exists.
    pub fn task_head(
        &self,
        stamp: &wire::TaskStamp,
    ) -> Result<Option<JournalHead<wire::TaskHead>>, GuardError> {
        let head = self.text_head::<wire::TaskHead>("task_heads", "stamp", stamp.as_str())?;
        if let Some(head) = &head
            && head.body.born != *stamp
        {
            return Err(GuardError::Invalid(format!(
                "task head indexed as {stamp} carries body stamp {}",
                head.body.born
            )));
        }
        if let Some(head) = &head
            && head.body.revision != head.position
        {
            return Err(GuardError::Invalid(format!(
                "task {stamp} statement head revision disagrees with head_seq"
            )));
        }
        Ok(head)
    }

    /// Current workflow head, when the inception stamp exists.
    pub fn workflow_head(
        &self,
        stamp: &wire::TaskStamp,
    ) -> Result<Option<JournalHead<wire::WorkflowHead>>, GuardError> {
        self.text_head("workflow_heads", "stamp", stamp.as_str())
    }

    /// Current claim head, when the inception stamp exists.
    pub fn claim_head(
        &self,
        stamp: &wire::TaskStamp,
    ) -> Result<Option<JournalHead<wire::ClaimHead>>, GuardError> {
        let head = self.text_head::<wire::ClaimHead>("claim_heads", "stamp", stamp.as_str())?;
        if let Some(head) = &head
            && head.body.fact != head.position
        {
            return Err(GuardError::Invalid(format!(
                "task {stamp} claim head fact disagrees with head_seq"
            )));
        }
        Ok(head)
    }

    /// Current lease head, when the inception stamp exists.
    pub fn lease_head(
        &self,
        stamp: &wire::TaskStamp,
    ) -> Result<Option<JournalHead<wire::LeaseHead>>, GuardError> {
        self.text_head("lease_heads", "stamp", stamp.as_str())
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
        let head = self.text_head::<wire::TrustHead>("trust_heads", "digest", digest.as_str())?;
        if let Some(head) = &head
            && head.body.digest != *digest
        {
            return Err(GuardError::Invalid(format!(
                "trust head indexed as {digest} carries body digest {}",
                head.body.digest
            )));
        }
        Ok(head)
    }

    /// Every current executable trust head in ascending digest order.
    pub fn trust_heads(&self) -> Result<Vec<JournalHead<wire::TrustHead>>, GuardError> {
        let mut statement = self
            .connection
            .prepare("SELECT digest FROM trust_heads ORDER BY digest")
            .map_err(GuardError::Store)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(GuardError::Store)?;
        let mut heads = Vec::new();
        for row in rows {
            let indexed = row.map_err(GuardError::Store)?;
            let digest = indexed.parse::<wire::Digest>().map_err(|error| {
                GuardError::Invalid(format!("trust head key {indexed:?} is invalid: {error}"))
            })?;
            let head = self.trust_head(&digest)?.ok_or_else(|| {
                GuardError::Invalid(format!("trust head indexed as {digest} disappeared"))
            })?;
            heads.push(head);
        }
        Ok(heads)
    }

    /// Current finding head, when the identity exists.
    pub fn finding_head(
        &self,
        identity: &str,
    ) -> Result<Option<JournalHead<wire::FindingHead>>, GuardError> {
        self.text_head("finding_heads", "identity", identity)
    }

    /// Current transcript source head, when that stream has been accounted.
    ///
    /// Addressed by [`wire::TranscriptStreamKey`] rather than by a stream name,
    /// because the row is keyed by lane, vendor and stream together. A bare
    /// stream name matches no row, and this lookup reports no row as `Ok(None)`
    /// -- the same answer a genuinely unaccounted stream gives. Passing one is
    /// not merely discouraged, it does not compile:
    ///
    /// ```compile_fail
    /// fn catch_up(read: &journal::JournalRead<'_>) {
    ///     let _ = read.transcript_source_head("sidecar");
    /// }
    /// ```
    pub fn transcript_source_head(
        &self,
        stream: &wire::TranscriptStreamKey,
    ) -> Result<Option<JournalHead<wire::TranscriptSourceHead>>, GuardError> {
        self.text_head("transcript_source_heads", "stream", stream.as_str())
    }

    /// Every accounted transcript head for one lane, in ascending stream order.
    ///
    /// The singular [`Self::transcript_source_head`] addresses one stream and
    /// needs a vendor and stream name to compose its key. A lane alone cannot
    /// supply them and nothing here maps a lane to the vendors that have
    /// occupied it, so this answers the question a lane *can* ask: everything
    /// accounted beneath it. One lane hosts several vendors over its life, which
    /// makes the whole set the honest answer rather than a convenience.
    ///
    /// The lane is matched against each decoded body rather than against a
    /// pattern over the stored key, because the body is the durable spelling and
    /// a stored key disagreeing with it is a head violation. That violation is
    /// reported instead of being silently included or dropped, and it is checked
    /// for every row rather than only the matching ones: the journal is one
    /// authority, so a corrupt row is corrupt no matter whose lane asked.
    pub fn transcript_source_heads(
        &self,
        lane: &wire::CanonicalUuid,
    ) -> Result<Vec<JournalHead<wire::TranscriptSourceHead>>, GuardError> {
        let mut statement = self
            .connection
            .prepare("SELECT stream, head_seq, body FROM transcript_source_heads ORDER BY stream")
            .map_err(GuardError::Store)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(GuardError::Store)?;
        let mut heads = Vec::new();
        for row in rows {
            let (stream, position, bytes) = row.map_err(GuardError::Store)?;
            heads.push(self.decoded_source_head(&stream, position, &bytes)?);
        }
        heads.retain(|head: &JournalHead<wire::TranscriptSourceHead>| &head.body.lane == lane);
        Ok(heads)
    }

    /// The accounted transcript boundary a lane's phase interval opens or closes at.
    ///
    /// This is the only way a lifecycle fact obtains a boundary, so every such
    /// fact names the same authority. It answers [`SourceBoundary::Proven`] or
    /// it fails: a lane with nothing accounted yet is proven at zero heads, and
    /// a journal that cannot be read is a violated assumption rather than an
    /// unprovable boundary. Degrading a read failure into
    /// [`SourceBoundary::Incomplete`] would record *unknown* where the truth is
    /// *broken*.
    ///
    /// No live producer writes `Incomplete`: every one of them admits a claim
    /// before it writes, and a claim carries a lane, so a producer that has
    /// nothing to ask about here does not exist. The variant is reached only by
    /// the forward migration of facts stored before boundaries were recorded,
    /// whose boundary genuinely cannot be proven because nothing recorded it.
    pub fn source_boundary(
        &self,
        lane: &wire::CanonicalUuid,
    ) -> Result<SourceBoundary, GuardError> {
        Ok(SourceBoundary::Proven {
            heads: self
                .transcript_source_heads(lane)?
                .into_iter()
                .map(|head| head.body)
                .collect(),
        })
    }

    /// Every lane a live claim is currently bound to.
    ///
    /// A membership transition records boundaries only for these, because a
    /// lane nobody is working spends nothing across the move and a boundary for
    /// it would be attribution noise the reducer has to filter back out. The
    /// disposition is read from the decoded head rather than from a query
    /// predicate so an ended claim can never be counted as live.
    pub fn actively_claimed_lanes(&self) -> Result<Vec<wire::CanonicalUuid>, GuardError> {
        let mut statement = self
            .connection
            .prepare("SELECT stamp, head_seq, body FROM claim_heads ORDER BY stamp")
            .map_err(GuardError::Store)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(GuardError::Store)?;
        let mut lanes = Vec::new();
        for row in rows {
            let (stamp, position, bytes) = row.map_err(GuardError::Store)?;
            let head = self.decoded_claim_head(&stamp, position, &bytes)?;
            if head.body.disposition == wire::ClaimDisposition::Active {
                lanes.push(head.body.lane);
            }
        }
        Ok(lanes)
    }

    /// Boundaries for each named lane that is actively claimed, in the given order.
    ///
    /// The filter is applied here rather than by each caller so every membership
    /// transition scopes attribution the same way, and the answer stays empty
    /// rather than becoming a second spelling of *unknown* when no named lane is
    /// claimed: an empty list says no claimed lane moved, which is exactly what
    /// the reducer needs to attribute nothing across the transition.
    pub fn lane_source_boundaries(
        &self,
        lanes: &[wire::CanonicalUuid],
    ) -> Result<Vec<wire::body::LaneSourceBoundary>, GuardError> {
        let claimed = self.actively_claimed_lanes()?;
        lanes
            .iter()
            .filter(|lane| claimed.contains(lane))
            .map(|lane| {
                Ok(wire::body::LaneSourceBoundary {
                    lane: *lane,
                    source: self.source_boundary(lane)?,
                })
            })
            .collect()
    }

    /// Decode one claim head and prove it against its stored position.
    ///
    /// The same two checks [`Self::claim_head`] makes when it is asked for one
    /// stamp, so a scan cannot admit a row the keyed read would have refused.
    fn decoded_claim_head(
        &self,
        stamp: &str,
        position: i64,
        bytes: &[u8],
    ) -> Result<JournalHead<wire::ClaimHead>, GuardError> {
        let head: JournalHead<wire::ClaimHead> = decode_head(position, bytes).map_err(|error| {
            let detail = match error {
                GuardError::Invalid(detail) => detail,
                other => other.to_string(),
            };
            GuardError::Invalid(format!("claim head {stamp:?} is invalid: {detail}"))
        })?;
        if head.body.fact != head.position {
            return Err(GuardError::Invalid(format!(
                "task {stamp} claim head fact disagrees with head_seq"
            )));
        }
        if self.cursor.is_none_or(|cursor| head.position > cursor) {
            return Err(GuardError::Invalid(format!(
                "claim head {stamp:?} position {} is beyond cursor {:?}",
                head.position,
                self.cursor.map(wire::Seq::get)
            )));
        }
        Ok(head)
    }

    /// Decode one transcript source head and prove it against its stored key.
    fn decoded_source_head(
        &self,
        stream: &str,
        position: i64,
        bytes: &[u8],
    ) -> Result<JournalHead<wire::TranscriptSourceHead>, GuardError> {
        let head: JournalHead<wire::TranscriptSourceHead> =
            decode_head(position, bytes).map_err(|error| {
                let detail = match error {
                    GuardError::Invalid(detail) => detail,
                    other => other.to_string(),
                };
                GuardError::Invalid(format!(
                    "transcript source head {stream:?} is invalid: {detail}"
                ))
            })?;
        if head.body.key().as_str() != stream {
            return Err(GuardError::Invalid(format!(
                "transcript source head stored as {stream:?} names {:?}",
                head.body.key().as_str()
            )));
        }
        if self.cursor.is_none_or(|cursor| head.position > cursor) {
            return Err(GuardError::Invalid(format!(
                "transcript source head {stream:?} position {} is beyond cursor {:?}",
                head.position,
                self.cursor.map(wire::Seq::get)
            )));
        }
        Ok(head)
    }

    /// Current resident head, when an epoch has been seated.
    pub fn residency_head(&self) -> Result<Option<JournalHead<wire::ResidencyHead>>, GuardError> {
        read_metrics::observe_query();
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
            read_metrics::observe_row();
            read_metrics::observe_decode(bytes.len());
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
        read_metrics::observe_query();
        let row = self
            .connection
            .query_row(&statement, [key], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .optional()
            .map_err(GuardError::Store)?;
        row.map_or(Ok(None), |(position, bytes)| {
            read_metrics::observe_row();
            read_metrics::observe_decode(bytes.len());
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
        self.heads.push(HeadUpdate::new(
            fact,
            HeadUpdateKind::Ordinary,
            move |seq| {
                let head = build(seq);
                let key = head.born.as_str().to_owned();
                if head.revision != seq {
                    return Err("task head revision does not name its assigned fact".to_owned());
                }
                HeadRow::text("task_heads", "stamp", key, seq, &head)
            },
        ));
    }

    /// Replace the current workflow head at the assigned fact position.
    pub fn workflow_head<F>(&mut self, task: wire::TaskStamp, fact: PendingFact, build: F)
    where
        F: FnOnce(wire::Seq) -> wire::WorkflowHead + Send + 'static,
    {
        self.heads.push(HeadUpdate::new(
            fact,
            HeadUpdateKind::Ordinary,
            move |seq| {
                HeadRow::text(
                    "workflow_heads",
                    "stamp",
                    task.as_str().to_owned(),
                    seq,
                    &build(seq),
                )
            },
        ));
    }

    /// Replace the current claim head at the assigned fact position.
    pub fn claim_head<F>(&mut self, task: wire::TaskStamp, fact: PendingFact, build: F)
    where
        F: FnOnce(wire::Seq) -> wire::ClaimHead + Send + 'static,
    {
        self.heads.push(HeadUpdate::new(
            fact,
            HeadUpdateKind::Ordinary,
            move |seq| {
                let head = build(seq);
                if head.fact != seq {
                    return Err("claim head fact does not name its assigned fact".to_owned());
                }
                HeadRow::claim(task.as_str().to_owned(), seq, &head)
            },
        ));
    }

    /// Replace the current lease head at the assigned fact position.
    pub fn lease_head<F>(&mut self, task: wire::TaskStamp, fact: PendingFact, build: F)
    where
        F: FnOnce(wire::Seq) -> wire::LeaseHead + Send + 'static,
    {
        self.heads.push(HeadUpdate::new(
            fact,
            HeadUpdateKind::Ordinary,
            move |seq| {
                HeadRow::text(
                    "lease_heads",
                    "stamp",
                    task.as_str().to_owned(),
                    seq,
                    &build(seq),
                )
            },
        ));
    }

    /// Replace one named witness head at the assigned fact position.
    pub fn witness_head<F>(&mut self, fact: PendingFact, build: F)
    where
        F: FnOnce(wire::Seq) -> wire::WitnessHead + Send + 'static,
    {
        self.heads.push(HeadUpdate::new(
            fact,
            HeadUpdateKind::Ordinary,
            move |seq| {
                let head = build(seq);
                let key = head.name.clone();
                if head.head != seq {
                    return Err("witness head does not name its assigned fact".to_owned());
                }
                HeadRow::text("witness_heads", "name", key, seq, &head)
            },
        ));
    }

    /// Replace one executable trust head at the assigned fact position.
    pub fn trust_head<F>(&mut self, fact: PendingFact, build: F)
    where
        F: FnOnce(wire::Seq) -> wire::TrustHead + Send + 'static,
    {
        self.heads.push(HeadUpdate::new(
            fact,
            HeadUpdateKind::Ordinary,
            move |seq| {
                let head = build(seq);
                HeadRow::text(
                    "trust_heads",
                    "digest",
                    head.digest.as_str().to_owned(),
                    seq,
                    &head,
                )
            },
        ));
    }

    /// Replace one gate finding head at the assigned fact position.
    pub fn finding_head<F>(&mut self, fact: PendingFact, build: F)
    where
        F: FnOnce(wire::Seq) -> wire::FindingHead + Send + 'static,
    {
        self.heads.push(HeadUpdate::new(
            fact,
            HeadUpdateKind::Ordinary,
            move |seq| {
                let head = build(seq);
                let key = head.identity.clone();
                HeadRow::text("finding_heads", "identity", key, seq, &head)
            },
        ));
    }

    /// Replace one directive's current steering head at the assigned fact.
    pub fn steering_head<F>(&mut self, fact: PendingFact, build: F)
    where
        F: FnOnce(wire::Seq) -> wire::SteeringHead + Send + 'static,
    {
        self.heads.push(HeadUpdate::new(
            fact,
            HeadUpdateKind::Steering,
            move |seq| HeadRow::steering(seq, &build(seq)),
        ));
    }

    /// Index one exact missed-recognition body at its evidence fact.
    pub fn steering_miss(&mut self, fact: PendingFact, miss: wire::body::SteerMissed) {
        self.heads.push(HeadUpdate::new(
            fact,
            HeadUpdateKind::SteeringMiss,
            move |seq| {
                let identity = canonical_identity(&miss).map_err(|error| error.to_string())?;
                HeadRow::text("steering_misses", "identity", identity, seq, &miss)
            },
        ));
    }

    /// Replace one transcript source head at the assigned fact position.
    pub fn transcript_source_head<F>(&mut self, fact: PendingFact, build: F)
    where
        F: FnOnce(wire::Seq) -> wire::TranscriptSourceHead + Send + 'static,
    {
        self.heads.push(HeadUpdate::new(
            fact,
            HeadUpdateKind::Ordinary,
            move |seq| {
                let head = build(seq);
                let key = head.key().into_string();
                HeadRow::text("transcript_source_heads", "stream", key, seq, &head)
            },
        ));
    }

    /// Replace the singleton resident head at the assigned fact position.
    pub fn residency_head<F>(&mut self, fact: PendingFact, build: F)
    where
        F: FnOnce(wire::Seq) -> wire::ResidencyHead + Send + 'static,
    {
        self.heads.push(HeadUpdate::new(
            fact,
            HeadUpdateKind::Ordinary,
            move |seq| HeadRow::singleton("residency_head", seq, &build(seq)),
        ));
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
    kind: HeadUpdateKind,
    build: Box<HeadBuilder>,
}

impl HeadUpdate {
    fn new<F>(fact: PendingFact, kind: HeadUpdateKind, build: F) -> Self
    where
        F: FnOnce(wire::Seq) -> Result<HeadRow, String> + Send + 'static,
    {
        Self {
            fact,
            kind,
            build: Box::new(build),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HeadUpdateKind {
    Ordinary,
    Steering,
    SteeringMiss,
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
    steering: Option<SteeringHeadColumns>,
    claim: Option<ClaimIndexColumns>,
}

struct ClaimIndexColumns {
    actor: String,
    disposition: &'static str,
}

struct SteeringHeadColumns {
    key: String,
    lane: String,
    disposition: &'static str,
    created: wire::Seq,
    minted_millis: Option<i64>,
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
            steering: None,
            claim: None,
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
            steering: None,
            claim: None,
        })
    }

    fn claim(key: String, seq: wire::Seq, head: &wire::ClaimHead) -> Result<Self, String> {
        if key.is_empty() {
            return Err("claim_heads head key must not be empty".to_owned());
        }
        Ok(Self {
            table: "claim_heads",
            key_column: "stamp",
            key: HeadKey::Text(key),
            seq,
            body: wire::encode(head).map_err(|error| error.to_string())?,
            steering: None,
            claim: Some(ClaimIndexColumns {
                actor: head.actor.to_string(),
                disposition: claim_disposition_name(head.disposition),
            }),
        })
    }

    fn steering(seq: wire::Seq, head: &wire::SteeringHead) -> Result<Self, String> {
        if head.key.is_empty() {
            return Err("steering head key must not be empty".to_owned());
        }
        if head.created > seq {
            return Err("steering head cannot predate its creating fact".to_owned());
        }
        Ok(Self {
            table: "steering_heads",
            key_column: "directive",
            key: HeadKey::Text(head.directive.to_string()),
            seq,
            body: wire::encode(head).map_err(|error| error.to_string())?,
            steering: Some(SteeringHeadColumns {
                key: head.key.clone(),
                lane: head.lane.to_string(),
                disposition: disposition_name(head.disposition),
                created: head.created,
                minted_millis: head.minted_millis,
            }),
            claim: None,
        })
    }
}

const fn claim_disposition_name(disposition: wire::ClaimDisposition) -> &'static str {
    match disposition {
        wire::ClaimDisposition::Active => "active",
        wire::ClaimDisposition::Freed => "freed",
        wire::ClaimDisposition::Lapsed => "lapsed",
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

include!("guard/commit.rs");

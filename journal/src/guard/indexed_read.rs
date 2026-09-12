//! Indexed reads exposed by one immutable journal transaction.

use rusqlite::{OptionalExtension, Row, params, params_from_iter, types::Value};
use serde::Serialize;

use super::commit::decode_seq;
use super::{
    FactPageStart, FactReadBudget, GuardError, JournalHead, JournalRead, SelectedFactPage,
    TaskHeads, read_metrics,
};
use wire::body::SourceBoundary;

mod bounds;
mod origin;

use bounds::{admitted_envelope_length, decode_bounded_fact};

struct CausalPage {
    facts: Vec<wire::Envelope<wire::AnyFact>>,
    total: Option<u64>,
    origin: Option<wire::Origin>,
    decoded_bytes: usize,
    overflow: Option<wire::Overflow>,
}

impl CausalPage {
    fn new(capacity: usize) -> Self {
        Self {
            facts: Vec::with_capacity(capacity),
            total: None,
            origin: None,
            decoded_bytes: 0,
            overflow: None,
        }
    }

    fn push(&mut self, row: &Row<'_>, budget: FactReadBudget) -> Result<bool, GuardError> {
        let seq = row.get::<_, i64>(0).map_err(GuardError::Store)?;
        let remaining = row.get::<_, i64>(3).map_err(GuardError::Store)?;
        read_metrics::observe_row();
        let length =
            admitted_envelope_length(seq, row.get::<_, i64>(5).map_err(GuardError::Store)?)?;
        if self.facts.len() == budget.rows()
            || (!self.facts.is_empty()
                && self.decoded_bytes.saturating_add(length) > budget.decoded_bytes())
        {
            let continuation = u64::try_from(seq)
                .ok()
                .and_then(wire::Seq::new)
                .ok_or_else(|| {
                    GuardError::Invalid(format!(
                        "thread index sequence {seq} is outside the wire range"
                    ))
                })?;
            let omitted = u64::try_from(remaining)
                .map_err(|_| {
                    GuardError::Invalid(format!(
                        "thread index depth {remaining} is outside the wire range"
                    ))
                })?
                .saturating_add(1);
            self.overflow = Some(wire::Overflow {
                omitted,
                continuation,
            });
            return Ok(false);
        }
        let origin = row.get::<_, String>(1).map_err(GuardError::Store)?;
        let cause = row.get::<_, Option<i64>>(2).map_err(GuardError::Store)?;
        let (fact, length) = decode_bounded_fact(row, seq, 4, 5, 6)?;
        self.decoded_bytes = self.decoded_bytes.saturating_add(length);
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
        if self
            .origin
            .as_ref()
            .is_some_and(|chain_origin| chain_origin != &fact.origin)
        {
            return Err(GuardError::Invalid(format!(
                "thread index at {} crosses causal origins",
                fact.seq
            )));
        }
        self.origin.get_or_insert_with(|| fact.origin.clone());
        let remaining = u64::try_from(remaining).map_err(|_| {
            GuardError::Invalid(format!(
                "thread index depth {remaining} is outside the wire range"
            ))
        })?;
        if let Some(total) = self.total {
            let step = u64::try_from(self.facts.len()).unwrap_or(u64::MAX);
            let expected = total.saturating_sub(step.saturating_add(1));
            if remaining != expected {
                return Err(GuardError::Invalid(format!(
                    "thread index at {} has depth {remaining}, expected {expected}",
                    fact.seq
                )));
            }
        } else {
            self.total = Some(remaining.saturating_add(1));
        }
        self.facts.push(fact);
        Ok(true)
    }

    fn finish(
        self,
        limit: usize,
        horizon: wire::Seq,
    ) -> Result<Option<wire::Packed<wire::Envelope<wire::AnyFact>>>, GuardError> {
        let Some(total) = self.total else {
            return Ok(None);
        };
        let expected = usize::try_from(total).unwrap_or(usize::MAX).min(limit);
        if self.overflow.is_none() && self.facts.len() != expected {
            return Err(GuardError::Invalid(format!(
                "indexed causal range expected {expected} row(s), found {}",
                self.facts.len()
            )));
        }
        Ok(Some(wire::Packed {
            rows: self.facts,
            overflow: self.overflow,
            horizon,
        }))
    }
}

impl JournalRead<'_> {
    /// Last durable fact position visible to this transaction.
    pub fn cursor(&self) -> Result<Option<wire::Seq>, GuardError> {
        Ok(self.cursor)
    }

    /// Read one bounded page of the named fact kinds after an admitted cursor.
    ///
    /// The `(kind, seq)` authority index makes this proportional to matching
    /// rows. Original positions and order are retained. The continuation is
    /// the last returned cursor, so the next call resumes strictly after it.
    pub fn selected_fact_page(
        &self,
        kinds: &[wire::FactKind],
        after: Option<wire::Seq>,
        budget: FactReadBudget,
    ) -> Result<SelectedFactPage, GuardError> {
        let Some(horizon) = self.cursor else {
            return Ok(SelectedFactPage {
                rows: Vec::new(),
                continuation: None,
                horizon: None,
            });
        };
        if kinds.is_empty() || after == Some(horizon) {
            return Ok(SelectedFactPage {
                rows: Vec::new(),
                continuation: None,
                horizon: Some(horizon),
            });
        }
        if after.is_some_and(|after| after > horizon) {
            return Err(GuardError::Invalid(format!(
                "selected fact cursor {after:?} is beyond journal cursor {horizon}"
            )));
        }
        let placeholders = (3..kinds.len() + 3)
            .map(|index| format!("?{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        let limit_parameter = kinds.len() + 3;
        let sql = format!(
            "SELECT seq, kind, length(envelope), envelope FROM fact_envelopes \
             WHERE seq > ?1 AND seq <= ?2 AND kind IN ({placeholders}) \
             ORDER BY seq LIMIT ?{limit_parameter}"
        );
        let mut parameters = Vec::with_capacity(kinds.len() + 3);
        let after = after
            .map(|seq| {
                i64::try_from(seq.get()).map_err(|_| {
                    GuardError::Invalid("selected fact cursor exceeds SQLite".to_owned())
                })
            })
            .transpose()?
            .unwrap_or(0);
        parameters.push(Value::Integer(after));
        parameters.push(Value::Integer(i64::try_from(horizon.get()).map_err(
            |_| GuardError::Invalid("selected fact horizon exceeds SQLite".to_owned()),
        )?));
        parameters.extend(
            kinds
                .iter()
                .map(|kind| Value::Text(kind.as_str().to_owned())),
        );
        let query_limit = budget
            .rows()
            .checked_add(1)
            .expect("repository fact row ceiling leaves room for an overflow probe");
        parameters.push(Value::Integer(i64::try_from(query_limit).map_err(
            |_| GuardError::Invalid("selected fact page limit exceeds SQLite".to_owned()),
        )?));
        read_metrics::observe_query();
        let mut statement = self.connection.prepare(&sql).map_err(GuardError::Store)?;
        let mut rows = statement
            .query(params_from_iter(&parameters))
            .map_err(GuardError::Store)?;
        let mut facts: Vec<wire::Envelope<wire::AnyFact>> = Vec::with_capacity(budget.rows());
        let mut decoded_bytes = 0usize;
        let mut continuation = None;
        while let Some(row) = rows.next().map_err(GuardError::Store)? {
            let seq = row.get::<_, i64>(0).map_err(GuardError::Store)?;
            read_metrics::observe_row();
            if facts.len() == budget.rows() {
                continuation = facts.last().map(|fact| fact.seq);
                break;
            }
            let length =
                admitted_envelope_length(seq, row.get::<_, i64>(2).map_err(GuardError::Store)?)?;
            if !facts.is_empty() && decoded_bytes.saturating_add(length) > budget.decoded_bytes() {
                continuation = facts.last().map(|fact| fact.seq);
                break;
            }
            let (fact, length) = decode_bounded_fact(row, seq, 1, 2, 3)?;
            decoded_bytes = decoded_bytes.saturating_add(length);
            facts.push(fact);
        }
        Ok(SelectedFactPage {
            rows: facts,
            continuation,
            horizon: Some(horizon),
        })
    }

    /// Read one bounded newest-first page of a retained fact kind.
    ///
    /// The continuation is the oldest returned cursor, so the next call passes
    /// it as `before` and resumes strictly before it.
    pub fn recent_fact_page(
        &self,
        kind: wire::FactKind,
        before: Option<wire::Seq>,
        budget: FactReadBudget,
    ) -> Result<SelectedFactPage, GuardError> {
        let Some(horizon) = self.cursor else {
            return Ok(SelectedFactPage {
                rows: Vec::new(),
                continuation: None,
                horizon: None,
            });
        };
        if before.is_some_and(|before| before > horizon) {
            return Err(GuardError::Invalid(format!(
                "recent fact cursor {before:?} is beyond journal cursor {horizon}"
            )));
        }
        let query_limit = budget
            .rows()
            .checked_add(1)
            .expect("repository fact row ceiling leaves room for an overflow probe");
        let horizon_sql = i64::try_from(horizon.get())
            .map_err(|_| GuardError::Invalid("recent fact horizon exceeds SQLite".to_owned()))?;
        let query_limit = i64::try_from(query_limit)
            .map_err(|_| GuardError::Invalid("recent fact page limit exceeds SQLite".to_owned()))?;
        let mut sql = String::from(
            "SELECT seq, kind, length(envelope), envelope FROM fact_envelopes \
             WHERE kind = ?1 AND seq <= ?2",
        );
        let mut parameters = vec![
            Value::Text(kind.as_str().to_owned()),
            Value::Integer(horizon_sql),
        ];
        if let Some(before) = before {
            let before = i64::try_from(before.get())
                .map_err(|_| GuardError::Invalid("recent fact cursor exceeds SQLite".to_owned()))?;
            sql.push_str(" AND seq < ?3 ORDER BY seq DESC LIMIT ?4");
            parameters.push(Value::Integer(before));
            parameters.push(Value::Integer(query_limit));
        } else {
            sql.push_str(" ORDER BY seq DESC LIMIT ?3");
            parameters.push(Value::Integer(query_limit));
        }
        read_metrics::observe_query();
        let mut statement = self.connection.prepare(&sql).map_err(GuardError::Store)?;
        let mut rows = statement
            .query(params_from_iter(&parameters))
            .map_err(GuardError::Store)?;
        let mut facts: Vec<wire::Envelope<wire::AnyFact>> = Vec::with_capacity(budget.rows());
        let mut decoded_bytes = 0usize;
        let mut continuation = None;
        while let Some(row) = rows.next().map_err(GuardError::Store)? {
            let seq = row.get::<_, i64>(0).map_err(GuardError::Store)?;
            read_metrics::observe_row();
            if facts.len() == budget.rows() {
                continuation = facts.last().map(|fact| fact.seq);
                break;
            }
            let length =
                admitted_envelope_length(seq, row.get::<_, i64>(2).map_err(GuardError::Store)?)?;
            if !facts.is_empty() && decoded_bytes.saturating_add(length) > budget.decoded_bytes() {
                continuation = facts.last().map(|fact| fact.seq);
                break;
            }
            let (fact, length) = decode_bounded_fact(row, seq, 1, 2, 3)?;
            decoded_bytes = decoded_bytes.saturating_add(length);
            facts.push(fact);
        }
        Ok(SelectedFactPage {
            rows: facts,
            continuation,
            horizon: Some(horizon),
        })
    }

    /// Read the newest fact of one kind through the immutable horizon.
    pub fn latest_fact(
        &self,
        kind: wire::FactKind,
    ) -> Result<Option<wire::Envelope<wire::AnyFact>>, GuardError> {
        let Some(horizon) = self.cursor else {
            return Ok(None);
        };
        let horizon = i64::try_from(horizon.get())
            .map_err(|_| GuardError::Invalid("latest fact horizon exceeds SQLite".to_owned()))?;
        read_metrics::observe_query();
        let mut statement = self
            .connection
            .prepare(
                "SELECT seq, kind, length(envelope), envelope FROM fact_envelopes \
                 WHERE kind = ?1 AND seq <= ?2 ORDER BY seq DESC LIMIT 1",
            )
            .map_err(GuardError::Store)?;
        let mut rows = statement
            .query(params![kind.as_str(), horizon])
            .map_err(GuardError::Store)?;
        let Some(row) = rows.next().map_err(GuardError::Store)? else {
            return Ok(None);
        };
        let seq = row.get::<_, i64>(0).map_err(GuardError::Store)?;
        read_metrics::observe_row();
        decode_bounded_fact(row, seq, 1, 2, 3).map(|(fact, _)| Some(fact))
    }

    /// Read one bounded primary-key fact range through an immutable horizon.
    pub fn fact_page(
        &self,
        start: FactPageStart,
        horizon: wire::Seq,
        budget: FactReadBudget,
    ) -> Result<wire::Packed<wire::Envelope<wire::AnyFact>>, GuardError> {
        let limit = budget.rows();
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
                "SELECT seq, kind, length(envelope), envelope FROM fact_envelopes \
                 WHERE seq >= ?1 AND seq <= ?2 ORDER BY seq LIMIT ?3",
            )
            .map_err(GuardError::Store)?;
        let mut rows = statement
            .query(params![first, horizon_sql, query_limit])
            .map_err(GuardError::Store)?;
        let mut facts = Vec::with_capacity(limit.min(usize::try_from(total).unwrap_or(usize::MAX)));
        let mut decoded_bytes = 0usize;
        let mut continuation = None;
        while let Some(row) = rows.next().map_err(GuardError::Store)? {
            let seq = row.get::<_, i64>(0).map_err(GuardError::Store)?;
            read_metrics::observe_row();
            let length =
                admitted_envelope_length(seq, row.get::<_, i64>(2).map_err(GuardError::Store)?)?;
            if facts.len() == limit
                || (!facts.is_empty()
                    && decoded_bytes.saturating_add(length) > budget.decoded_bytes())
            {
                continuation = u64::try_from(seq).ok().and_then(wire::Seq::new);
                break;
            }
            let (fact, length) = decode_bounded_fact(row, seq, 1, 2, 3)?;
            decoded_bytes = decoded_bytes.saturating_add(length);
            facts.push(fact);
        }
        if continuation.is_none() && u64::try_from(facts.len()).unwrap_or(u64::MAX) != total {
            return Err(GuardError::Invalid(format!(
                "indexed fact range expected {total} row(s), found {}",
                facts.len()
            )));
        }
        let overflow = continuation.map(|continuation| wire::Overflow {
            omitted: horizon
                .get()
                .saturating_sub(continuation.get())
                .saturating_add(1),
            continuation,
        });
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
        budget: FactReadBudget,
    ) -> Result<Option<wire::Packed<wire::Envelope<wire::AnyFact>>>, GuardError> {
        let limit = budget.rows();
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
                        fact.kind, length(fact.envelope), fact.envelope \
                 FROM chain JOIN fact_envelopes AS fact ON fact.seq = chain.seq \
                 ORDER BY chain.step LIMIT ?2",
            )
            .map_err(GuardError::Store)?;
        let mut rows = statement
            .query(params![position_sql, query_limit_sql])
            .map_err(GuardError::Store)?;
        let mut page = CausalPage::new(query_limit);
        while let Some(row) = rows.next().map_err(GuardError::Store)? {
            if !page.push(row, budget)? {
                break;
            }
        }
        page.finish(limit, horizon)
    }

    /// Every current task storage bundle in ascending birth order.
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
            bundles.push(
                self.task_heads_for(&stamp)?
                    .expect("the union query names at least one head plane"),
            );
        }
        Ok(bundles)
    }

    /// Current storage bundle for one exact task birth stamp.
    pub fn task_heads_for(&self, stamp: &wire::TaskStamp) -> Result<Option<TaskHeads>, GuardError> {
        let statement = self.task_head(stamp)?;
        let workflow = self.workflow_head(stamp)?;
        let claim = self.claim_head(stamp)?;
        let lease = self.lease_head(stamp)?;
        if statement.is_none() && workflow.is_none() && claim.is_none() && lease.is_none() {
            return Ok(None);
        }
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
        Ok(Some(TaskHeads {
            statement,
            workflow,
            claim,
            lease,
        }))
    }

    /// Task bundles whose indexed heads changed inside one committed range.
    pub fn changed_task_heads(
        &self,
        first: wire::Seq,
        last: wire::Seq,
    ) -> Result<Vec<TaskHeads>, GuardError> {
        let Some(cursor) = self.cursor else {
            return Err(GuardError::Invalid(
                "changed task heads require a durable cursor".to_owned(),
            ));
        };
        if first > last || last > cursor {
            return Err(GuardError::Invalid(format!(
                "changed task head range {first}..={last} is outside cursor {cursor}"
            )));
        }
        let first = i64::try_from(first.get())
            .map_err(|_| GuardError::Invalid("task head range exceeds SQLite".to_owned()))?;
        let last = i64::try_from(last.get())
            .map_err(|_| GuardError::Invalid("task head range exceeds SQLite".to_owned()))?;
        read_metrics::observe_query();
        let mut statement = self
            .connection
            .prepare(
                "SELECT DISTINCT stamp FROM (\
                    SELECT stamp, head_seq FROM task_heads UNION ALL \
                    SELECT stamp, head_seq FROM workflow_heads UNION ALL \
                    SELECT stamp, head_seq FROM claim_heads UNION ALL \
                    SELECT stamp, head_seq FROM lease_heads\
                 ) WHERE head_seq >= ?1 AND head_seq <= ?2 ORDER BY stamp",
            )
            .map_err(GuardError::Store)?;
        let rows = statement
            .query_map(params![first, last], |row| row.get::<_, String>(0))
            .map_err(GuardError::Store)?;
        let mut bundles = Vec::new();
        for row in rows {
            let indexed = row.map_err(GuardError::Store)?;
            let stamp = indexed.parse::<wire::TaskStamp>().map_err(|error| {
                GuardError::Invalid(format!("task head key {indexed:?} is invalid: {error}"))
            })?;
            bundles.push(
                self.task_heads_for(&stamp)?
                    .expect("a changed head range names a current task bundle"),
            );
        }
        Ok(bundles)
    }

    /// Current task statement head, when the birth stamp exists.
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

    /// Current workflow head, when the birth stamp exists.
    pub fn workflow_head(
        &self,
        stamp: &wire::TaskStamp,
    ) -> Result<Option<JournalHead<wire::WorkflowHead>>, GuardError> {
        self.text_head("workflow_heads", "stamp", stamp.as_str())
    }

    /// Current claim head, when the birth stamp exists.
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

    /// Current lease head, when the birth stamp exists.
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
    pub(super) fn decoded_claim_head(
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

pub(super) fn decode_head<T>(position: i64, bytes: &[u8]) -> Result<JournalHead<T>, GuardError>
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

//! Bounded reads of the room-authority projection.

use rusqlite::{OptionalExtension, params};
use wire::rooms::{
    CandidateCoverage, CandidateCritique, ExecutionAuthorization, PreparationDecision, ProposalRef,
    RetainedCandidate, RoomAuthorityStamp, RoomDelivered, WorkingGroup,
};

use super::{
    CurrentHeadWitness, FactReadBudget, GuardError, JournalHead, JournalRead, SelectedFactPage,
    admitted_envelope_length, decode_bounded_fact, decode_head, read_metrics,
};

/// One write-once proposal head, decoded as its subject's family.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProposalAuthority {
    /// One retained candidate.
    Candidate(RetainedCandidate),
    /// One critique of an exact candidate.
    Critique(CandidateCritique),
    /// One coverage statement over an exact candidate.
    Coverage(CandidateCoverage),
    /// One decision under a controlling task.
    Decision(PreparationDecision),
}

/// One ordered bounded page of current working-group heads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupHeadPage {
    /// Working-group heads decoded within this call's row and byte budgets.
    pub rows: Vec<JournalHead<WorkingGroup>>,
    /// Last returned identity when more current groups remain.
    pub continuation: Option<RoomAuthorityStamp>,
    /// Cursor identity of the projection this page observed.
    pub witness: CurrentHeadWitness,
}

/// One ordered bounded page of groups containing an anchor task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnchorGroupPage {
    /// Group identities decoded within this call's row and byte budgets.
    pub rows: Vec<RoomAuthorityStamp>,
    /// Last returned identity when more matching groups remain.
    pub continuation: Option<RoomAuthorityStamp>,
    /// Cursor identity of the projection this page observed.
    pub witness: CurrentHeadWitness,
}

impl JournalRead<'_> {
    /// Current working-group head, absent before creation or after removal.
    pub fn group_head(
        &self,
        group: &RoomAuthorityStamp,
    ) -> Result<Option<JournalHead<WorkingGroup>>, GuardError> {
        let key = group.to_string();
        let head = self.text_head::<WorkingGroup>("group_heads", "working_group", &key)?;
        if let Some(head) = &head
            && head.body.group != *group
        {
            return Err(GuardError::Invalid(format!(
                "group head indexed as {key} carries body group {}",
                head.body.group
            )));
        }
        Ok(head)
    }

    /// One page of current working groups strictly after `after`, in identity
    /// order.
    pub fn group_head_page(
        &self,
        after: Option<&RoomAuthorityStamp>,
        budget: FactReadBudget,
    ) -> Result<GroupHeadPage, GuardError> {
        let after = after.map(ToString::to_string).unwrap_or_default();
        read_metrics::observe_query();
        let mut statement = self
            .connection
            .prepare(
                "SELECT working_group, head_seq, length(body), body FROM group_heads \
                 WHERE working_group > ?1 ORDER BY working_group LIMIT ?2",
            )
            .map_err(GuardError::Store)?;
        let mut rows = statement
            .query(params![after, probe_limit(budget)?])
            .map_err(GuardError::Store)?;
        let mut heads = Vec::with_capacity(budget.rows());
        let mut decoded = 0usize;
        let mut continuation = None;
        while let Some(row) = rows.next().map_err(GuardError::Store)? {
            let indexed = row.get::<_, String>(0).map_err(GuardError::Store)?;
            read_metrics::observe_row();
            if heads.len() == budget.rows() {
                continuation = heads
                    .last()
                    .map(|head: &JournalHead<WorkingGroup>| head.body.group.clone());
                break;
            }
            let length = indexed_length(row, 2, "working-group body")?;
            if !heads.is_empty() && decoded.saturating_add(length) > budget.decoded_bytes() {
                continuation = heads.last().map(|head| head.body.group.clone());
                break;
            }
            let group = indexed.parse::<RoomAuthorityStamp>().map_err(|error| {
                GuardError::Invalid(format!("group head index names {indexed:?}: {error}"))
            })?;
            let position = row.get::<_, i64>(1).map_err(GuardError::Store)?;
            let bytes = row.get::<_, Vec<u8>>(3).map_err(GuardError::Store)?;
            let head: JournalHead<WorkingGroup> = decode_head(position, &bytes)?;
            if head.body.group != group {
                return Err(GuardError::Invalid(format!(
                    "group head indexed as {group} carries body group {}",
                    head.body.group
                )));
            }
            read_metrics::observe_decode(length);
            decoded = decoded.saturating_add(length);
            heads.push(head);
        }
        Ok(GroupHeadPage {
            rows: heads,
            continuation,
            witness: CurrentHeadWitness {
                cursor: self.cursor,
            },
        })
    }

    /// One page of working groups whose order includes this room anchor,
    /// strictly after `after` in identity order.
    pub fn anchor_group_page(
        &self,
        anchor: &wire::TaskStamp,
        after: Option<&RoomAuthorityStamp>,
        budget: FactReadBudget,
    ) -> Result<AnchorGroupPage, GuardError> {
        let after = after.map(ToString::to_string).unwrap_or_default();
        read_metrics::observe_query();
        let mut statement = self
            .connection
            .prepare(
                "SELECT length(CAST(working_group AS BLOB)), working_group \
                 FROM group_anchors WHERE anchor = ?1 AND working_group > ?2 \
                 ORDER BY working_group LIMIT ?3",
            )
            .map_err(GuardError::Store)?;
        let mut rows = statement
            .query(params![anchor.as_str(), after, probe_limit(budget)?])
            .map_err(GuardError::Store)?;
        let mut groups = Vec::with_capacity(budget.rows());
        let mut decoded = 0usize;
        let mut continuation = None;
        while let Some(row) = rows.next().map_err(GuardError::Store)? {
            read_metrics::observe_row();
            if groups.len() == budget.rows() {
                continuation = groups.last().cloned();
                break;
            }
            let length = indexed_length(row, 0, "working-group identity")?;
            if !groups.is_empty() && decoded.saturating_add(length) > budget.decoded_bytes() {
                continuation = groups.last().cloned();
                break;
            }
            let indexed = row.get::<_, String>(1).map_err(GuardError::Store)?;
            let group = indexed.parse().map_err(|error| {
                GuardError::Invalid(format!("group anchor index names {indexed:?}: {error}"))
            })?;
            read_metrics::observe_decode(length);
            decoded = decoded.saturating_add(length);
            groups.push(group);
        }
        Ok(AnchorGroupPage {
            rows: groups,
            continuation,
            witness: CurrentHeadWitness {
                cursor: self.cursor,
            },
        })
    }

    /// The write-once head of one proposal subject, absent before recording.
    pub fn proposal_head(
        &self,
        proposal: &ProposalRef,
    ) -> Result<Option<JournalHead<ProposalAuthority>>, GuardError> {
        let subject = crate::rooms::subject(proposal)
            .map_err(|refusal| GuardError::Invalid(refusal.to_string()))?;
        let head = match proposal {
            ProposalRef::Candidate { candidate } => self.proposal_family(
                &subject,
                |body: &RetainedCandidate| body.candidate == *candidate,
                ProposalAuthority::Candidate,
            )?,
            ProposalRef::Critique { critique } => self.proposal_family(
                &subject,
                |body: &CandidateCritique| body.critique == *critique,
                ProposalAuthority::Critique,
            )?,
            ProposalRef::Coverage { coverage } => self.proposal_family(
                &subject,
                |body: &CandidateCoverage| body.coverage == *coverage,
                ProposalAuthority::Coverage,
            )?,
            ProposalRef::Decision { decision } => self.proposal_family(
                &subject,
                |body: &PreparationDecision| body.decision == *decision,
                ProposalAuthority::Decision,
            )?,
        };
        Ok(head)
    }
    /// Authenticated principal that retained one candidate, absent with its head.
    pub fn proposal_author(
        &self,
        proposal: &ProposalRef,
    ) -> Result<Option<wire::Actor>, GuardError> {
        let subject = crate::rooms::subject(proposal)
            .map_err(|refusal| GuardError::Invalid(refusal.to_string()))?;
        read_metrics::observe_query();
        let actor = self
            .connection
            .query_row(
                "SELECT fact.actor FROM proposal_heads AS proposal \
                 JOIN fact_envelopes AS fact ON fact.seq = proposal.head_seq \
                 WHERE proposal.subject = ?1",
                [&subject],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(GuardError::Store)?;
        actor
            .map(|actor| {
                actor.parse().map_err(|_| {
                    GuardError::Invalid(format!(
                        "proposal head {subject} names invalid actor {actor:?}"
                    ))
                })
            })
            .transpose()
    }

    fn proposal_family<T>(
        &self,
        subject: &str,
        names: impl FnOnce(&T) -> bool,
        family: impl FnOnce(T) -> ProposalAuthority,
    ) -> Result<Option<JournalHead<ProposalAuthority>>, GuardError>
    where
        T: serde::de::DeserializeOwned + serde::Serialize,
    {
        let Some(head) = self.text_head::<T>("proposal_heads", "subject", subject)? else {
            return Ok(None);
        };
        if !names(head.body()) {
            return Err(GuardError::Invalid(format!(
                "proposal head indexed as {subject} carries another subject"
            )));
        }
        let position = head.position();
        Ok(Some(JournalHead::new(position, family(head.into_body()))))
    }

    /// Current authorization of one selected output, absent while unauthorized.
    pub fn authorization_head(
        &self,
        output: &wire::TaskStamp,
    ) -> Result<Option<JournalHead<ExecutionAuthorization>>, GuardError> {
        let head = self.text_head::<ExecutionAuthorization>(
            "authorization_heads",
            "output",
            output.as_str(),
        )?;
        if let Some(head) = &head
            && !head.body.selected_outputs.contains(output)
        {
            return Err(GuardError::Invalid(format!(
                "authorization head indexed as {output} does not select it"
            )));
        }
        Ok(head)
    }

    /// One logical lane's current room catch-up cursor, absent before its
    /// first landed delivery.
    pub fn room_cursor(
        &self,
        lane: wire::CanonicalUuid,
    ) -> Result<Option<JournalHead<RoomDelivered>>, GuardError> {
        let key = lane.to_string();
        let head = self.text_head::<RoomDelivered>("room_cursors", "lane", &key)?;
        if let Some(head) = &head
            && head.body.hunk.head.lane != lane
        {
            return Err(GuardError::Invalid(format!(
                "room cursor indexed as {key} lands in lane {}",
                head.body.hunk.head.lane
            )));
        }
        Ok(head)
    }

    /// One bounded page of a room's publications strictly after `after`, in
    /// journal order, through the current horizon.
    pub fn room_publications(
        &self,
        room: &wire::TaskStamp,
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
        let after = after.map_or(Ok(0), |seq| i64::try_from(seq.get()));
        let horizon_sql = i64::try_from(horizon.get());
        let (Ok(after), Ok(horizon_sql)) = (after, horizon_sql) else {
            return Err(GuardError::Invalid(
                "room publication cursor exceeds SQLite".to_owned(),
            ));
        };
        read_metrics::observe_query();
        let mut statement = self
            .connection
            .prepare(
                "SELECT facts.seq, facts.kind, length(facts.envelope), facts.envelope \
                 FROM room_publications AS published \
                 JOIN fact_envelopes AS facts ON facts.seq = published.seq \
                 WHERE published.room = ?1 AND published.seq > ?2 AND published.seq <= ?3 \
                 ORDER BY published.seq LIMIT ?4",
            )
            .map_err(GuardError::Store)?;
        let probe = probe_limit(budget)?;
        let mut rows = statement
            .query(params![room.as_str(), after, horizon_sql, probe])
            .map_err(GuardError::Store)?;
        let mut facts: Vec<wire::Envelope<wire::AnyFact>> = Vec::new();
        let mut decoded = 0usize;
        let mut continuation = None;
        while let Some(row) = rows.next().map_err(GuardError::Store)? {
            let seq = row.get::<_, i64>(0).map_err(GuardError::Store)?;
            read_metrics::observe_row();
            let length =
                admitted_envelope_length(seq, row.get::<_, i64>(2).map_err(GuardError::Store)?)?;
            if facts.len() == budget.rows()
                || (!facts.is_empty() && decoded.saturating_add(length) > budget.decoded_bytes())
            {
                continuation = facts.last().map(|fact| fact.seq);
                break;
            }
            let (fact, length) = decode_bounded_fact(row, seq, 1, 2, 3)?;
            if !matches!(
                fact.fact.kind(),
                wire::FactKind::RoomPublished | wire::FactKind::RoomMessaged
            ) {
                return Err(GuardError::Invalid(format!(
                    "room publication index names {} fact {seq}",
                    fact.fact.kind()
                )));
            }
            decoded = decoded.saturating_add(length);
            facts.push(fact);
        }
        Ok(SelectedFactPage {
            rows: facts,
            continuation,
            horizon: Some(horizon),
        })
    }

    /// Journal position of one room publication through the current
    /// horizon, absent when `publication` names none in `room`.
    pub fn room_publication_seq(
        &self,
        room: &wire::TaskStamp,
        publication: &RoomAuthorityStamp,
    ) -> Result<Option<wire::Seq>, GuardError> {
        let Some(horizon) = self.room_horizon()? else {
            return Ok(None);
        };
        read_metrics::observe_query();
        let seq = self
            .connection
            .query_row(
                "SELECT seq FROM room_publications \
                 WHERE room = ?1 AND publication = ?2 AND seq <= ?3",
                params![room.as_str(), publication.as_str(), horizon],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(GuardError::Store)?;
        seq.map(|seq| {
            u64::try_from(seq)
                .ok()
                .and_then(wire::Seq::new)
                .ok_or_else(|| {
                    GuardError::Invalid(format!("room publication seq {seq} is invalid"))
                })
        })
        .transpose()
    }

    /// How many publications `room` holds strictly after `after`, through
    /// the current horizon.
    pub fn room_publication_count(
        &self,
        room: &wire::TaskStamp,
        after: Option<wire::Seq>,
    ) -> Result<usize, GuardError> {
        let Some(horizon) = self.room_horizon()? else {
            return Ok(0);
        };
        let after = after
            .map_or(Ok(0), |seq| i64::try_from(seq.get()))
            .map_err(|_| {
                GuardError::Invalid("room publication cursor exceeds SQLite".to_owned())
            })?;
        read_metrics::observe_query();
        let count = self
            .connection
            .query_row(
                "SELECT count(*) FROM room_publications \
                 WHERE room = ?1 AND seq > ?2 AND seq <= ?3",
                params![room.as_str(), after, horizon],
                |row| row.get::<_, i64>(0),
            )
            .map_err(GuardError::Store)?;
        usize::try_from(count)
            .map_err(|_| GuardError::Invalid(format!("room publication count {count} is invalid")))
    }

    /// The read's horizon as SQLite spells it, absent on an empty journal.
    fn room_horizon(&self) -> Result<Option<i64>, GuardError> {
        self.cursor
            .map(|horizon| {
                i64::try_from(horizon.get()).map_err(|_| {
                    GuardError::Invalid("room publication horizon exceeds SQLite".to_owned())
                })
            })
            .transpose()
    }
}

fn probe_limit(budget: FactReadBudget) -> Result<i64, GuardError> {
    let limit = budget
        .rows()
        .checked_add(1)
        .expect("bounded room page probe");
    i64::try_from(limit)
        .map_err(|_| GuardError::Invalid("room read limit exceeds SQLite".to_owned()))
}

fn indexed_length(
    row: &rusqlite::Row<'_>,
    column: usize,
    class: &str,
) -> Result<usize, GuardError> {
    let length = row.get::<_, i64>(column).map_err(GuardError::Store)?;
    usize::try_from(length)
        .map_err(|_| GuardError::Invalid(format!("{class} has an invalid byte length")))
}

//! Bounded reads of the room-authority projection.

use rusqlite::{OptionalExtension, params};
use wire::rooms::{
    CandidateCoverage, CandidateCovered, CandidateCritique, CandidateCritiqued, CandidateRetained,
    ExecutionAuthorization, ExecutionAuthorized, PreparationDecided, PreparationDecision,
    ProposalRef, QuestionResultHead, QuestionResultRecord, QuestionResultRef, RetainedCandidate,
    RoomAuthorityStamp, RoomCoordinationStarted, RoomDelivered, RoomHorizon, RoomMessaged,
    RoomPublication, RoomPublished, RoomPutAside, RoomReceiptHead, RoomReceiptRecord,
    RoomReceiptRef, WorkingGroup,
};

use super::{
    CurrentHeadWitness, FactReadBudget, GuardError, JournalHead, JournalRead, SelectedFactPage,
    admitted_envelope_length, decode_bounded_fact, decode_head, read_metrics,
};

mod relevance;

pub use relevance::{RoomRelevanceMetrics, RoomRelevancePage, RoomRelevanceRecord};

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

/// One ordered page of operator items still owing a disposition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenRoomItemPage {
    /// Open item records and their current guarded heads, in item order.
    pub rows: Vec<(RoomReceiptRecord, JournalHead<RoomReceiptHead>)>,
    /// Number of open items at or after the requested cursor.
    pub population: usize,
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

    /// One bounded journal-ordered page of retained preparation authority for
    /// `controller`, strictly after `after`, through the current horizon.
    pub fn preparation_authority_page(
        &self,
        controller: &wire::TaskStamp,
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
                "preparation authority cursor exceeds SQLite".to_owned(),
            ));
        };
        read_metrics::observe_query();
        let mut statement = self
            .connection
            .prepare(
                "WITH authority(seq) AS (\
                     SELECT head_seq FROM proposal_heads WHERE controller = ?1 \
                     UNION \
                     SELECT authorization.head_seq FROM authorization_heads AS authorization \
                     JOIN proposal_heads AS decision ON decision.subject = authorization.decision \
                     WHERE decision.controller = ?1\
                 ) \
                 SELECT facts.seq, facts.kind, length(facts.envelope), facts.envelope \
                 FROM authority JOIN fact_envelopes AS facts ON facts.seq = authority.seq \
                 WHERE facts.seq > ?2 AND facts.seq <= ?3 \
                 ORDER BY facts.seq LIMIT ?4",
            )
            .map_err(GuardError::Store)?;
        let probe = probe_limit(budget)?;
        let mut rows = statement
            .query(params![controller.as_str(), after, horizon_sql, probe])
            .map_err(GuardError::Store)?;
        let mut facts = Vec::new();
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
                continuation = facts.last().map(|fact: &wire::Envelope<_>| fact.seq);
                break;
            }
            let (fact, length) = decode_bounded_fact(row, seq, 1, 2, 3)?;
            if preparation_controller(&fact.fact).as_ref() != Some(controller) {
                return Err(GuardError::Invalid(format!(
                    "preparation authority index for {controller} names {} fact {seq}",
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

    /// Number of retained preparation records for `controller` strictly after
    /// `after`, through the current horizon.
    pub fn preparation_authority_count(
        &self,
        controller: &wire::TaskStamp,
        after: Option<wire::Seq>,
    ) -> Result<usize, GuardError> {
        let Some(horizon) = self.cursor else {
            return Ok(0);
        };
        let after = after
            .map_or(Ok(0), |seq| i64::try_from(seq.get()))
            .map_err(|_| {
                GuardError::Invalid("preparation authority cursor exceeds SQLite".to_owned())
            })?;
        let horizon = i64::try_from(horizon.get()).map_err(|_| {
            GuardError::Invalid("preparation authority horizon exceeds SQLite".to_owned())
        })?;
        read_metrics::observe_query();
        let count = self
            .connection
            .query_row(
                "WITH authority(seq) AS (\
                     SELECT head_seq FROM proposal_heads WHERE controller = ?1 \
                     UNION \
                     SELECT authorization.head_seq FROM authorization_heads AS authorization \
                     JOIN proposal_heads AS decision ON decision.subject = authorization.decision \
                     WHERE decision.controller = ?1\
                 ) \
                 SELECT count(*) FROM authority WHERE seq > ?2 AND seq <= ?3",
                params![controller.as_str(), after, horizon],
                |row| row.get::<_, i64>(0),
            )
            .map_err(GuardError::Store)?;
        usize::try_from(count).map_err(|_| {
            GuardError::Invalid(format!("preparation authority count {count} exceeds usize"))
        })
    }

    /// Whether `position` is one retained preparation record for `controller`.
    pub fn preparation_authority_contains(
        &self,
        controller: &wire::TaskStamp,
        position: wire::Seq,
    ) -> Result<bool, GuardError> {
        let Some(horizon) = self.cursor else {
            return Ok(false);
        };
        let position = i64::try_from(position.get()).map_err(|_| {
            GuardError::Invalid("preparation authority cursor exceeds SQLite".to_owned())
        })?;
        let horizon = i64::try_from(horizon.get()).map_err(|_| {
            GuardError::Invalid("preparation authority horizon exceeds SQLite".to_owned())
        })?;
        read_metrics::observe_query();
        self.connection
            .query_row(
                "WITH authority(seq) AS (\
                     SELECT head_seq FROM proposal_heads WHERE controller = ?1 \
                     UNION \
                     SELECT authorization.head_seq FROM authorization_heads AS authorization \
                     JOIN proposal_heads AS decision ON decision.subject = authorization.decision \
                     WHERE decision.controller = ?1\
                 ) \
                 SELECT EXISTS(SELECT 1 FROM authority WHERE seq = ?2 AND seq <= ?3)",
                params![controller.as_str(), position, horizon],
                |row| row.get(0),
            )
            .map_err(GuardError::Store)
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

    /// One logical lane's room catch-up horizon, absent until the resident
    /// records the lane's first start; the head's position is the horizon.
    pub fn room_catch_up_horizon(
        &self,
        lane: wire::CanonicalUuid,
    ) -> Result<Option<JournalHead<RoomHorizon>>, GuardError> {
        let key = lane.to_string();
        let head = self.text_head::<RoomHorizon>("room_horizons", "lane", &key)?;
        if let Some(head) = &head
            && head.body.lane != lane
        {
            return Err(GuardError::Invalid(format!(
                "room horizon indexed as {key} names lane {}",
                head.body.lane
            )));
        }
        Ok(head)
    }

    /// Latest room publication authored by one exact actor through the current
    /// horizon. Catch-up uses this bounded row to retain the lane's current
    /// activity without replaying the room's retained publication history.
    pub fn latest_room_publication(
        &self,
        room: &wire::TaskStamp,
        actor: &wire::Actor,
    ) -> Result<Option<wire::Envelope<wire::AnyFact>>, GuardError> {
        let Some(horizon) = self.cursor else {
            return Ok(None);
        };
        let horizon = i64::try_from(horizon.get()).map_err(|_| {
            GuardError::Invalid("room publication horizon exceeds SQLite".to_owned())
        })?;
        read_metrics::observe_query();
        let mut statement = self
            .connection
            .prepare(
                "SELECT facts.seq, facts.kind, length(facts.envelope), facts.envelope \
                 FROM room_publications AS published \
                 JOIN fact_envelopes AS facts ON facts.seq = published.seq \
                 WHERE published.room = ?1 AND facts.actor = ?2 AND published.seq <= ?3 \
                 ORDER BY published.seq DESC LIMIT 1",
            )
            .map_err(GuardError::Store)?;
        let mut rows = statement
            .query(params![room.as_str(), actor.to_string(), horizon])
            .map_err(GuardError::Store)?;
        let Some(row) = rows.next().map_err(GuardError::Store)? else {
            return Ok(None);
        };
        let seq = row.get::<_, i64>(0).map_err(GuardError::Store)?;
        read_metrics::observe_row();
        let publication = decode_bounded_fact(row, seq, 1, 2, 3)?.0;
        if publication.actor != *actor {
            return Err(GuardError::Invalid(format!(
                "room publication actor index names {}, decoded {}",
                actor, publication.actor
            )));
        }
        let named = publication
            .fact
            .read::<RoomPublished>()
            .map(|body| body.room)
            .or_else(|| {
                publication
                    .fact
                    .read::<RoomMessaged>()
                    .map(|body| body.room)
            });
        if named.as_ref() != Some(room) {
            return Err(GuardError::Invalid(format!(
                "room publication index for {room} names {named:?}"
            )));
        }
        Ok(Some(publication))
    }
    /// A room's latest `room::directed` record, absent before its first one.
    pub fn room_direction(
        &self,
        room: &wire::TaskStamp,
    ) -> Result<Option<JournalHead<RoomCoordinationStarted>>, GuardError> {
        let head =
            self.text_head::<RoomCoordinationStarted>("room_directions", "room", room.as_str())?;
        require_room(head, room, |body| &body.room)
    }

    /// A room's latest `room::shelved` record, absent before its first one.
    pub fn room_shelving(
        &self,
        room: &wire::TaskStamp,
    ) -> Result<Option<JournalHead<RoomPutAside>>, GuardError> {
        let head = self.text_head::<RoomPutAside>("room_shelvings", "room", room.as_str())?;
        require_room(head, room, |body| &body.room)
    }

    /// One operator item's immutable record and current guarded head, absent
    /// before its publication opens it; the head's position is its latest move.
    pub fn room_item(
        &self,
        item: &RoomReceiptRef,
    ) -> Result<Option<(RoomReceiptRecord, JournalHead<RoomReceiptHead>)>, GuardError> {
        let key = item.to_string();
        let head = self.conversation_head::<RoomReceiptRecord, RoomReceiptHead>(
            "room_items",
            "item",
            &key,
        )?;
        if let Some((record, head)) = &head
            && (record.receipt() != item || head.body.receipt() != item)
        {
            return Err(GuardError::Invalid(format!(
                "operator item indexed as {key} names another item"
            )));
        }
        Ok(head)
    }

    /// One question's immutable result record and current guarded result
    /// head, absent before its ASK; the head's position is its latest move.
    pub fn question_result(
        &self,
        result: &QuestionResultRef,
    ) -> Result<Option<(QuestionResultRecord, JournalHead<QuestionResultHead>)>, GuardError> {
        let key = result.to_string();
        let head = self.conversation_head::<QuestionResultRecord, QuestionResultHead>(
            "room_results",
            "result",
            &key,
        )?;
        if let Some((record, head)) = &head
            && (record.result() != result || head.body.result() != result)
        {
            return Err(GuardError::Invalid(format!(
                "question result indexed as {key} names another result"
            )));
        }
        Ok(head)
    }

    fn conversation_head<R, H>(
        &self,
        table: &'static str,
        key_column: &'static str,
        key: &str,
    ) -> Result<Option<(R, JournalHead<H>)>, GuardError>
    where
        R: serde::de::DeserializeOwned + serde::Serialize,
        H: serde::de::DeserializeOwned + serde::Serialize,
    {
        let statement =
            format!("SELECT head_seq, record, head FROM {table} WHERE {key_column} = ?1");
        read_metrics::observe_query();
        let row = self
            .connection
            .query_row(&statement, [key], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Vec<u8>>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .optional()
            .map_err(GuardError::Store)?;
        let Some((position, record, head)) = row else {
            return Ok(None);
        };
        self.conversation_row(table, key_column, key, position, &record, &head)
            .map(Some)
    }

    /// Every pending question result's immutable record and current guarded
    /// head, in result order.
    pub fn pending_question_results(
        &self,
    ) -> Result<Vec<(QuestionResultRecord, JournalHead<QuestionResultHead>)>, GuardError> {
        let rows = self.conversation_rows::<QuestionResultRecord, QuestionResultHead>(
            "room_results",
            "result",
            "pending = 1",
        )?;
        rows.into_iter()
            .map(|(key, record, head)| {
                if record.result().to_string() != key || head.body.result().to_string() != key {
                    return Err(GuardError::Invalid(format!(
                        "question result indexed as {key} names another result"
                    )));
                }
                Ok((record, head))
            })
            .collect()
    }

    /// Every settled question result in a task room that no shadow relevance
    /// observation names yet, in result order. The observation shares its
    /// result's identity, so each result is observed at most once.
    pub fn unobserved_question_results(
        &self,
    ) -> Result<Vec<(QuestionResultRecord, JournalHead<QuestionResultHead>)>, GuardError> {
        let rows = self.conversation_rows::<QuestionResultRecord, QuestionResultHead>(
            "room_results",
            "result",
            "pending = 0 \
             AND NOT EXISTS (SELECT 1 FROM room_relevance_observations AS observed \
                             WHERE observed.observation = room_results.result) \
             AND json_extract(CAST(record AS TEXT), '$.context') IS NOT NULL",
        )?;
        rows.into_iter()
            .map(|(key, record, head)| {
                if record.result().to_string() != key || head.body.result().to_string() != key {
                    return Err(GuardError::Invalid(format!(
                        "question result indexed as {key} names another result"
                    )));
                }
                Ok((record, head))
            })
            .collect()
    }

    /// Every operator item that still owes a disposition, pending or lapsed,
    /// with its immutable record and current guarded head, in item order.
    pub fn open_room_items(
        &self,
    ) -> Result<Vec<(RoomReceiptRecord, JournalHead<RoomReceiptHead>)>, GuardError> {
        let rows = self.conversation_rows::<RoomReceiptRecord, RoomReceiptHead>(
            "room_items",
            "item",
            "open = 1",
        )?;
        rows.into_iter()
            .map(|(key, record, head)| {
                if record.receipt().to_string() != key || head.body.receipt().to_string() != key {
                    return Err(GuardError::Invalid(format!(
                        "operator item indexed as {key} names another item"
                    )));
                }
                Ok((record, head))
            })
            .collect()
    }

    /// At most `limit` operator items still owing a disposition, pending or
    /// lapsed, at or after `from` in item order, with how many such items lie
    /// at or after `from` in all.
    pub fn open_room_items_page(
        &self,
        from: Option<&RoomReceiptRef>,
        limit: usize,
    ) -> Result<OpenRoomItemPage, GuardError> {
        let from = from.map_or_else(String::new, ToString::to_string);
        let limit = i64::try_from(limit)
            .map_err(|_| GuardError::Invalid("operator item page exceeds SQLite".to_owned()))?;
        read_metrics::observe_query();
        let population = self
            .connection
            .query_row(
                "SELECT count(*) FROM room_items WHERE open = 1 AND item >= ?1",
                [&from],
                |row| row.get::<_, i64>(0),
            )
            .map_err(GuardError::Store)?;
        let population = usize::try_from(population).map_err(|_| {
            GuardError::Invalid(format!("operator item count {population} is invalid"))
        })?;
        read_metrics::observe_query();
        let mut statement = self
            .connection
            .prepare(
                "SELECT item, head_seq, record, head FROM room_items \
                 WHERE open = 1 AND item >= ?1 ORDER BY item LIMIT ?2",
            )
            .map_err(GuardError::Store)?;
        let rows = statement
            .query_map(params![from, limit], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            })
            .map_err(GuardError::Store)?;
        let mut items = Vec::new();
        for row in rows {
            let (key, position, record, head) = row.map_err(GuardError::Store)?;
            let (record, head) = self.conversation_row::<RoomReceiptRecord, RoomReceiptHead>(
                "room_items",
                "item",
                &key,
                position,
                &record,
                &head,
            )?;
            if record.receipt().to_string() != key || head.body.receipt().to_string() != key {
                return Err(GuardError::Invalid(format!(
                    "operator item indexed as {key} names another item"
                )));
            }
            items.push((record, head));
        }
        Ok(OpenRoomItemPage {
            rows: items,
            population,
        })
    }

    /// One admitted conversation publication and its journal position.
    pub fn room_conversation(
        &self,
        publication: &RoomAuthorityStamp,
    ) -> Result<Option<(wire::Seq, RoomPublication)>, GuardError> {
        self.conversation_where("publication = ?1", publication.to_string())
    }

    /// The first conversation publication admitted strictly after `seq`.
    ///
    /// A lane packet's room records precede its conversation record in one
    /// write, so this finds that record; the caller proves the match by source.
    pub fn room_conversation_after(
        &self,
        seq: wire::Seq,
    ) -> Result<Option<(wire::Seq, RoomPublication)>, GuardError> {
        let seq = i64::try_from(seq.get())
            .map_err(|_| GuardError::Invalid("conversation cursor exceeds SQLite".to_owned()))?;
        self.conversation_where("head_seq > ?1 ORDER BY head_seq LIMIT 1", seq)
    }

    fn conversation_where(
        &self,
        condition: &'static str,
        value: impl rusqlite::ToSql,
    ) -> Result<Option<(wire::Seq, RoomPublication)>, GuardError> {
        read_metrics::observe_query();
        let row = self
            .connection
            .query_row(
                &format!("SELECT head_seq, body FROM room_conversations WHERE {condition}"),
                [value],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
            )
            .optional()
            .map_err(GuardError::Store)?;
        let Some((position, body)) = row else {
            return Ok(None);
        };
        read_metrics::observe_row();
        read_metrics::observe_decode(body.len());
        let seq = u64::try_from(position)
            .ok()
            .and_then(wire::Seq::new)
            .filter(|seq| self.cursor.is_some_and(|cursor| *seq <= cursor))
            .ok_or_else(|| {
                GuardError::Invalid(format!("conversation position {position} is invalid"))
            })?;
        let publication = wire::decode::<RoomPublication>(&body)
            .map_err(|error| GuardError::Invalid(format!("conversation {position}: {error}")))?;
        Ok(Some((seq, publication)))
    }

    /// The admitted envelope at `seq`, absent beyond the read cursor.
    pub fn room_fact(
        &self,
        seq: wire::Seq,
    ) -> Result<Option<wire::Envelope<wire::AnyFact>>, GuardError> {
        if self.cursor.is_none_or(|cursor| seq > cursor) {
            return Ok(None);
        }
        let seq = i64::try_from(seq.get())
            .map_err(|_| GuardError::Invalid("fact position exceeds SQLite".to_owned()))?;
        read_metrics::observe_query();
        let mut statement = self
            .connection
            .prepare(
                "SELECT seq, kind, length(envelope), envelope FROM fact_envelopes WHERE seq = ?1",
            )
            .map_err(GuardError::Store)?;
        let mut rows = statement.query([seq]).map_err(GuardError::Store)?;
        let Some(row) = rows.next().map_err(GuardError::Store)? else {
            return Ok(None);
        };
        read_metrics::observe_row();
        Ok(Some(decode_bounded_fact(row, seq, 1, 2, 3)?.0))
    }

    /// Every conversation head row of `table` meeting `condition`, keyed by
    /// its stored key, in key order.
    fn conversation_rows<R, H>(
        &self,
        table: &'static str,
        key_column: &'static str,
        condition: &'static str,
    ) -> Result<Vec<(String, R, JournalHead<H>)>, GuardError>
    where
        R: serde::de::DeserializeOwned + serde::Serialize,
        H: serde::de::DeserializeOwned + serde::Serialize,
    {
        let statement = format!(
            "SELECT {key_column}, head_seq, record, head FROM {table} \
             WHERE {condition} ORDER BY {key_column}"
        );
        read_metrics::observe_query();
        let mut statement = self
            .connection
            .prepare(&statement)
            .map_err(GuardError::Store)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            })
            .map_err(GuardError::Store)?;
        let mut heads = Vec::new();
        for row in rows {
            let (key, position, record, head) = row.map_err(GuardError::Store)?;
            let (record, head) =
                self.conversation_row(table, key_column, &key, position, &record, &head)?;
            heads.push((key, record, head));
        }
        Ok(heads)
    }

    fn conversation_row<R, H>(
        &self,
        table: &'static str,
        key_column: &'static str,
        key: &str,
        position: i64,
        record: &[u8],
        head: &[u8],
    ) -> Result<(R, JournalHead<H>), GuardError>
    where
        R: serde::de::DeserializeOwned + serde::Serialize,
        H: serde::de::DeserializeOwned + serde::Serialize,
    {
        read_metrics::observe_row();
        read_metrics::observe_decode(record.len() + head.len());
        let invalid = |error: GuardError| {
            let detail = match error {
                GuardError::Invalid(detail) => detail,
                other => other.to_string(),
            };
            GuardError::Invalid(format!("{table} {key_column} {key:?} is invalid: {detail}"))
        };
        let record = decode_head::<R>(position, record).map_err(invalid)?.body;
        let head = decode_head::<H>(position, head).map_err(invalid)?;
        if self.cursor.is_none_or(|cursor| head.position > cursor) {
            return Err(GuardError::Invalid(format!(
                "{table} {key_column} {key:?} head position {} is beyond cursor {:?}",
                head.position,
                self.cursor.map(wire::Seq::get)
            )));
        }
        Ok((record, head))
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
                wire::FactKind::RoomPublished
                    | wire::FactKind::RoomMessaged
                    | wire::FactKind::RoomPosted
                    | wire::FactKind::RoomCoordinationStarted
                    | wire::FactKind::RoomPutAside
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

    /// One page of the facts behind current room authority through
    /// `horizon`, in journal order: every room publication, the current head
    /// of each working group, proposal, and authorization, for each room
    /// coordination the direction it follows and that direction's current
    /// steering head, every conversation record with the current head of
    /// each operator item and question result, and the opening and current
    /// steering head of each directive a publication or conversation names.
    ///
    /// A head that moves past `horizon` leaves this set, so the facts after
    /// `horizon` complete the authority it describes without overlap.
    pub fn room_authority_page(
        &self,
        start: crate::FactPageStart,
        horizon: wire::Seq,
        budget: FactReadBudget,
    ) -> Result<wire::Packed<wire::Envelope<wire::AnyFact>>, GuardError> {
        if self.cursor.is_none_or(|cursor| horizon > cursor) {
            return Err(GuardError::Invalid(format!(
                "room authority horizon {horizon} is beyond cursor {:?}",
                self.cursor
            )));
        }
        let first = match start {
            crate::FactPageStart::After(after) => after.map_or(1, |after| after.get() + 1),
            crate::FactPageStart::At(position) => position.get(),
        };
        let sql = |value: u64| {
            i64::try_from(value)
                .map_err(|_| GuardError::Invalid("room authority position exceeds SQLite".into()))
        };
        let (first, last) = (sql(first)?, sql(horizon.get())?);
        let directed = wire::FactKind::RoomCoordinationStarted.as_str();
        read_metrics::observe_query();
        let mut statement = self
            .connection
            .prepare(ROOM_AUTHORITY_PAGE)
            .map_err(GuardError::Store)?;
        let mut rows = statement
            .query(params![directed, first, last, probe_limit(budget)?])
            .map_err(GuardError::Store)?;
        let mut facts = Vec::with_capacity(budget.rows());
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
                continuation = Some(seq);
                break;
            }
            let (fact, length) = decode_bounded_fact(row, seq, 1, 2, 3)?;
            decoded = decoded.saturating_add(length);
            facts.push(fact);
        }
        drop(rows);
        let overflow = continuation
            .map(|continuation| {
                read_metrics::observe_query();
                let omitted = self
                    .connection
                    .query_row(
                        ROOM_AUTHORITY_REMAINING,
                        params![directed, continuation, last],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(GuardError::Store)?;
                Ok(wire::Overflow {
                    omitted: u64::try_from(omitted).map_err(|_| {
                        GuardError::Invalid(format!("room authority count {omitted} is invalid"))
                    })?,
                    continuation: u64::try_from(continuation)
                        .ok()
                        .and_then(wire::Seq::new)
                        .ok_or_else(|| {
                            GuardError::Invalid(format!(
                                "room authority position {continuation} is invalid"
                            ))
                        })?,
                })
            })
            .transpose()?;
        Ok(wire::Packed {
            rows: facts,
            overflow,
            horizon,
        })
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

/// Positions of the facts behind current room authority; `?1` spells the
/// room-coordination kind, whose direction always sits one position earlier.
/// A publication's or conversation's lane deliveries are the directives its
/// own fact names.
macro_rules! room_authority {
    () => {
        "WITH directed(seq) AS ( \
             SELECT p.seq FROM room_publications p \
             JOIN fact_envelopes f ON f.seq = p.seq WHERE f.kind = ?1 \
         ), recorded(seq) AS ( \
             SELECT seq FROM room_publications \
             UNION SELECT head_seq FROM room_conversations \
         ), delivered(directive) AS ( \
             SELECT j.value FROM recorded r \
             JOIN fact_envelopes f ON f.seq = r.seq, \
             json_each(CAST(f.envelope AS TEXT), '$.fact.directed') j \
         ), authority(seq) AS ( \
             SELECT seq FROM recorded \
             UNION SELECT head_seq FROM group_heads \
             UNION SELECT head_seq FROM proposal_heads \
             UNION SELECT head_seq FROM authorization_heads \
             UNION SELECT head_seq FROM room_items \
             UNION SELECT head_seq FROM room_results \
             UNION SELECT seq - 1 FROM directed \
             UNION SELECT h.head_seq FROM directed d \
             JOIN steering_heads h ON h.created_seq = d.seq - 1 \
             UNION SELECT h.created_seq FROM delivered d \
             JOIN steering_heads h ON h.directive = d.directive \
             UNION SELECT h.head_seq FROM delivered d \
             JOIN steering_heads h ON h.directive = d.directive \
         ) "
    };
}

const ROOM_AUTHORITY_PAGE: &str = concat!(
    room_authority!(),
    "SELECT f.seq, f.kind, length(f.envelope), f.envelope FROM authority a \
     JOIN fact_envelopes f ON f.seq = a.seq \
     WHERE a.seq >= ?2 AND a.seq <= ?3 ORDER BY a.seq LIMIT ?4"
);

const ROOM_AUTHORITY_REMAINING: &str = concat!(
    room_authority!(),
    "SELECT count(*) FROM authority WHERE seq >= ?2 AND seq <= ?3"
);

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

fn preparation_controller(fact: &wire::AnyFact) -> Option<wire::TaskStamp> {
    if let Some(body) = fact.read::<CandidateRetained>() {
        Some(body.candidate.candidate.controller)
    } else if let Some(body) = fact.read::<CandidateCritiqued>() {
        Some(body.critique.critique.candidate.controller)
    } else if let Some(body) = fact.read::<CandidateCovered>() {
        Some(body.coverage.coverage.candidate.controller)
    } else if let Some(body) = fact.read::<PreparationDecided>() {
        Some(body.decision.decision.controller)
    } else {
        fact.read::<ExecutionAuthorized>()
            .map(|body| body.authorization.decision.controller)
    }
}

fn require_room<T>(
    head: Option<JournalHead<T>>,
    room: &wire::TaskStamp,
    named: impl Fn(&T) -> &wire::TaskStamp,
) -> Result<Option<JournalHead<T>>, GuardError> {
    match head {
        Some(head) if named(&head.body) != room => Err(GuardError::Invalid(format!(
            "room coordination head indexed as {room} names room {}",
            named(&head.body)
        ))),
        head => Ok(head),
    }
}

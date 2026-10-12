//! Bounded reads of the room-authority projection.

use rusqlite::params;
use wire::rooms::{
    CandidateCoverage, CandidateCritique, ExecutionAuthorization, PreparationDecision, ProposalRef,
    RetainedCandidate, RoomAuthorityStamp, WorkingGroup,
};

use super::{
    FactReadBudget, GuardError, JournalHead, JournalRead, SelectedFactPage,
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

    /// Current working groups strictly after `after`, in identity order.
    pub fn group_heads(
        &self,
        after: Option<&RoomAuthorityStamp>,
        budget: FactReadBudget,
    ) -> Result<Vec<JournalHead<WorkingGroup>>, GuardError> {
        let after = after.map(ToString::to_string).unwrap_or_default();
        read_metrics::observe_query();
        let mut statement = self
            .connection
            .prepare(
                "SELECT head_seq, body FROM group_heads WHERE working_group > ?1 \
                 ORDER BY working_group LIMIT ?2",
            )
            .map_err(GuardError::Store)?;
        let rows = statement
            .query_map(params![after, limit(budget)?], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .map_err(GuardError::Store)?;
        let mut heads = Vec::new();
        for row in rows {
            let (position, bytes) = row.map_err(GuardError::Store)?;
            read_metrics::observe_row();
            read_metrics::observe_decode(bytes.len());
            heads.push(decode_head(position, &bytes)?);
        }
        Ok(heads)
    }

    /// Working groups whose order includes this room anchor, in identity order.
    pub fn anchor_groups(
        &self,
        anchor: &wire::TaskStamp,
    ) -> Result<Vec<RoomAuthorityStamp>, GuardError> {
        read_metrics::observe_query();
        let mut statement = self
            .connection
            .prepare(
                "SELECT working_group FROM group_anchors WHERE anchor = ?1 \
                 ORDER BY working_group",
            )
            .map_err(GuardError::Store)?;
        let rows = statement
            .query_map([anchor.as_str()], |row| row.get::<_, String>(0))
            .map_err(GuardError::Store)?;
        let mut groups = Vec::new();
        for row in rows {
            read_metrics::observe_row();
            let group = row.map_err(GuardError::Store)?;
            groups.push(group.parse().map_err(|error| {
                GuardError::Invalid(format!("group anchor index names {group:?}: {error}"))
            })?);
        }
        Ok(groups)
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
        let probe = limit(budget)?.saturating_add(1);
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
            if fact.fact.kind() != wire::FactKind::RoomPublished {
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
}

fn limit(budget: FactReadBudget) -> Result<i64, GuardError> {
    i64::try_from(budget.rows())
        .map_err(|_| GuardError::Invalid("room read limit exceeds SQLite".to_owned()))
}

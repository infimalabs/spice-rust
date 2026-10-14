//! Bounded journal-ordered pages of one controller's operator decisions.

use rusqlite::params;

use super::{
    FactReadBudget, GuardError, JournalRead, SelectedFactPage, admitted_envelope_length,
    decode_bounded_fact, read_metrics,
};

impl JournalRead<'_> {
    /// One bounded page of `controller`'s recorded `proposal::decided` facts
    /// strictly after `after`, in decision order, through the current horizon.
    pub fn preparation_decisions(
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
        let invalid =
            || GuardError::Invalid("preparation decision cursor exceeds SQLite".to_owned());
        let after = i64::try_from(after.map_or(0, wire::Seq::get)).map_err(|_| invalid())?;
        let horizon_sql = i64::try_from(horizon.get()).map_err(|_| invalid())?;
        let probe = budget
            .rows()
            .checked_add(1)
            .and_then(|limit| i64::try_from(limit).ok())
            .ok_or_else(invalid)?;
        read_metrics::observe_query();
        let mut statement = self
            .connection
            .prepare(
                "SELECT facts.seq, facts.kind, length(facts.envelope), facts.envelope \
                 FROM proposal_heads AS heads \
                 JOIN fact_envelopes AS facts ON facts.seq = heads.head_seq \
                 WHERE heads.controller = ?1 AND facts.kind = ?2 \
                 AND facts.seq > ?3 AND facts.seq <= ?4 \
                 ORDER BY facts.seq LIMIT ?5",
            )
            .map_err(GuardError::Store)?;
        let mut rows = statement
            .query(params![
                controller.as_str(),
                wire::FactKind::PreparationDecided.as_str(),
                after,
                horizon_sql,
                probe
            ])
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
            if fact
                .fact
                .read::<wire::rooms::PreparationDecided>()
                .is_none_or(|decided| decided.decision.decision.controller != *controller)
            {
                return Err(GuardError::Invalid(format!(
                    "decision index for {controller} names another fact {seq}"
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

//! Build bounded Design maturity and per-Design notice indexes from immutable facts.

use rusqlite::{OptionalExtension, Transaction, params};
use wire::rooms::{DesignMaturity, DesignNoticeProvenance, DesignNoticed, RoomContributed};
use wire::{AnyFact, Envelope, FactKind};

use super::{MigrationError, OpenError};

/// Release that introduced the bounded Design maturity projection.
const MATURITY_RELEASE: &str = "ROOMS-dW6GkN3W6";

/// Create the projection from current-shape facts without rewriting any fact.
pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    transaction
        .execute_batch("DELETE FROM design_notices; DELETE FROM design_maturity;")
        .map_err(OpenError::Sqlite)?;
    let mut facts = transaction
        .prepare("SELECT seq, envelope FROM fact_envelopes WHERE kind IN (?1, ?2) ORDER BY seq")
        .map_err(OpenError::Sqlite)?;
    let mut rows = facts
        .query([
            FactKind::DesignNoticed.as_str(),
            FactKind::RoomContributed.as_str(),
        ])
        .map_err(OpenError::Sqlite)?;
    let mut notice_insert = transaction
        .prepare("INSERT INTO design_notices(design, seq) VALUES (?1, ?2)")
        .map_err(OpenError::Sqlite)?;
    let mut maturity_read = transaction
        .prepare("SELECT body FROM design_maturity WHERE design = ?1")
        .map_err(OpenError::Sqlite)?;
    let mut maturity_write = transaction
        .prepare(
            "INSERT INTO design_maturity(design, body) VALUES (?1, ?2) \
             ON CONFLICT(design) DO UPDATE SET body = excluded.body",
        )
        .map_err(OpenError::Sqlite)?;
    while let Some(row) = rows.next().map_err(OpenError::Sqlite)? {
        let seq = row.get::<_, i64>(0).map_err(OpenError::Sqlite)?;
        let bytes = row.get::<_, Vec<u8>>(1).map_err(OpenError::Sqlite)?;
        let envelope: Envelope<AnyFact> = wire::decode(&bytes)
            .map_err(|error| refusal(seq, &format!("retained fact does not decode: {error}")))?;
        let (design, notice, contribution) =
            if let Some(notice) = envelope.fact.read::<DesignNoticed>() {
                notice_insert
                    .execute(params![notice.design.as_str(), seq])
                    .map_err(OpenError::Sqlite)?;
                if notice.provenance != DesignNoticeProvenance::Organic {
                    continue;
                }
                (notice.design.clone(), Some(notice), None)
            } else if let Some(contribution) = envelope.fact.read::<RoomContributed>() {
                (contribution.room.clone(), None, Some(contribution))
            } else {
                continue;
            };
        let mut maturity = maturity_read
            .query_row([design.as_str()], |row| row.get::<_, Vec<u8>>(0))
            .optional()
            .map_err(OpenError::Sqlite)?
            .map(|body| wire::decode(&body))
            .transpose()
            .map_err(|error| {
                refusal(
                    seq,
                    &format!("retained maturity projection does not decode: {error}"),
                )
            })?
            .unwrap_or_else(DesignMaturity::default);
        match (notice, contribution) {
            (Some(notice), None) => maturity.observe_notice(&notice),
            (None, Some(contribution)) => maturity.observe_contribution(&contribution),
            _ => unreachable!("one typed maturity fact was selected"),
        }
        let body = wire::encode(&maturity)
            .map_err(|error| refusal(seq, &format!("maturity does not encode: {error}")))?;
        maturity_write
            .execute(params![design.as_str(), body])
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn refusal(seq: i64, detail: &str) -> OpenError {
    MigrationError::RetainedRow {
        table: "fact_envelopes",
        row: seq.to_string(),
        release: MATURITY_RELEASE,
        detail: detail.to_owned(),
    }
    .into()
}

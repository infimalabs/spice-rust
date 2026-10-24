//! Build bounded Design maturity and per-Design notice indexes from immutable facts.

use rusqlite::{OptionalExtension, Transaction, params};
use wire::rooms::{DesignMaturity, DesignNoticed, RoomContributed, RoomLeft};
use wire::{AnyFact, Envelope, FactKind};

use super::{OpenError, residual};

/// Release that introduced the bounded Design maturity projection.
const MATURITY_RELEASE: &str = "ROOMS-dW6GkN3W6";

/// Create the projection from current-shape facts without rewriting any fact.
pub(super) fn cross(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    transaction
        .execute_batch("DELETE FROM design_notices; DELETE FROM design_maturity;")
        .map_err(OpenError::Sqlite)?;
    let mut facts = transaction
        .prepare("SELECT seq, envelope FROM fact_envelopes WHERE kind IN (?1, ?2, ?3) ORDER BY seq")
        .map_err(OpenError::Sqlite)?;
    let mut rows = facts
        .query([
            FactKind::DesignNoticed.as_str(),
            FactKind::RoomContributed.as_str(),
            FactKind::RoomLeft.as_str(),
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
        let fact = Folded::read(&envelope.fact);
        let design = match &fact {
            Some(Folded::Request(notice)) => {
                notice_insert
                    .execute(params![notice.design.as_str(), seq])
                    .map_err(OpenError::Sqlite)?;
                notice.design.clone()
            }
            Some(Folded::Contribution(contribution)) => contribution.room.clone(),
            Some(Folded::Leave(leave)) => leave.room.clone(),
            None => continue,
        };
        let existing = maturity_read
            .query_row([design.as_str()], |row| row.get::<_, Vec<u8>>(0))
            .optional()
            .map_err(OpenError::Sqlite)?
            .map(|body| wire::decode::<DesignMaturity>(&body))
            .transpose()
            .map_err(|error| {
                refusal(
                    seq,
                    &format!("retained maturity projection does not decode: {error}"),
                )
            })?;
        let maturity = match (existing, fact) {
            (existing, Some(Folded::Request(notice))) => {
                let mut maturity = existing.unwrap_or_default();
                maturity.observe_request(&notice);
                maturity
            }
            (existing, Some(Folded::Contribution(contribution))) => {
                let mut maturity = existing.unwrap_or_default();
                maturity.observe_contribution(&contribution);
                maturity
            }
            (Some(mut maturity), Some(Folded::Leave(leave))) => {
                maturity.observe_leave(&leave);
                maturity
            }
            // A leave from a room that no request or contribution reached.
            (None, Some(Folded::Leave(_))) | (_, None) => continue,
        };
        let body = wire::encode(&maturity)
            .map_err(|error| refusal(seq, &format!("maturity does not encode: {error}")))?;
        maturity_write
            .execute(params![design.as_str(), body])
            .map_err(OpenError::Sqlite)?;
    }
    Ok(())
}

fn refusal(seq: i64, detail: &str) -> OpenError {
    residual::retained(
        "fact_envelopes",
        seq.to_string(),
        MATURITY_RELEASE,
        detail.to_owned(),
    )
}

/// One retained fact the maturity projection folds.
enum Folded {
    Request(Box<DesignNoticed>),
    Contribution(RoomContributed),
    Leave(RoomLeft),
}

impl Folded {
    fn read(fact: &AnyFact) -> Option<Self> {
        fact.read::<DesignNoticed>()
            .map(|notice| Self::Request(Box::new(notice)))
            .or_else(|| fact.read::<RoomContributed>().map(Self::Contribution))
            .or_else(|| fact.read::<RoomLeft>().map(Self::Leave))
    }
}

//! Retire each lease warning together with the steering directive it opened.
//!
//! A `claim::warned` fact opened an ordinary steering directive, and later
//! steering facts (an acknowledgment, a cancellation, a missed recognition of
//! its key) name that directive by identity or key. Steering replay refuses a
//! transition of a directive no fact opened, so the warning cannot retire
//! alone: every steering fact naming one of its directives or keys retires
//! with it, and the directive's indexed head and miss evidence drop. A task
//! whose cause names a retired key keeps that cause; its origin read then has
//! no request text to preview.

use std::collections::BTreeSet;

use rusqlite::params;

use crate::OpenError;
use crate::migrate::retire::{self, COLUMNS, Retirement};

pub(super) fn cross(
    transaction: &rusqlite::Transaction<'_>,
    retirement: Retirement,
) -> Result<(), OpenError> {
    let warnings = retire::read_rows(
        transaction,
        &format!("SELECT {COLUMNS} FROM fact_envelopes WHERE kind = 'claim::warned' ORDER BY seq"),
        [],
    )?;
    if warnings.is_empty() {
        return Ok(());
    }
    let mut directives = BTreeSet::new();
    let mut keys = BTreeSet::new();
    for row in &warnings {
        let field = |name: &str| {
            serde_json::from_slice::<wire::Envelope<serde_json::Value>>(&row.envelope)
                .ok()
                .and_then(|envelope| Some(envelope.fact.get(name)?.as_str()?.to_owned()))
                .ok_or_else(|| {
                    retirement.error(row.seq, format!("lease warning names no steering {name}"))
                })
        };
        directives.insert(field("directive")?);
        keys.insert(field("key")?);
    }
    for row in warnings {
        retire::retire_row(transaction, row, retirement)?;
    }
    let directives =
        serde_json::Value::from(directives.into_iter().collect::<Vec<_>>()).to_string();
    let keys = serde_json::Value::from(keys.into_iter().collect::<Vec<_>>()).to_string();
    let dependents = retire::read_rows(
        transaction,
        &format!(
            "SELECT {COLUMNS} FROM fact_envelopes \
             WHERE kind >= 'steer::' AND kind < 'steer;' AND (\
                 json_extract(CAST(envelope AS TEXT), '$.fact.directive') \
                     IN (SELECT value FROM json_each(?1)) \
                 OR json_extract(CAST(envelope AS TEXT), '$.fact.key') \
                     IN (SELECT value FROM json_each(?2))\
             ) ORDER BY seq"
        ),
        params![directives, keys],
    )?;
    for row in dependents {
        let seq = row.seq;
        retire::retire_row(transaction, row, retirement)?;
        transaction
            .execute("DELETE FROM steering_misses WHERE head_seq = ?1", [seq])
            .map_err(OpenError::Sqlite)?;
    }
    transaction
        .execute(
            "DELETE FROM steering_heads WHERE directive IN (SELECT value FROM json_each(?1))",
            [directives],
        )
        .map_err(OpenError::Sqlite)?;
    Ok(())
}

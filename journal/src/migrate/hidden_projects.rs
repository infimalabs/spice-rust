//! Retained hidden projects refuse at their owning release.
//!
//! A project spelled with a leading dot named a hidden project until
//! [`HIDDEN_PROJECT_RELEASE`]; current projects are exact public labels that
//! never begin with a dot. No current label carries such a row forward without
//! inventing one, so any retained task, claim, fact, receipt, or landing naming
//! one refuses the whole crossing transaction, leaving the journal unchanged.

use rusqlite::{OptionalExtension, Transaction};

use super::{OpenError, residual, table_exists};

/// Last release that admitted leading-dot hidden projects.
const HIDDEN_PROJECT_RELEASE: &str = "8aaba9962";

/// Every retained JSON column that can carry a project, with its row key.
const JSON_COLUMNS: [(&str, &str, &str); 7] = [
    ("task_heads", "stamp", "body"),
    ("claim_heads", "stamp", "body"),
    ("fact_envelopes", "seq", "envelope"),
    ("command_receipts", "command_id", "request"),
    ("command_receipts", "command_id", "response"),
    ("historical_command_receipts", "command_id", "request"),
    ("historical_command_receipts", "command_id", "response"),
];

/// Refuse the first retained row that names a hidden project.
pub(super) fn refuse_retained(transaction: &Transaction<'_>) -> Result<(), OpenError> {
    for (table, key, column) in JSON_COLUMNS {
        let sql = format!(
            "SELECT CAST(row.{key} AS TEXT), node.value \
             FROM {table} AS row, json_tree(CAST(row.{column} AS TEXT)) AS node \
             WHERE instr(CAST(row.{column} AS TEXT), '\"project\":\".') > 0 \
             AND node.key = 'project' AND node.type = 'text' AND node.value LIKE '.%' \
             ORDER BY row.{key} LIMIT 1"
        );
        refuse_first(transaction, table, &sql)?;
    }
    refuse_first(
        transaction,
        "release_landings",
        "SELECT commit_id, project FROM release_landings WHERE project LIKE '.%' \
         ORDER BY commit_id LIMIT 1",
    )
}

fn refuse_first(
    transaction: &Transaction<'_>,
    table: &'static str,
    sql: &str,
) -> Result<(), OpenError> {
    if !table_exists(transaction, table)? {
        return Ok(());
    }
    let hidden = transaction
        .query_row(sql, [], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .optional()
        .map_err(OpenError::Sqlite)?;
    match hidden {
        None => Ok(()),
        Some((row, project)) => Err(residual::retained(
            table,
            row,
            HIDDEN_PROJECT_RELEASE,
            format!(
                "it names hidden project {project}, and current projects are exact public \
                 labels that never carry a hidden project forward"
            ),
        )),
    }
}

//! Retained settings command answers crossing into principal display.

use super::*;

/// Last release whose settings answers carried one Git-derived operator identity.
const RETIRED_OPERATOR_RELEASE: &str = "50c8254c";

/// Cross each retained settings mutation answer through the current strict
/// answer DTO. The retired answer named at most one Git-derived operator; when
/// the repository has since born its bootstrap principal, that same recorded
/// identity becomes the bootstrap entry. Answers from before any principal
/// birth carry an empty directory, as they carried no principal identity.
pub(super) fn cross(transaction: &rusqlite::Transaction<'_>) -> Result<(), OpenError> {
    let bootstrap = transaction
        .query_row(
            "SELECT json_extract(CAST(envelope AS TEXT), '$.fact.principal') \
             FROM fact_envelopes WHERE kind = 'principal::admitted' ORDER BY seq LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(OpenError::Sqlite)?
        .map(|principal| {
            principal.parse::<wire::CanonicalUuid>().map_err(|error| {
                receipt_error(
                    "bootstrap",
                    format!("the first principal identity is invalid: {error}"),
                )
            })
        })
        .transpose()?;
    let command = wire::CommandKind::GlobalSettingsSet;
    let receipts = guidance_rows::<String>(
        transaction,
        "SELECT command_id, response FROM command_receipts WHERE command_kind = ?1 \
         ORDER BY command_id",
        command.as_str(),
    )?;
    for (command_id, bytes) in receipts {
        let refuse = |detail: String| receipt_error(&command_id, detail);
        let mut response: wire::Response<serde_json::Value> = wire::decode(&bytes)
            .map_err(|error| refuse(format!("the stored response does not decode: {error}")))?;
        let wire::Outcome::Reply(reply) = &mut response.outcome else {
            return Err(refuse("the stored response is not a reply".to_owned()));
        };
        let body = reply
            .body
            .as_object_mut()
            .ok_or_else(|| refuse("the stored settings answer is not a JSON object".to_owned()))?;
        let crossed = !body.contains_key("principals");
        if crossed {
            let retired = body.remove("operator");
            let principals = match (bootstrap, retired) {
                (Some(principal), Some(identity)) => serde_json::json!([{
                    "principal": principal,
                    "identity": identity,
                }]),
                _ => serde_json::json!([]),
            };
            body.insert("principals".to_owned(), principals);
        }
        let answer = wire::encode(&reply.body)
            .and_then(|body| command.binding().answer.decode(&body))
            .and_then(serde_json::to_value)
            .map_err(|error| refuse(format!("the crossed answer is not current: {error}")))?;
        if !crossed && answer == reply.body {
            continue;
        }
        reply.body = answer;
        let bytes = wire::encode(&response)
            .map_err(|error| refuse(format!("the crossed response cannot encode: {error}")))?;
        let changed = transaction
            .execute(
                "UPDATE command_receipts SET response = ?1 \
                 WHERE command_id = ?2 AND command_kind = ?3",
                params![bytes, command_id, command.as_str()],
            )
            .map_err(OpenError::Sqlite)?;
        if changed != 1 {
            return Err(refuse(
                "the row changed under the exclusive crossing".to_owned(),
            ));
        }
    }
    Ok(())
}

fn receipt_error<T: ToString + ?Sized>(row: &T, detail: String) -> OpenError {
    MigrationError::RetainedRow {
        table: "command_receipts",
        row: row.to_string(),
        release: RETIRED_OPERATOR_RELEASE,
        detail,
    }
    .into()
}

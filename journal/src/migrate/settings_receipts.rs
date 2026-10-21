//! Retained settings command answers crossing into the current principal
//! display.

use wire::principal::{PrincipalAdmitted, PrincipalCode, PrincipalHead};

use super::*;

/// Last release whose settings answers carried one Git-derived operator identity.
const RETIRED_OPERATOR_RELEASE: &str = "50c8254c";
/// Last release whose principal directory entries carried no principal code.
const CODELESS_DIRECTORY_RELEASE: &str = "fc0fd097d";

/// Cross each retained settings mutation answer through the current strict
/// answer DTO. The retired answer named at most one Git-derived operator; when
/// the repository has since born its bootstrap principal, that same recorded
/// identity becomes the bootstrap entry. Answers from before any principal
/// birth carry an empty directory, as they carried no principal identity.
/// Each directory entry carries the code of the principal it names, derived
/// from that principal's admission.
pub(super) fn cross(transaction: &rusqlite::Transaction<'_>) -> Result<(), OpenError> {
    let births = births(transaction)?;
    let command = wire::CommandKind::GlobalSettingsSet;
    residual::rewrite_receipts(
        transaction,
        command,
        None,
        RETIRED_OPERATOR_RELEASE,
        |_, response| {
            let response =
                residual::edit_reply(command, response, |body| cross_answer(body, &births))?;
            Ok((None, response))
        },
    )
}

/// Cross one stored settings answer: a retired answer gains its principal
/// directory from the operator identity it carried; a current one gains the
/// codes its entries lack. Answers whether the answer changed.
fn cross_answer(
    body: &mut serde_json::Map<String, serde_json::Value>,
    births: &[(wire::CanonicalUuid, PrincipalCode)],
) -> Result<bool, residual::Refusal> {
    if body.contains_key("principals") {
        return code_entries(body, births)
            .map_err(|detail| residual::Refusal::under(CODELESS_DIRECTORY_RELEASE, detail));
    }
    let retired = body.remove("operator");
    let principals = match (births.first(), retired) {
        (Some((principal, code)), Some(identity)) => serde_json::json!([{
            "principal": principal,
            "code": code,
            "identity": identity,
        }]),
        _ => serde_json::json!([]),
    };
    body.insert("principals".to_owned(), principals);
    Ok(true)
}

/// Every born principal with its code, oldest birth first.
fn births(
    transaction: &rusqlite::Transaction<'_>,
) -> Result<Vec<(wire::CanonicalUuid, PrincipalCode)>, OpenError> {
    let mut statement = transaction
        .prepare(
            "SELECT seq, json_remove(json_extract(CAST(envelope AS TEXT), '$.fact'), '$.kind') \
             FROM fact_envelopes WHERE kind = 'principal::admitted' ORDER BY seq",
        )
        .map_err(OpenError::Sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(OpenError::Sqlite)?;
    rows.map(|row| {
        let (seq, fact) = row.map_err(OpenError::Sqlite)?;
        let refuse = |detail: String| -> OpenError {
            MigrationError::RetiredFact {
                seq,
                kind: "principal::admitted",
                release: CODELESS_DIRECTORY_RELEASE,
                detail,
            }
            .into()
        };
        let admitted: PrincipalAdmitted = serde_json::from_str(&fact)
            .map_err(|error| refuse(format!("the principal admission is invalid: {error}")))?;
        let born = u64::try_from(seq)
            .ok()
            .and_then(wire::Seq::new)
            .ok_or_else(|| refuse("the principal admission has no position".to_owned()))?;
        let head = PrincipalHead {
            principal: admitted.principal,
            born,
            profile: admitted.profile,
            roles: admitted.roles,
            capability: admitted.capability,
        };
        Ok((head.principal, PrincipalCode::of(&head)))
    })
    .collect()
}

/// Give each stored directory entry lacking one the code of the principal it
/// names, answering whether any entry gained one.
fn code_entries(
    body: &mut serde_json::Map<String, serde_json::Value>,
    births: &[(wire::CanonicalUuid, PrincipalCode)],
) -> Result<bool, String> {
    let entries = body
        .get_mut("principals")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| "the stored principal directory is not a JSON array".to_owned())?;
    let mut coded = false;
    for entry in entries {
        let entry = entry
            .as_object_mut()
            .ok_or_else(|| "a stored principal entry is not a JSON object".to_owned())?;
        if entry.contains_key("code") {
            continue;
        }
        let principal = entry
            .get("principal")
            .and_then(serde_json::Value::as_str)
            .and_then(|principal| principal.parse::<wire::CanonicalUuid>().ok())
            .ok_or_else(|| "a stored principal entry names no principal".to_owned())?;
        let code = births
            .iter()
            .find_map(|(born, code)| (*born == principal).then_some(*code))
            .ok_or_else(|| format!("principal {principal} has no admission"))?;
        entry.insert("code".to_owned(), serde_json::json!(code));
        coded = true;
    }
    Ok(coded)
}

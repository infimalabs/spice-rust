//! Lift each taste hold's evidence out of the audit notes its statement's
//! policy reads recorded and into the typed hold.
//!
//! A retired release spelled a hold as its plan alone and appended one
//! `taste-review audit` task note per policy read. The hold now carries the
//! matches and the count of revisions rechecked since it began, so the
//! crossing reads them from those notes once. The notes stay as history,
//! spelled as retained task notes since the orphan-fact crossing.

use std::collections::BTreeMap;

use rusqlite::{Transaction, params};
use serde_json::{Map, Value};
use wire::body::RetainedTaskNote;
use wire::{FactKind, StatementField, TasteFinding, TasteHold, TasteMatch, TastePlan};

use super::{OpenError, residual};
use crate::rooms::AUDIT_PREFIX;

/// Last release that spelled a hold as its plan and kept its evidence in
/// audit notes.
const TASTE_AUDIT_RELEASE: &str = "fe3487acd";

/// Select a JSON object whose taste is still the retired plan spelling.
fn plan_only(column: &str, path: &str) -> String {
    format!("json_type(CAST({column} AS TEXT), '{path}') = 'text'")
}

/// One retained audit note: the fact that caused it and its raw text.
struct Audit {
    cause: Option<i64>,
    note: String,
}

/// Every retained audit note, by task stamp in journal order.
pub(super) struct Audits {
    by_task: BTreeMap<String, Vec<Audit>>,
}

impl Audits {
    pub(super) fn read(transaction: &Transaction<'_>) -> Result<Self, OpenError> {
        let mut statement = transaction
            .prepare(
                "SELECT cause, \
                        json_extract(CAST(envelope AS TEXT), '$.fact.fact.task'), \
                        json_extract(CAST(envelope AS TEXT), '$.fact.fact.note') \
                 FROM fact_envelopes WHERE kind = ?1 \
                 AND json_extract(CAST(envelope AS TEXT), '$.fact.fact.kind') = ?2 \
                 AND json_extract(CAST(envelope AS TEXT), '$.fact.fact.note') GLOB ?3 \
                 ORDER BY seq",
            )
            .map_err(OpenError::Sqlite)?;
        let rows = statement
            .query_map(
                params![
                    FactKind::JournalRetained.as_str(),
                    RetainedTaskNote::KIND,
                    format!("{AUDIT_PREFIX}*")
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(1)?,
                        Audit {
                            cause: row.get(0)?,
                            note: row.get(2)?,
                        },
                    ))
                },
            )
            .map_err(OpenError::Sqlite)?;
        let mut by_task = BTreeMap::<String, Vec<Audit>>::new();
        for row in rows {
            let (task, audit) = row.map_err(OpenError::Sqlite)?;
            by_task.entry(task).or_default().push(audit);
        }
        Ok(Self { by_task })
    }

    fn of(&self, task: &str) -> &[Audit] {
        self.by_task.get(task).map_or(&[], Vec::as_slice)
    }

    /// The hold a creation fact opened: the one required audit it caused.
    fn opened(&self, task: &str, added: i64, plan: TastePlan) -> Result<TasteHold, String> {
        let caused = self
            .of(task)
            .iter()
            .filter(|audit| audit.cause == Some(added))
            .collect::<Vec<_>>();
        let [audit] = caused.as_slice() else {
            return Err(format!(
                "the held creation fact caused {} taste-review audits, not one",
                caused.len()
            ));
        };
        let Some(matches) = parse(&audit.note)? else {
            return Err("the audit the held creation caused matched nothing".to_owned());
        };
        Ok(TasteHold {
            plan: Some(plan),
            matches,
            rechecks: 0,
            opened: None,
        })
    }

    /// The hold a current statement stands under: its latest audit's matches,
    /// rechecked once for each required audit directly before it.
    fn standing(&self, task: &str, plan: TastePlan) -> Result<TasteHold, String> {
        let mut audits = self.of(task).iter().rev();
        let latest = audits
            .next()
            .ok_or("the held statement has no taste-review audit")?;
        let Some(matches) = parse(&latest.note)? else {
            return Err(
                "the held statement's latest taste-review audit matched nothing".to_owned(),
            );
        };
        let mut rechecks = 0_u32;
        for audit in audits {
            if parse(&audit.note)?.is_none() {
                break;
            }
            rechecks = rechecks
                .checked_add(1)
                .ok_or("the recheck count overflows")?;
        }
        Ok(TasteHold {
            plan: Some(plan),
            matches,
            rechecks,
            opened: None,
        })
    }
}

/// Replace a statement's plan-only taste with the hold `hold` reads for it.
fn lift(
    statement: &mut Map<String, Value>,
    hold: impl FnOnce(TastePlan) -> Result<TasteHold, String>,
) -> Result<(), String> {
    let Some(plan) = statement.get("taste").filter(|taste| taste.is_string()) else {
        return Ok(());
    };
    let plan = serde_json::from_value::<TastePlan>(plan.clone())
        .map_err(|error| format!("the stored taste plan does not decode: {error}"))?;
    let hold = serde_json::to_value(hold(plan)?)
        .map_err(|error| format!("the taste hold cannot encode: {error}"))?;
    statement.insert("taste".to_owned(), hold);
    Ok(())
}

/// Lift the plan-only taste of the creation fact at `seq`, if it holds one.
pub(super) fn lift_opened(
    seq: i64,
    fact: &mut Map<String, Value>,
    audits: &Audits,
) -> Result<(), String> {
    let task = fact
        .get("task")
        .and_then(Value::as_str)
        .ok_or("the creation fact names no task")?
        .to_owned();
    lift(fact, |plan| audits.opened(&task, seq, plan))
}

/// Lift every task head still spelling its hold as a plan.
pub(super) fn cross_heads(transaction: &Transaction<'_>, audits: &Audits) -> Result<(), OpenError> {
    let rows = {
        let mut statement = transaction
            .prepare(&format!(
                "SELECT stamp, head_seq, body FROM task_heads WHERE {} ORDER BY stamp",
                plan_only("body", "$.taste")
            ))
            .map_err(OpenError::Sqlite)?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })
            .map_err(OpenError::Sqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };
    for (stamp, head_seq, body) in rows {
        let refuse = |detail: String| {
            residual::retained("task_heads", stamp.clone(), TASTE_AUDIT_RELEASE, detail)
        };
        let mut body: Value = serde_json::from_slice(&body)
            .map_err(|error| refuse(format!("the stored statement does not decode: {error}")))?;
        let statement = body
            .as_object_mut()
            .ok_or_else(|| refuse("the stored statement is not an object".to_owned()))?;
        lift(statement, |plan| audits.standing(&stamp, plan)).map_err(refuse)?;
        let bytes = serde_json::to_vec(&body)
            .map_err(|error| refuse(format!("the crossed statement cannot encode: {error}")))?;
        let changed = transaction
            .execute(
                "UPDATE task_heads SET body = ?1 WHERE stamp = ?2 AND head_seq = ?3",
                params![bytes, stamp, head_seq],
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

/// Lift every creation fact still spelling its hold as a plan.
pub(super) fn cross_facts(transaction: &Transaction<'_>, audits: &Audits) -> Result<(), OpenError> {
    residual::rewrite_facts(
        transaction,
        FactKind::TaskAdded,
        &plan_only("envelope", "$.fact.taste"),
        TASTE_AUDIT_RELEASE,
        |seq, fact| lift_opened(seq, fact, audits),
    )
}

/// Read one audit note: `None` for a clear read, else the matches it found.
///
/// The retired writer spelled `taste-review audit policy=HEX outcome=OUTCOME
/// matches=[MATCH; ...]`, each match `field=Q taste=Q trigger=Q message=Q
/// line=N column=N` with every `Q` a Rust debug-quoted string.
fn parse(note: &str) -> Result<Option<Vec<TasteMatch>>, String> {
    let malformed = || format!("the taste-review audit {note:?} is malformed");
    let rest = note
        .strip_prefix(AUDIT_PREFIX)
        .and_then(|rest| rest.strip_prefix("policy="))
        .and_then(|rest| rest.split_once(' '))
        .and_then(|(_, rest)| rest.strip_prefix("outcome="))
        .and_then(|rest| rest.split_once(' '))
        .ok_or_else(malformed)?;
    let (outcome, rest) = rest;
    let mut rest = rest
        .strip_prefix("matches=[")
        .and_then(|rest| rest.strip_suffix(']'))
        .ok_or_else(malformed)?;
    let mut matches = Vec::new();
    while !rest.is_empty() {
        let (found, after) = parse_match(rest).ok_or_else(malformed)?;
        matches.push(found);
        rest = match after.strip_prefix("; ") {
            Some(next) => next,
            None if after.is_empty() => after,
            None => return Err(malformed()),
        };
    }
    match (outcome, matches.is_empty()) {
        ("clear", true) => Ok(None),
        ("required", false) => Ok(Some(matches)),
        _ => Err(malformed()),
    }
}

fn parse_match(text: &str) -> Option<(TasteMatch, &str)> {
    let (field, text) = quoted(text.strip_prefix("field=")?)?;
    let (taste, text) = quoted(text.strip_prefix(" taste=")?)?;
    let (trigger, text) = quoted(text.strip_prefix(" trigger=")?)?;
    let (message, text) = quoted(text.strip_prefix(" message=")?)?;
    let (line, text) = number(text.strip_prefix(" line=")?)?;
    let (column, text) = number(text.strip_prefix(" column=")?)?;
    let field = match field.as_str() {
        "title" => StatementField::Title,
        "description" => StatementField::Description,
        "acceptance" => StatementField::Acceptance,
        _ => return None,
    };
    Some((
        TasteMatch {
            field,
            line,
            taste,
            finding: Some(TasteFinding {
                trigger,
                column,
                message,
            }),
        },
        text,
    ))
}

/// Read one Rust debug-quoted string and return it with the text after it.
fn quoted(text: &str) -> Option<(String, &str)> {
    let mut chars = text.strip_prefix('"')?.char_indices();
    let body = &text[1..];
    let mut value = String::new();
    while let Some((at, next)) = chars.next() {
        match next {
            '"' => return Some((value, &body[at + 1..])),
            '\\' => value.push(match chars.next()?.1 {
                '\\' => '\\',
                '"' => '"',
                '\'' => '\'',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                '0' => '\0',
                'u' => {
                    let (open, _) = chars.next().filter(|(_, brace)| *brace == '{')?;
                    let close = body[open..].find('}')? + open;
                    let code = u32::from_str_radix(&body[open + 1..close], 16).ok()?;
                    while chars.next()?.0 < close {}
                    char::from_u32(code)?
                }
                _ => return None,
            }),
            other => value.push(other),
        }
    }
    None
}

/// Read one decimal count and return it with the text after it.
fn number(text: &str) -> Option<(usize, &str)> {
    let end = text
        .find(|digit: char| !digit.is_ascii_digit())
        .unwrap_or(text.len());
    Some((text[..end].parse().ok()?, &text[end..]))
}

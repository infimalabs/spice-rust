use super::*;

pub(super) fn open_instrument_findings(
    snapshot: &ValidatedSnapshot,
    instrument: gate::GateId,
) -> Result<Vec<(Digest, &Entry)>, Outcome<CommandAnswer>> {
    let mut findings = snapshot
        .entries
        .iter()
        .filter(|entry| gate_finding_task(entry) && gate_finding_task_open(entry))
        .map(|entry| {
            gate_finding_task_instrument(entry)
                .map(|observed| (observed, entry))
                .map_err(|detail| gate_finding_migration_refusal(entry, detail))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|(observed, _)| *observed == instrument)
        .map(|(_, entry)| entry)
        .map(|entry| gate_finding_task_identity(entry).map(|identity| (identity, entry)))
        .collect::<Result<Vec<_>, _>>()?;
    findings.sort_by(|left, right| left.0.cmp(&right.0));
    if let Some(pair) = findings.windows(2).find(|pair| pair[0].0 == pair[1].0) {
        return Err(refusal(
            "gate::finding-ambiguous",
            Some(pair[0].0.to_string()),
            "settle duplicate open gate finding tasks before redispatching",
        ));
    }
    Ok(findings)
}

fn gate_finding_task_instrument(entry: &Entry) -> Result<gate::GateId, String> {
    gate_finding_statement_instrument(entry.statement())
}

fn gate_finding_statement_instrument(statement: &TaskHead) -> Result<gate::GateId, String> {
    let tagged = statement
        .tags
        .iter()
        .filter_map(|tag| tag.strip_prefix(GATE_INSTRUMENT_TAG_PREFIX))
        .collect::<Vec<_>>();
    let written = match tagged.as_slice() {
        [instrument] => *instrument,
        [] => {
            let Some(description) = statement.description.as_deref() else {
                return Err("legacy task has no evidence description".to_owned());
            };
            let legacy = description
                .lines()
                .filter_map(|line| line.strip_prefix("Gate: "))
                .collect::<Vec<_>>();
            let [instrument] = legacy.as_slice() else {
                return Err(format!(
                    "legacy evidence carries {} canonical Gate lines; expected one",
                    legacy.len()
                ));
            };
            *instrument
        }
        _ => {
            return Err(format!(
                "task carries {} gate-instrument tags; expected one",
                tagged.len()
            ));
        }
    };
    gate::GateId::BATTERY
        .into_iter()
        .find(|candidate| candidate.as_str() == written)
        .ok_or_else(|| format!("task names unregistered gate instrument {written:?}"))
}

fn gate_finding_migration_refusal(entry: &Entry, detail: String) -> Outcome<CommandAnswer> {
    refusal(
        "gate::finding-migration-required",
        Some(format!("{}: {detail}", entry.handle())),
        "repair the legacy task through the current release's gate-finding migration before redispatching",
    )
}

pub(super) fn open_instrument_tasks(
    entries: &[Entry],
    instrument: gate::GateId,
) -> Result<std::collections::BTreeSet<wire::TaskStamp>, Outcome<CommandAnswer>> {
    Ok(entries
        .iter()
        .filter(|entry| gate_finding_task(entry) && gate_finding_task_open(entry))
        .map(|entry| {
            gate_finding_task_instrument(entry)
                .map(|observed| (observed, entry))
                .map_err(|detail| gate_finding_migration_refusal(entry, detail))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|(observed, _)| *observed == instrument)
        .map(|(_, entry)| entry.born().clone())
        .collect())
}

pub(super) fn current_open_instrument_tasks(
    heads: &[journal::TaskHeads],
    instrument: gate::GateId,
) -> Result<std::collections::BTreeSet<wire::TaskStamp>, GuardError> {
    Ok(heads
        .iter()
        .filter(|heads| {
            heads.statement().body().project == GATE_FINDINGS_PROJECT
                && heads
                    .statement()
                    .body()
                    .tags
                    .iter()
                    .any(|tag| tag == GATE_FINDING_TAG)
        })
        .filter(|heads| {
            heads.workflow().body().status == "pending"
                && heads.workflow().body().completion.is_none()
        })
        .map(|heads| {
            gate_finding_statement_instrument(heads.statement().body())
                .map(|observed| (observed, heads))
                .map_err(|detail| {
                    GuardError::refused(format!(
                        "gate::finding-migration-required:{}:{detail}",
                        heads.statement().body().born
                    ))
                })
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|(observed, _)| *observed == instrument)
        .map(|(_, heads)| heads.statement().body().born.clone())
        .collect())
}

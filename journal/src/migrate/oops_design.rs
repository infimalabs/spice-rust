use rusqlite::{OptionalExtension, params};

use super::{OpenError, residual};

const OOPS_DESIGN_RELEASE: &str = "schema-74";
/// Acceptance this crossing stamped on every retained report it made a Plan.
const CROSSED_REPORT_ACCEPTANCE: [&str; 2] = [
    "Explicitly disposition the reported friction as duplicate, no-work, deferred, covered, or change-warranted.",
    "When change is warranted, construct acceptance-bearing child work that addresses it.",
];

struct Report {
    seq: i64,
    stamp: wire::TaskStamp,
    project: String,
}

/// Cross the retired OOPS filing queue into one current opportunistic Design.
///
/// The reports retain their task identity and authored statement. Their filing
/// marker becomes an ordinary filed Plan statement controlled by the synthetic
/// Design generation. The synthetic Design fact is appended because retained
/// sequence identities never move during a crossing.
pub(super) fn cross(transaction: &rusqlite::Transaction<'_>) -> Result<(), OpenError> {
    let reports = retained_reports(transaction)?;
    cross_grooming_evidence(transaction)?;
    if reports.is_empty() {
        return Ok(());
    }

    let design = design_stamp(transaction, &reports)?;
    let preparation = wire::TaskPreparation {
        parent: design.clone(),
        controller: design.clone(),
        construction: reports[0].stamp.clone(),
    };
    let acceptance = CROSSED_REPORT_ACCEPTANCE.map(str::to_owned).to_vec();
    let acceptance = serde_json::to_string(&acceptance)
        .map_err(|error| retained("task_heads", design.to_string(), error.to_string()))?;

    for report in &reports {
        let mut association = preparation.clone();
        association.construction = report.stamp.clone();
        let association = serde_json::to_string(&association)
            .map_err(|error| retained("task_heads", report.stamp.to_string(), error.to_string()))?;
        transaction
            .execute(
                "UPDATE task_heads
                 SET body = CAST(json_remove(json_set(
                     CAST(body AS TEXT),
                     '$.project', ?1,
                     '$.intent', 'plan',
                     '$.preparation', json(?2),
                     '$.acceptance', json(?3)
                 ), '$.filing') AS BLOB)
                 WHERE stamp = ?4",
                params![
                    report.project,
                    association,
                    acceptance,
                    report.stamp.to_string()
                ],
            )
            .map_err(OpenError::Sqlite)?;
        transaction
            .execute(
                "UPDATE fact_envelopes
                 SET envelope = CAST(json_remove(json_set(
                     CAST(envelope AS TEXT),
                     '$.fact.project', ?1,
                     '$.fact.intent', 'plan',
                     '$.fact.preparation', json(?2),
                     '$.fact.acceptance', json(?3)
                 ), '$.fact.filing') AS BLOB)
                 WHERE seq = ?4",
                params![report.project, association, acceptance, report.seq],
            )
            .map_err(OpenError::Sqlite)?;
        canonicalize_report_rows(transaction, report)?;
    }

    append_design(transaction, &design, &reports[0])
}

fn canonicalize_report_rows(
    transaction: &rusqlite::Transaction<'_>,
    report: &Report,
) -> Result<(), OpenError> {
    let statement = transaction
        .query_row(
            "SELECT body FROM task_heads WHERE stamp = ?1",
            [report.stamp.to_string()],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .map_err(OpenError::Sqlite)?;
    let statement = wire::decode::<wire::TaskHead>(&statement).map_err(|error| {
        retained(
            "task_heads",
            report.stamp.to_string(),
            format!("crossed statement does not decode: {error}"),
        )
    })?;
    let statement = wire::encode(&statement)
        .map_err(|error| retained("task_heads", report.stamp.to_string(), error.to_string()))?;
    transaction
        .execute(
            "UPDATE task_heads SET body = ?1 WHERE stamp = ?2",
            params![statement, report.stamp.to_string()],
        )
        .map_err(OpenError::Sqlite)?;

    let envelope = transaction
        .query_row(
            "SELECT envelope FROM fact_envelopes WHERE seq = ?1",
            [report.seq],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .map_err(OpenError::Sqlite)?;
    let envelope = wire::decode::<wire::Envelope<wire::AnyFact>>(&envelope).map_err(|error| {
        retained(
            "fact_envelopes",
            report.seq.to_string(),
            format!("crossed task birth does not decode: {error}"),
        )
    })?;
    let envelope = wire::encode(&envelope)
        .map_err(|error| retained("fact_envelopes", report.seq.to_string(), error.to_string()))?;
    transaction
        .execute(
            "UPDATE fact_envelopes SET envelope = ?1 WHERE seq = ?2",
            params![envelope, report.seq],
        )
        .map_err(OpenError::Sqlite)?;
    Ok(())
}

fn retained_reports(transaction: &rusqlite::Transaction<'_>) -> Result<Vec<Report>, OpenError> {
    let rows = {
        let mut query = transaction
            .prepare(
                "SELECT fact.seq,
                        json_extract(CAST(fact.envelope AS TEXT), '$.fact.task'),
                        json_extract(CAST(fact.envelope AS TEXT), '$.fact.project')
                 FROM fact_envelopes AS fact
                 WHERE fact.kind = 'task::added'
                   AND (
                     json_extract(CAST(fact.envelope AS TEXT), '$.fact.filing') = 'oops'
                     OR json_extract(CAST(fact.envelope AS TEXT), '$.fact.project') = '.oops'
                     OR json_extract(CAST(fact.envelope AS TEXT), '$.fact.project') LIKE '.oops.%'
                   )
                 ORDER BY fact.seq",
            )
            .map_err(OpenError::Sqlite)?;
        query
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(OpenError::Sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(OpenError::Sqlite)?
    };

    let mut reports = Vec::with_capacity(rows.len());
    for (seq, stamp, project) in rows {
        let stamp = stamp.parse::<wire::TaskStamp>().map_err(|error| {
            retained(
                "fact_envelopes",
                seq.to_string(),
                format!("invalid task stamp: {error}"),
            )
        })?;
        let head_exists = transaction
            .query_row(
                "SELECT 1 FROM task_heads WHERE stamp = ?1",
                [stamp.to_string()],
                |_| Ok(()),
            )
            .optional()
            .map_err(OpenError::Sqlite)?
            .is_some();
        if !head_exists {
            return Err(retained(
                "fact_envelopes",
                seq.to_string(),
                format!("OOPS report {stamp} has no retained statement head"),
            ));
        }
        let project = if project == ".oops" {
            "task.oops".to_owned()
        } else if let Some(public) = project.strip_prefix(".oops.") {
            public.to_owned()
        } else {
            project
        };
        reports.push(Report {
            seq,
            stamp,
            project,
        });
    }
    Ok(reports)
}

fn design_stamp(
    transaction: &rusqlite::Transaction<'_>,
    reports: &[Report],
) -> Result<wire::TaskStamp, OpenError> {
    let first = reports
        .iter()
        .map(|report| report.stamp.micros())
        .min()
        .expect("a nonempty retained report set has a first report");
    for micros in (0..first).rev().take(1_024) {
        let stamp = wire::TaskStamp::for_micros(micros)
            .map_err(|error| retained("task_heads", "oops-design".to_owned(), error.to_string()))?;
        let occupied = transaction
            .query_row(
                "SELECT 1 FROM task_heads WHERE stamp = ?1",
                [stamp.to_string()],
                |_| Ok(()),
            )
            .optional()
            .map_err(OpenError::Sqlite)?
            .is_some();
        if !occupied {
            return Ok(stamp);
        }
    }
    Err(retained(
        "task_heads",
        reports[0].stamp.to_string(),
        "no unoccupied predecessor stamp can own the retained OOPS generation".to_owned(),
    ))
}

fn design_authority(
    transaction: &rusqlite::Transaction<'_>,
    first: &Report,
) -> Result<(wire::Instant, wire::Origin, wire::TaskProvenance), OpenError> {
    let (at, origin) = transaction
        .query_row(
            "SELECT instant, origin FROM fact_envelopes WHERE seq = ?1",
            [first.seq],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(OpenError::Sqlite)?;
    let at = at.parse::<wire::Instant>().map_err(|error| {
        retained(
            "fact_envelopes",
            first.seq.to_string(),
            format!("invalid instant: {error}"),
        )
    })?;
    let origin = origin.parse::<wire::Origin>().map_err(|error| {
        retained(
            "fact_envelopes",
            first.seq.to_string(),
            format!("invalid origin: {error}"),
        )
    })?;
    let provenance = origin
        .as_str()
        .parse::<wire::TaskProvenance>()
        .map_err(|error| {
            retained(
                "fact_envelopes",
                first.seq.to_string(),
                format!("origin cannot own the migrated Design: {error}"),
            )
        })?;
    Ok((at, origin, provenance))
}

fn append_design(
    transaction: &rusqlite::Transaction<'_>,
    design: &wire::TaskStamp,
    first: &Report,
) -> Result<(), OpenError> {
    let (at, origin, provenance) = design_authority(transaction, first)?;
    let next = transaction
        .query_row(
            "SELECT coalesce(max(seq), 0) + 1 FROM fact_envelopes",
            [],
            |row| row.get::<_, i64>(0),
        )
        .map_err(OpenError::Sqlite)?;
    let sequence = wire::Seq::new(
        u64::try_from(next)
            .map_err(|error| retained("fact_envelopes", next.to_string(), error.to_string()))?,
    )
    .ok_or_else(|| {
        retained(
            "fact_envelopes",
            next.to_string(),
            "zero sequence".to_owned(),
        )
    })?;
    let phase = "design"
        .parse::<wire::Phase>()
        .expect("design is a valid phase");
    let creation = wire::body::TaskCreationContext::unavailable(
        wire::body::TaskCreationUnavailable::SpiceActor,
    );
    let added = wire::body::TaskAdded {
        task: design.clone(),
        title: wire::OOPS_DESIGN_TITLE.to_owned(),
        pitch: wire::OOPS_DESIGN_PITCH.to_owned(),
        description: Some(wire::OOPS_DESIGN_DESCRIPTION.to_owned()),
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        project: wire::OOPS_DESIGN_PROJECT.to_owned(),
        filing: wire::TaskFiling::Filed,
        priority: None,
        taste: None,
        document_slug: None,
        depends: Vec::new(),
        intent: Some(wire::TaskIntent::OpportunisticDesign),
        preparation: None,
        flow: vec![phase.clone()],
        phase: phase.clone(),
        revision: 1,
        creation: creation.clone(),
    };
    let fact: wire::AnyFact = wire::decode(
        &wire::encode(&wire::Fact::new(added))
            .map_err(|error| retained("fact_envelopes", next.to_string(), error.to_string()))?,
    )
    .map_err(|error| retained("fact_envelopes", next.to_string(), error.to_string()))?;
    let envelope = wire::Envelope {
        v: wire::WireVersion::CURRENT,
        seq: sequence,
        at,
        actor: wire::Actor::Spice,
        origin: origin.clone(),
        cause: None,
        fact,
    };
    let envelope_bytes = wire::encode(&envelope)
        .map_err(|error| retained("fact_envelopes", next.to_string(), error.to_string()))?;
    transaction
        .execute(
            "INSERT INTO fact_envelopes(
                seq, wire_version, instant, actor, origin, cause, kind, envelope
             ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, 'task::added', ?6)",
            params![
                next,
                envelope.v.0,
                envelope.at.to_string(),
                envelope.actor.to_string(),
                origin.as_str(),
                envelope_bytes,
            ],
        )
        .map_err(OpenError::Sqlite)?;
    transaction
        .execute(
            "INSERT INTO thread_index(seq, origin, cause, root, depth)
             VALUES (?1, ?2, NULL, ?1, 0)",
            params![next, origin.as_str()],
        )
        .map_err(OpenError::Sqlite)?;

    append_design_heads(
        transaction,
        design,
        next,
        sequence,
        provenance,
        creation,
        phase,
    )
}

fn append_design_heads(
    transaction: &rusqlite::Transaction<'_>,
    design: &wire::TaskStamp,
    next: i64,
    sequence: wire::Seq,
    provenance: wire::TaskProvenance,
    creation: wire::body::TaskCreationContext,
    phase: wire::Phase,
) -> Result<(), OpenError> {
    let statement = wire::TaskHead {
        born: design.clone(),
        queue_origin: sequence,
        title: wire::OOPS_DESIGN_TITLE.to_owned(),
        pitch: wire::OOPS_DESIGN_PITCH.to_owned(),
        description: Some(wire::OOPS_DESIGN_DESCRIPTION.to_owned()),
        acceptance: Vec::new(),
        requirements: wire::EnvironmentRequirements::default(),
        project: wire::OOPS_DESIGN_PROJECT.to_owned(),
        filing: wire::TaskFiling::Filed,
        priority: None,
        taste: None,
        document_slug: None,
        dependencies: Vec::new(),
        intent: Some(wire::TaskIntent::OpportunisticDesign),
        preparation: None,
        provenance,
        creation,
        revision: sequence,
    };
    let workflow = wire::WorkflowHead {
        flow: vec![phase.clone()],
        phase,
        status: wire::WorkflowStatus::pending(),
        deferral: None,
        generation: 0,
    };
    let statement = wire::encode(&statement)
        .map_err(|error| retained("task_heads", design.to_string(), error.to_string()))?;
    let workflow = wire::encode(&workflow)
        .map_err(|error| retained("workflow_heads", design.to_string(), error.to_string()))?;
    transaction
        .execute(
            "INSERT INTO task_heads(stamp, head_seq, body) VALUES (?1, ?2, ?3)",
            params![design.to_string(), next, statement],
        )
        .map_err(OpenError::Sqlite)?;
    transaction
        .execute(
            "INSERT INTO workflow_heads(stamp, head_seq, body, revised_seq)
             VALUES (?1, ?2, ?3, NULL)",
            params![design.to_string(), next, workflow],
        )
        .map_err(OpenError::Sqlite)?;
    transaction
        .execute(
            "UPDATE cursor_head SET seq = ?1 WHERE singleton = 1",
            [next],
        )
        .map_err(OpenError::Sqlite)?;
    transaction
        .execute(
            "UPDATE steering_index SET indexed_seq = ?1 WHERE singleton = 1",
            [next],
        )
        .map_err(OpenError::Sqlite)?;
    Ok(())
}

fn cross_grooming_evidence(transaction: &rusqlite::Transaction<'_>) -> Result<(), OpenError> {
    transaction
        .execute_batch(
            "UPDATE fact_envelopes
             SET envelope = CAST(json_set(
                 CAST(envelope AS TEXT),
                 '$.fact.content',
                 json_object(
                     'source', 'authored',
                     'text', 'Retained OOPS grooming: ' ||
                         json(json_extract(CAST(envelope AS TEXT), '$.fact.content.grooming'))
                 )
             ) AS BLOB)
             WHERE kind = 'room::published'
               AND json_extract(CAST(envelope AS TEXT), '$.fact.content.source') = 'oops-groomed';

             UPDATE command_receipts
             SET request = CAST(json_remove(json_set(
                 CAST(request AS TEXT),
                 '$.body.text', 'Retained OOPS grooming: ' ||
                     json(json_extract(CAST(request AS TEXT), '$.body.grooming'))
             ), '$.body.grooming') AS BLOB)
             WHERE json_type(CAST(request AS TEXT), '$.body.grooming') IS NOT NULL;",
        )
        .map_err(OpenError::Sqlite)
}

fn retained(table: &'static str, row: String, detail: String) -> OpenError {
    residual::retained(table, row, OOPS_DESIGN_RELEASE, detail)
}

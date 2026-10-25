//! Bounded private acquisition over the existing authenticated board door.

use super::*;
use wire::intent::{REPORT_FACT_KINDS, REPORT_READ_VERSION, ReportAnswer, ReportPin, ReportRead};

pub(super) fn read_report(
    bus: &Bus,
    request: &ReportRead,
    command: &wire::AdmittedCommand,
) -> Outcome<CommandAnswer> {
    if request.version != REPORT_READ_VERSION {
        return refusal(
            "report::version-mismatch",
            Some(request.version.to_string()),
            "use the current report-read contract",
        );
    }
    if request.kinds.is_empty()
        || request.kinds.len() > REPORT_FACT_KINDS.len()
        || request
            .kinds
            .iter()
            .any(|kind| !REPORT_FACT_KINDS.contains(kind))
    {
        return refusal(
            "report::kind-outside-scope",
            None,
            "request only the explicit report fact vocabulary",
        );
    }
    if request.after.is_some() && request.pin.is_none() {
        return refusal(
            "report::pin-required",
            None,
            "a continuation must retain the first answer's pin",
        );
    }
    let heads = match bus.app_heads() {
        Ok(heads) => heads,
        Err(error) => return store_fault("report::authority-unreadable", &error.to_string()),
    };
    let Some(residency) = heads.residency() else {
        return refusal(
            "report::authority-unavailable",
            None,
            "the current native residency must be admitted",
        );
    };
    if let Err(outcome) = actor_route(command.actor(), residency.body()) {
        return outcome;
    }
    let decoder = match spice::version::running() {
        Ok(running) => running.binary.clone(),
        Err(fault) => return Outcome::Fault(fault),
    };
    let pin = ReportPin {
        schema: spice::journal_schema::CURRENT,
        decoder,
        epoch: command.epoch(),
    };
    if request
        .pin
        .as_ref()
        .is_some_and(|expected| *expected != pin)
    {
        return refusal(
            "report::generation-changed",
            None,
            "restart acquisition after decoder, schema or residency replacement",
        );
    }
    let request = request.clone();
    match bus.read(move |read| page(read, request, pin)) {
        Ok(Ok(answer)) => Outcome::Reply(Reply {
            position: ReplyPosition::Snapshot(answer.upper),
            body: CommandAnswer::Report(Box::new(answer)),
        }),
        Ok(Err(refused)) => Outcome::Refusal(refused),
        Err(error) => store_fault("report::journal-unreadable", &error.to_string()),
    }
}

fn page(
    read: &JournalRead<'_>,
    request: ReportRead,
    pin: ReportPin,
) -> Result<Result<ReportAnswer, wire::Refusal>, GuardError> {
    if read.cursor()?.is_none_or(|cursor| request.upper > cursor) {
        return Ok(Err(boundary_refusal(
            "report::boundary-unavailable",
            "the study upper bound is beyond this guard's cursor",
        )));
    }
    if request.after.is_some_and(|after| after > request.upper)
        || request
            .command_at
            .is_some_and(|position| position > request.upper)
    {
        return Ok(Err(boundary_refusal(
            "report::position-outside-boundary",
            "continuation and command evidence must not exceed the study barrier",
        )));
    }
    let mut page = read.selected_fact_page(
        &request.kinds,
        request.after,
        Some(request.upper),
        journal::FactReadBudget::RETAINED,
    )?;
    let mut scope_error = None;
    page.rows.retain(|envelope| {
        let Some(task) = envelope.fact.task_subject() else {
            return true;
        };
        match filed_task(read, task) {
            Ok(filed) => filed,
            Err(error) => {
                scope_error = Some(error);
                false
            }
        }
    });
    if let Some(error) = scope_error {
        return Err(error);
    }
    let horizon = read.fact_page(
        journal::FactPageStart::At(request.upper),
        request.upper,
        journal::FactReadBudget::new(1, journal::MAX_RETAINED_FACT_PAGE_BYTES)
            .expect("one-row native report horizon budget"),
    )?;
    let horizon_at = horizon
        .rows
        .into_iter()
        .next()
        .ok_or_else(|| GuardError::Invalid("report barrier fact is missing".to_owned()))?
        .at;
    let command_record = request
        .command_at
        .map(|position| read.report_review_flows(position))
        .transpose()?
        .flatten();
    if let Some(record) = &command_record {
        let born = record
            .task
            .born()
            .map_err(|error| GuardError::Invalid(format!("report command target: {error}")))?;
        if record.last > request.upper || !filed_task(read, &born)? {
            return Ok(Err(boundary_refusal(
                "report::command-outside-scope",
                "the command record must be wholly within the filed study boundary",
            )));
        }
    }
    Ok(Ok(ReportAnswer {
        version: REPORT_READ_VERSION,
        pin,
        upper: request.upper,
        horizon_at,
        rows: page.rows.into_iter().map(wire::StreamFact::from).collect(),
        continuation: page.continuation,
        command_record,
    }))
}

fn filed_task(read: &JournalRead<'_>, task: &wire::TaskStamp) -> Result<bool, GuardError> {
    let head = read.task_head(task)?.ok_or_else(|| {
        GuardError::Invalid(format!(
            "report task scope has no authoritative head for {task}"
        ))
    })?;
    Ok(head.body().filing.is_filed())
}

fn boundary_refusal(reason: &str, detail: &str) -> wire::Refusal {
    wire::Refusal::bare(
        wire::RefusalDiagnostic::new(reason.to_owned(), None, None, None)
            .with_detail(detail.to_owned()),
    )
}

//! Packaged, cached, and configured worker dispatch over one staged source.

use super::*;

pub(super) fn dispatch_resolution(
    progress: &mut File,
    started: Instant,
) -> Result<ResolutionReport, WorkerFailure> {
    write_phase(progress, &WorkerPhase::Policy).map_err(worker_io_failure)?;
    let mut dispatch = configured::Dispatch::capture().map_err(configured_failure)?;
    let source = dispatch.source().clone();
    let packaged = if dispatch.builtins_enabled() {
        write_phase(progress, &WorkerPhase::TestSelection).map_err(worker_io_failure)?;
        let report = spice::change_gate::run_exhaustive(progress).map_err(change_gate_failure)?;
        if report.source() != &source {
            return Err(WorkerFailure {
                reason: "gate::resolution-staged-source-moved".to_owned(),
                detail: format!(
                    "dispatcher source {source} differs from exhaustive source {}",
                    report.source(),
                ),
                fault: false,
            });
        }
        dispatch = dispatch.rebind(&source).map_err(configured_failure)?;
        Some(
            ResolutionPackagedReport::new(&report).map_err(|detail| WorkerFailure {
                reason: "gate::resolution-report-invalid".to_owned(),
                detail,
                fault: true,
            })?,
        )
    } else {
        None
    };
    let configured = if dispatch.has_configured() {
        write_phase(
            progress,
            &WorkerPhase::Configured {
                steps: dispatch.configured_count(),
            },
        )
        .map_err(worker_io_failure)?;
        let remaining = WORK_RUNTIME
            .saturating_sub(started.elapsed())
            .saturating_sub(configured::cleanup_reserve());
        dispatch
            .execute_resolution_configured(remaining, Cancellation::Never)
            .map_err(configured_failure)?
    } else {
        None
    };
    dispatch.verify().map_err(configured_failure)?;
    ResolutionReport::new(source, packaged, configured).map_err(|detail| WorkerFailure {
        reason: "gate::resolution-report-invalid".to_owned(),
        detail,
        fault: true,
    })
}

pub(super) fn dispatch(progress: &mut File, started: Instant) -> Result<Report, WorkerFailure> {
    write_phase(progress, &WorkerPhase::Policy).map_err(worker_io_failure)?;
    let dispatch = configured::Dispatch::capture().map_err(configured_failure)?;
    let source = dispatch.source().clone();
    let (packaged, full_verifier, dispatch) = packaged(progress, dispatch, &source)?;

    let configured = if dispatch.has_configured() {
        write_phase(
            progress,
            &WorkerPhase::Configured {
                steps: dispatch.configured_count(),
            },
        )
        .map_err(worker_io_failure)?;
        let remaining = WORK_RUNTIME
            .saturating_sub(started.elapsed())
            .saturating_sub(configured::cleanup_reserve());
        dispatch
            .execute_configured(remaining, Cancellation::Never)
            .map_err(configured_failure)?
    } else {
        None
    };
    dispatch.verify().map_err(configured_failure)?;
    Report::new(source, packaged, full_verifier, configured).map_err(|detail| WorkerFailure {
        reason: "gate::pre-commit-report-invalid".to_owned(),
        detail,
        fault: true,
    })
}

type Builtins = (
    Option<spice::change_gate::ChangeGateReport>,
    Option<FullVerifierReport>,
    configured::Dispatch,
);

fn packaged(
    progress: &mut File,
    mut dispatch: configured::Dispatch,
    source: &Digest,
) -> Result<Builtins, WorkerFailure> {
    write_phase(progress, &WorkerPhase::TestSelection).map_err(worker_io_failure)?;
    let admitted =
        spice::gate::admit_full_verifier_evidence(dispatch.repository(), gate::Selector::staged())
            .map_err(|error| WorkerFailure {
                reason: error.reason().to_owned(),
                detail: error.detail().to_owned(),
                fault: error.is_fault(),
            })?;
    if let Some(admitted) = admitted {
        if admitted.source() != source {
            return Err(WorkerFailure {
                reason: "gate::pre-commit-staged-source-moved".to_owned(),
                detail: format!(
                    "dispatcher source {source} differs from full-verifier source {}",
                    admitted.source(),
                ),
                fault: false,
            });
        }
        let cached = FullVerifierReport::new(&admitted, dispatch.changes()).map_err(|detail| {
            WorkerFailure {
                reason: "gate::change-selection-refused".to_owned(),
                detail,
                fault: false,
            }
        })?;
        dispatch = dispatch.rebind(source).map_err(configured_failure)?;
        let configured_steps = dispatch.omit_full_verifier_proven_configured();
        writeln!(
            progress,
            "[change-gate-full-verifier] evidence={} policy_arms={} configured_steps={configured_steps}",
            cached.evidence,
            admitted.battery().results().len(),
        )
        .map_err(worker_io_failure)?;
        progress.flush().map_err(worker_io_failure)?;
        return Ok((None, Some(cached), dispatch));
    }
    if !dispatch.builtins_enabled() {
        return Ok((None, None, dispatch));
    }
    let report = spice::change_gate::run(progress).map_err(change_gate_failure)?;
    if report.selection().source() != source {
        return Err(WorkerFailure {
            reason: "gate::pre-commit-staged-source-moved".to_owned(),
            detail: format!(
                "dispatcher source {source} differs from packaged source {}",
                report.selection().source(),
            ),
            fault: false,
        });
    }
    dispatch = dispatch.rebind(source).map_err(configured_failure)?;
    Ok((Some(report), None, dispatch))
}

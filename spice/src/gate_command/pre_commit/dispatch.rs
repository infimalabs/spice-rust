//! Packaged and configured worker dispatch over one staged source.

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
        let report = spice::change_gate::run_resolution(progress).map_err(change_gate_failure)?;
        if report.selection().source() != &source {
            return Err(WorkerFailure {
                reason: "gate::resolution-staged-source-moved".to_owned(),
                detail: format!(
                    "dispatcher source {source} differs from selected resolution source {}",
                    report.selection().source(),
                ),
                fault: false,
            });
        }
        let packaged =
            ResolutionPackagedReport::selected(&report).map_err(|detail| WorkerFailure {
                reason: "gate::resolution-report-invalid".to_owned(),
                detail,
                fault: true,
            })?;
        dispatch = dispatch.rebind(&source).map_err(configured_failure)?;
        Some(packaged)
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
            .execute_resolution_configured(remaining, Cancellation::Never, progress)
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
    let (packaged, mut dispatch) = packaged(progress, dispatch, &source)?;

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
            .execute_configured(remaining, Cancellation::Never, progress)
            .map_err(configured_failure)?
    } else {
        None
    };
    dispatch.verify().map_err(configured_failure)?;
    Report::new(source, packaged, configured).map_err(|detail| WorkerFailure {
        reason: "gate::pre-commit-report-invalid".to_owned(),
        detail,
        fault: true,
    })
}

type Builtins = (
    Option<spice::change_gate::ChangeGateReport>,
    configured::Dispatch,
);

fn packaged(
    progress: &mut File,
    mut dispatch: configured::Dispatch,
    source: &Digest,
) -> Result<Builtins, WorkerFailure> {
    if !dispatch.builtins_enabled() {
        return Ok((None, dispatch));
    }
    write_phase(progress, &WorkerPhase::TestSelection).map_err(worker_io_failure)?;
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
    Ok((Some(report), dispatch))
}

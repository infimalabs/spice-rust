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
        let packaged = match spice::change_gate::run_resolution(progress) {
            Ok(report) => {
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
                ResolutionPackagedReport::selected(&report)
            }
            Err(error) if error.reason() == "gate::change-selection-refused" => {
                writeln!(
                    progress,
                    "[change-gate-resolution] disposition=fresh-exhaustive detail={:?}",
                    error.detail(),
                )
                .map_err(worker_io_failure)?;
                progress.flush().map_err(worker_io_failure)?;
                let report = spice::change_gate::run_exhaustive(progress)
                    .map_err(change_gate_failure)?;
                if report.source() != &source {
                    return Err(WorkerFailure {
                        reason: "gate::resolution-staged-source-moved".to_owned(),
                        detail: format!(
                            "dispatcher source {source} differs from exhaustive resolution source {}",
                            report.source(),
                        ),
                        fault: false,
                    });
                }
                ResolutionPackagedReport::exhaustive(&report)
            }
            Err(error) => return Err(change_gate_failure(error)),
        }
        .map_err(|detail| WorkerFailure {
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
    let initial = configured::Dispatch::capture().map_err(configured_failure)?;
    let expected_source = initial.source().clone();
    let expected_authority = initial.into_authority();
    write_phase(progress, &WorkerPhase::TestSelection).map_err(worker_io_failure)?;
    let (admitted, miss) = match full_verifier_evidence(&expected_source)? {
        spice::gate::FullVerifierEvidenceAdmission::Admitted(admitted) => (Some(*admitted), None),
        spice::gate::FullVerifierEvidenceAdmission::Unavailable(miss) => {
            writeln!(
                progress,
                "[change-gate-full-verifier] disposition=miss stage={} detail={:?}",
                miss.stage().as_str(),
                miss.detail(),
            )
            .map_err(worker_io_failure)?;
            progress.flush().map_err(worker_io_failure)?;
            (None, Some(miss))
        }
    };
    let dispatch = configured::Dispatch::capture().map_err(configured_failure)?;
    dispatch
        .verify_authority(&expected_authority, "full-verifier-evidence-lookup")
        .map_err(configured_failure)?;
    let source = dispatch.source().clone();
    let (packaged, full_verifier, mut dispatch) =
        packaged(progress, dispatch, &source, admitted)
            .map_err(|failure| with_full_verifier_miss(failure, miss.as_ref()))?;

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
            .map_err(configured_failure)
            .map_err(|failure| with_full_verifier_miss(failure, miss.as_ref()))?
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
    admitted: Option<spice::gate::AdmittedFullVerifierEvidence>,
) -> Result<Builtins, WorkerFailure> {
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

fn full_verifier_evidence(
    current_source: &Digest,
) -> Result<spice::gate::FullVerifierEvidenceAdmission, WorkerFailure> {
    let current = env::current_dir().map_err(|error| WorkerFailure {
        reason: "gate::pre-commit-working-directory-unavailable".to_owned(),
        detail: error.to_string(),
        fault: true,
    })?;
    let repository = Repository::discover(&current).map_err(|error| WorkerFailure {
        reason: "gate::pre-commit-repository-unresolved".to_owned(),
        detail: error.to_string(),
        fault: true,
    })?;
    spice::gate::inspect_full_verifier_evidence_for_source(
        &repository,
        gate::Selector::staged(),
        current_source,
    )
    .map_err(|error| WorkerFailure {
        reason: error.reason().to_owned(),
        detail: error.detail().to_owned(),
        fault: error.is_fault(),
    })
}

fn with_full_verifier_miss(
    mut failure: WorkerFailure,
    miss: Option<&spice::gate::FullVerifierEvidenceMiss>,
) -> WorkerFailure {
    let Some(miss) = miss else {
        return failure;
    };
    failure.detail = format!(
        "full-verifier admission missed at stage={}: {}; {}",
        miss.stage().as_str(),
        miss.detail(),
        failure.detail,
    );
    failure
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_failure_retains_the_exact_full_verifier_miss_stage() {
        let miss = spice::gate::FullVerifierEvidenceMiss::new(
            spice::gate::FullVerifierEvidenceStage::Compiler,
            "compiler receipt authority differs",
        );
        let failure = with_full_verifier_miss(
            WorkerFailure {
                reason: "gate::configured-refused".to_owned(),
                detail: "configured source gate refused".to_owned(),
                fault: false,
            },
            Some(&miss),
        );

        assert_eq!(failure.reason, "gate::configured-refused");
        assert_eq!(
            failure.detail,
            concat!(
                "full-verifier admission missed at stage=compiler: ",
                "compiler receipt authority differs; configured source gate refused",
            ),
        );
        assert!(!failure.fault);
    }
}

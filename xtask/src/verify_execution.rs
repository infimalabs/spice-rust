//! Serial execution of the full and package-scoped verifier routes.

use std::io::{self, Write};

use super::verify_timing::{self, Conclusion, Disposition, Stage};
use super::{
    Action, BatteryRun, ENTRY_POINT_LABEL, FORMAT_LABEL, Runner, SCOPED_COMMANDS, Status, Step,
    VERIFY_ACTIONS, VerifyError, valid_package_name,
};

pub(super) fn verify_with<R: Runner + Send>(
    runner: &mut R,
    output: &mut impl Write,
) -> Result<(), VerifyError> {
    for (index, action) in VERIFY_ACTIONS.into_iter().enumerate() {
        run_action(
            runner,
            output,
            action,
            Stage {
                ordinal: index + 1,
                total: VERIFY_ACTIONS.len(),
                identity: action.label(),
                family: action.family(),
            },
        )?;
    }
    Ok(())
}

fn run_action<R: Runner + Send>(
    runner: &mut R,
    output: &mut impl Write,
    action: Action,
    stage: Stage<'_>,
) -> Result<(), VerifyError> {
    match action {
        Action::EntryPoint => run_entry_point(runner, output, stage),
        Action::Format => run_format(runner, output, stage),
        Action::Command(step) => run_command(runner, output, stage, step),
        Action::Browser => run_browser(runner, output, stage),
        Action::WorkspaceTests => run_workspace_tests(runner, output, stage),
        Action::Battery => run_battery(runner, output, stage),
        Action::Landing => run_landing(runner, output, stage),
    }
}

fn run_entry_point<R: Runner + Send>(
    runner: &mut R,
    output: &mut impl Write,
    stage: Stage<'_>,
) -> Result<(), VerifyError> {
    verify_timing::observe(
        output,
        stage,
        || runner.entry_point(),
        |result, output| match result {
            Ok(report) => {
                output
                    .write_all(report.as_bytes())
                    .map_err(VerifyError::Output)?;
                Ok(Conclusion::success((), Disposition::Success))
            }
            Err(detail) => Ok(Conclusion::failure(
                VerifyError::EntryPoint(detail),
                Disposition::Error,
            )),
        },
    )
}

fn run_format<R: Runner + Send>(
    runner: &mut R,
    output: &mut impl Write,
    stage: Stage<'_>,
) -> Result<(), VerifyError> {
    verify_timing::observe(
        output,
        stage,
        || runner.format(),
        |result, _| {
            Ok(process_conclusion(
                result,
                |source| VerifyError::Launch {
                    label: FORMAT_LABEL,
                    source,
                },
                |code| VerifyError::Exit {
                    label: FORMAT_LABEL,
                    code,
                },
            ))
        },
    )
}

fn run_command<R: Runner + Send>(
    runner: &mut R,
    output: &mut impl Write,
    stage: Stage<'_>,
    step: Step,
) -> Result<(), VerifyError> {
    verify_timing::observe(
        output,
        stage,
        || runner.run(step),
        |result, _| {
            Ok(process_conclusion(
                result,
                |source| VerifyError::Launch {
                    label: step.label,
                    source,
                },
                |code| VerifyError::Exit {
                    label: step.label,
                    code,
                },
            ))
        },
    )
}

fn run_browser<R: Runner + Send>(
    runner: &mut R,
    output: &mut impl Write,
    stage: Stage<'_>,
) -> Result<(), VerifyError> {
    verify_timing::observe(
        output,
        stage,
        || runner.browser(),
        |result, _| {
            Ok(match result {
                Err(source) => Conclusion::failure(
                    VerifyError::BrowserLaunch { source },
                    Disposition::LaunchError,
                ),
                Ok(run) if !run.status.success => Conclusion::failure(
                    VerifyError::BrowserExit {
                        code: run.status.code,
                    },
                    Disposition::from_status(run.status.code),
                ),
                Ok(run) if !run.completed => {
                    Conclusion::failure(VerifyError::BrowserIncomplete, Disposition::Incomplete)
                }
                Ok(run) => Conclusion::success((), Disposition::from_status(run.status.code)),
            })
        },
    )
}

fn run_workspace_tests<R: Runner + Send>(
    runner: &mut R,
    output: &mut impl Write,
    stage: Stage<'_>,
) -> Result<(), VerifyError> {
    verify_timing::observe(
        output,
        stage,
        || runner.workspace_tests(),
        |result, _| {
            Ok(process_conclusion(
                result,
                |source| VerifyError::Launch {
                    label: super::WORKSPACE_TEST_LABEL,
                    source,
                },
                |code| VerifyError::Exit {
                    label: super::WORKSPACE_TEST_LABEL,
                    code,
                },
            ))
        },
    )
}

fn run_battery<R: Runner + Send>(
    runner: &mut R,
    output: &mut impl Write,
    stage: Stage<'_>,
) -> Result<(), VerifyError> {
    verify_timing::observe(
        output,
        stage,
        || runner.battery(),
        |result, output| match result {
            Err(detail) => Ok(Conclusion::failure(
                VerifyError::Battery(detail),
                Disposition::Error,
            )),
            Ok(BatteryRun::Admitted(document)) => {
                output.write_all(&document).map_err(VerifyError::Output)?;
                writeln!(output).map_err(VerifyError::Output)?;
                Ok(Conclusion::success((), Disposition::Success))
            }
            Ok(BatteryRun::Refused(diagnostic)) => {
                output.write_all(&diagnostic).map_err(VerifyError::Output)?;
                Ok(Conclusion::failure(
                    VerifyError::BatteryRefused,
                    Disposition::Refused,
                ))
            }
        },
    )
}

fn run_landing<R: Runner + Send>(
    runner: &mut R,
    output: &mut impl Write,
    stage: Stage<'_>,
) -> Result<(), VerifyError> {
    verify_timing::observe(
        output,
        stage,
        || runner.landing(),
        |result, output| match result {
            Err(source) => Ok(Conclusion::failure(
                VerifyError::Landing(source.to_string()),
                Disposition::Error,
            )),
            Ok(report) => {
                serde_json::to_writer(&mut *output, &report).map_err(VerifyError::Encode)?;
                writeln!(output).map_err(VerifyError::Output)?;
                if report.admitted() {
                    Ok(Conclusion::success((), Disposition::Success))
                } else {
                    Ok(Conclusion::failure(
                        VerifyError::LandingRefused,
                        Disposition::Refused,
                    ))
                }
            }
        },
    )
}

fn process_conclusion(
    result: io::Result<Status>,
    launch_error: impl FnOnce(io::Error) -> VerifyError,
    exit_error: impl FnOnce(Option<i32>) -> VerifyError,
) -> Conclusion<(), VerifyError> {
    match result {
        Ok(status) if status.success => {
            Conclusion::success((), Disposition::from_status(status.code))
        }
        Ok(status) => Conclusion::failure(
            exit_error(status.code),
            Disposition::from_status(status.code),
        ),
        Err(source) => Conclusion::failure(launch_error(source), Disposition::LaunchError),
    }
}

pub(super) fn verify_package_with<R: Runner + Send>(
    package: &str,
    runner: &mut R,
    output: &mut impl Write,
) -> Result<(), VerifyError> {
    if !valid_package_name(package) {
        return Err(VerifyError::Package(package.to_owned()));
    }

    write_scope_inventory(package, output)?;
    let total = SCOPED_COMMANDS.len() + 1;
    run_entry_point(
        runner,
        output,
        Stage {
            ordinal: 1,
            total,
            identity: ENTRY_POINT_LABEL,
            family: "in-process:entry-point-audit",
        },
    )?;

    for (index, command) in SCOPED_COMMANDS.into_iter().enumerate() {
        let label = command.label(package);
        verify_timing::observe(
            output,
            Stage {
                ordinal: index + 2,
                total,
                identity: &label,
                family: "child-process:cargo",
            },
            || runner.run_scoped(command, package),
            |result, _| {
                Ok(process_conclusion(
                    result,
                    |source| VerifyError::ScopedLaunch {
                        label: label.clone(),
                        source,
                    },
                    |code| VerifyError::ScopedExit {
                        label: label.clone(),
                        code,
                    },
                ))
            },
        )?;
    }

    Ok(())
}

fn write_scope_inventory(package: &str, output: &mut impl Write) -> Result<(), VerifyError> {
    let skipped_actions = VERIFY_ACTIONS
        .iter()
        .filter(|action| !matches!(action, Action::EntryPoint))
        .map(|action| action.label())
        .collect::<Vec<_>>()
        .join(", ");
    let skipped_arms = gate::GateId::ALL
        .into_iter()
        .map(|gate_id| gate_id.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    writeln!(
        output,
        concat!(
            "[verify] scoped package {}; feedback only: ",
            "run `cargo xtask verify` before task completion"
        ),
        package
    )
    .map_err(VerifyError::Output)?;
    writeln!(
        output,
        "[verify] scoped full actions skipped: {skipped_actions}"
    )
    .map_err(VerifyError::Output)?;
    writeln!(output, "[verify] scoped gate arms run: none").map_err(VerifyError::Output)?;
    writeln!(output, "[verify] scoped gate arms skipped: {skipped_arms}")
        .map_err(VerifyError::Output)
}

fn execute_action<R: Runner>(
    runner: &R,
    action: Action,
    cancellation: &CancellationToken,
    budget: Budget,
) -> ActionCompletion {
    match action {
        Action::EntryPoint => match runner.entry_point() {
            Ok(report) => success(report.into_bytes(), Disposition::Success),
            Err(detail) => failure(VerifyError::EntryPoint(detail), Disposition::Error),
        },
        Action::Format => execute_process(
            runner.format(cancellation, budget.cargo_jobs),
            FORMAT_LABEL,
            |source| VerifyError::Launch {
                label: FORMAT_LABEL,
                source,
            },
            |code| VerifyError::Exit {
                label: FORMAT_LABEL,
                code,
            },
        ),
        Action::Command(step) => execute_process(
            runner.run(step, cancellation, budget.cargo_jobs),
            step.label,
            |source| VerifyError::Launch {
                label: step.label,
                source,
            },
            |code| VerifyError::Exit {
                label: step.label,
                code,
            },
        ),
        Action::Browser => execute_browser(runner.browser(cancellation, budget.cargo_jobs)),
        Action::WorkspaceTests => execute_process(
            runner.workspace_tests(cancellation, budget.cargo_jobs, budget.workspace_workers),
            super::WORKSPACE_TEST_LABEL,
            |source| VerifyError::Launch {
                label: super::WORKSPACE_TEST_LABEL,
                source,
            },
            |code| VerifyError::Exit {
                label: super::WORKSPACE_TEST_LABEL,
                code,
            },
        ),
        Action::Battery => execute_battery(runner.battery(cancellation), BATTERY_LABEL),
        Action::Landing => match runner.landing() {
            Err(source) => failure(VerifyError::Landing(source.to_string()), Disposition::Error),
            Ok(report) => {
                let mut document = Vec::new();
                match serde_json::to_writer(&mut document, &report) {
                    Ok(()) if report.admitted() => {
                        document.push(b'\n');
                        success(document, Disposition::Success)
                    }
                    Ok(()) => {
                        document.push(b'\n');
                        failure_with_bytes(
                            VerifyError::LandingRefused,
                            Disposition::Refused,
                            document,
                        )
                    }
                    Err(source) => failure(VerifyError::Encode(source), Disposition::RenderError),
                }
            }
        },
    }
}

fn execute_process(
    run: Result<CommandRun, RunError>,
    label: &str,
    launch_error: impl FnOnce(io::Error) -> VerifyError,
    exit_error: impl FnOnce(Option<i32>) -> VerifyError,
) -> ActionCompletion {
    match run {
        Err(RunError::Launch(source)) => failure(launch_error(source), Disposition::LaunchError),
        Err(RunError::Cleanup(detail)) => failure(
            VerifyError::Cleanup {
                label: label.to_owned(),
                detail,
            },
            Disposition::CleanupError,
        ),
        Ok(CommandRun::Cancelled(output)) => failure_with_output(
            VerifyError::Cancelled {
                label: label.to_owned(),
            },
            Disposition::Cancelled,
            output,
        ),
        Ok(CommandRun::TimedOut(output)) => failure_with_output(
            VerifyError::TimedOut {
                label: label.to_owned(),
            },
            Disposition::TimedOut,
            output,
        ),
        Ok(CommandRun::Exited(output)) if output.status.success => success(
            process_bytes(&output),
            Disposition::from_status(output.status.code),
        ),
        Ok(CommandRun::Exited(output)) => {
            let code = output.status.code;
            failure_with_output(exit_error(code), Disposition::from_status(code), output)
        }
    }
}

fn panic_detail(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(detail) = payload.downcast_ref::<&str>() {
        (*detail).to_owned()
    } else if let Some(detail) = payload.downcast_ref::<String>() {
        detail.clone()
    } else {
        "non-string panic payload".to_owned()
    }
}

fn execute_browser(run: Result<BrowserRun, RunError>) -> ActionCompletion {
    match run {
        Err(RunError::Launch(source)) => failure(
            VerifyError::BrowserLaunch { source },
            Disposition::LaunchError,
        ),
        Err(RunError::Cleanup(detail)) => failure(
            VerifyError::Cleanup {
                label: BROWSER_LABEL.to_owned(),
                detail,
            },
            Disposition::CleanupError,
        ),
        Ok(BrowserRun {
            process: CommandRun::Cancelled(output),
            ..
        }) => failure_with_output(
            VerifyError::Cancelled {
                label: BROWSER_LABEL.to_owned(),
            },
            Disposition::Cancelled,
            output,
        ),
        Ok(BrowserRun {
            process: CommandRun::TimedOut(output),
            ..
        }) => failure_with_output(
            VerifyError::TimedOut {
                label: BROWSER_LABEL.to_owned(),
            },
            Disposition::TimedOut,
            output,
        ),
        Ok(BrowserRun {
            process: CommandRun::Exited(output),
            completed: _,
        }) if !output.status.success => {
            let code = output.status.code;
            failure_with_output(
                VerifyError::BrowserExit { code },
                Disposition::from_status(code),
                output,
            )
        }
        Ok(BrowserRun {
            process: CommandRun::Exited(output),
            completed: false,
        }) => failure_with_output(
            VerifyError::BrowserIncomplete,
            Disposition::Incomplete,
            output,
        ),
        Ok(BrowserRun {
            process: CommandRun::Exited(output),
            completed: true,
        }) => success(
            process_bytes(&output),
            Disposition::from_status(output.status.code),
        ),
    }
}

fn execute_battery(run: Result<BatteryRun, RunError>, label: &'static str) -> ActionCompletion {
    match run {
        Err(RunError::Launch(source)) => failure(
            VerifyError::Battery(source.to_string()),
            Disposition::LaunchError,
        ),
        Err(RunError::Cleanup(detail)) => failure(
            VerifyError::Cleanup {
                label: label.to_owned(),
                detail,
            },
            Disposition::CleanupError,
        ),
        Ok(BatteryRun::Admitted(document)) => {
            let mut output = document;
            if !output.is_empty() {
                output.push(b'\n');
            }
            success(output, Disposition::Success)
        }
        Ok(BatteryRun::Refused(diagnostic)) => failure_with_bytes(
            VerifyError::BatteryRefused,
            Disposition::Refused,
            diagnostic,
        ),
        Ok(BatteryRun::Error(detail)) => failure(VerifyError::Battery(detail), Disposition::Error),
        Ok(BatteryRun::Cancelled) => failure(
            VerifyError::Cancelled {
                label: label.to_owned(),
            },
            Disposition::Cancelled,
        ),
    }
}

fn success(output: Vec<u8>, disposition: Disposition) -> ActionCompletion {
    ActionCompletion {
        elapsed: Duration::ZERO,
        output,
        disposition,
        result: Ok(()),
    }
}

fn failure(error: VerifyError, disposition: Disposition) -> ActionCompletion {
    failure_with_bytes(error, disposition, Vec::new())
}

fn failure_with_output(
    error: VerifyError,
    disposition: Disposition,
    output: ProcessOutput,
) -> ActionCompletion {
    failure_with_bytes(error, disposition, process_bytes(&output))
}

fn failure_with_bytes(
    error: VerifyError,
    disposition: Disposition,
    output: Vec<u8>,
) -> ActionCompletion {
    ActionCompletion {
        elapsed: Duration::ZERO,
        output,
        disposition,
        result: Err(error),
    }
}

fn process_bytes(output: &ProcessOutput) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(output.stdout.len() + output.stderr.len());
    bytes.extend_from_slice(&output.stdout);
    bytes.extend_from_slice(&output.stderr);
    bytes
}

fn write_failure_summary(
    output: &mut impl Write,
    primary: &str,
    cleanup: &[(usize, VerifyError)],
    interrupted: bool,
) -> Result<(), VerifyError> {
    for (index, error) in cleanup {
        writeln!(
            output,
            "[verify] scheduler=cleanup-fault ordinal={} identity={:?} detail={:?}",
            index + 1,
            VERIFY_ACTIONS[*index].label(),
            error.to_string(),
        )
        .map_err(VerifyError::Output)?;
    }
    writeln!(
        output,
        "[verify] scheduler=failed primary_ordinal={} interrupted={} cleanup_faults={}",
        primary,
        interrupted,
        cleanup.len(),
    )
    .map_err(VerifyError::Output)
}

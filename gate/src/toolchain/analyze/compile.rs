//! The one bounded compiler process shared by native and Canvas analysis.

use std::ffi::OsStr;
use std::path::Path;
use std::time::{Duration, Instant};

use sill::{
    Argv, Completion, Environment, Output, StdinPolicy, StreamPolicy, WaitOutcome, WaitPolicy,
    spawn,
};

use super::{Analysis, compile_findings, complete_compiler_output, plural};
use crate::GateId;
use crate::measure::Bounds;
use crate::toolchain::{ArmOutcome, FindingDetail};

#[derive(Clone, Copy)]
pub(in crate::toolchain) struct CompileInvocation<'a> {
    pub(super) name: &'static str,
    pub(super) program: &'a str,
    pub(super) arguments: &'a [&'a str],
    pub(super) cwd: &'a Path,
    pub(super) capture: CompileCapture,
}

#[derive(Clone, Copy)]
pub(in crate::toolchain) enum CompileCapture {
    Prefix,
    CargoJsonTail,
}

pub(in crate::toolchain) struct CompileFailure {
    pub(super) analysis: Box<Analysis>,
    elapsed: Option<Duration>,
}

pub(in crate::toolchain) struct CompileSuccess {
    elapsed: Duration,
    stdout: Vec<u8>,
}

impl CompileSuccess {
    pub(super) fn elapsed(&self) -> Duration {
        self.elapsed
    }

    pub(super) fn into_stdout(self) -> Vec<u8> {
        self.stdout
    }
}

impl CompileFailure {
    fn before_start(analysis: Analysis) -> Self {
        Self {
            analysis: Box::new(analysis),
            elapsed: None,
        }
    }

    fn after_wait(analysis: Analysis, elapsed: Duration) -> Self {
        Self {
            analysis: Box::new(analysis),
            elapsed: Some(elapsed),
        }
    }

    pub(super) fn into_parts(self) -> (Analysis, Option<Duration>) {
        (*self.analysis, self.elapsed)
    }
}

pub(in crate::toolchain) fn run_compile_analysis(
    id: GateId,
    root: &Path,
    invocation: CompileInvocation<'_>,
    bounds: &Bounds,
    environment: &Environment,
    detail: fn(String, String) -> FindingDetail,
) -> Result<CompileSuccess, CompileFailure> {
    run_compile_analysis_with_wait(id, root, invocation, bounds, environment, detail, None)
}

pub(in crate::toolchain) fn run_compile_analysis_with_wait(
    id: GateId,
    root: &Path,
    invocation: CompileInvocation<'_>,
    bounds: &Bounds,
    environment: &Environment,
    detail: fn(String, String) -> FindingDetail,
    wait: Option<WaitPolicy>,
) -> Result<CompileSuccess, CompileFailure> {
    let CompileInvocation {
        name,
        program,
        arguments,
        cwd,
        capture,
    } = invocation;
    let argv = Argv::new(program, arguments.iter().map(OsStr::new)).map_err(|error| {
        CompileFailure::before_start(Analysis::terminal(
            ArmOutcome::ParseFailure,
            format!("{name} argument vector was refused: {error}"),
        ))
    })?;
    let mut spec = bounds.spec(argv, cwd);
    spec.environment = environment.clone();
    configure_output(&mut spec, capture, bounds);
    if let Some(wait) = wait {
        spec.wait = wait;
    }
    let (outcome, status, elapsed) = wait_for_compile(name, spec)?;
    let (printed, stdout) = complete_compiler_output(
        name,
        &outcome,
        matches!(capture, CompileCapture::CargoJsonTail),
    )
    .map_err(|reason| {
        CompileFailure::after_wait(
            Analysis::terminal(ArmOutcome::ParseFailure, reason),
            elapsed,
        )
    })?;
    let findings = compile_findings(root, cwd, &printed, detail);
    if !findings.is_empty() {
        return Err(CompileFailure::after_wait(
            Analysis {
                outcome: ArmOutcome::Findings,
                reason: format!(
                    "{} parsed {} from {name}",
                    id.as_str(),
                    plural(findings.len(), "diagnostic")
                ),
                findings,
                exit_code: None,
                timing: None,
                cargo_artifacts: Vec::new(),
            },
            elapsed,
        ));
    }
    if status.success() {
        return Ok(CompileSuccess { elapsed, stdout });
    }
    let Some(code) = status.code() else {
        return Err(CompileFailure::after_wait(
            Analysis::terminal(
                ArmOutcome::ParseFailure,
                format!("{name} was signalled without a typed diagnostic"),
            ),
            elapsed,
        ));
    };
    Err(CompileFailure::after_wait(
        Analysis::refused(
            format!("{name} refused with {code} and emitted no typed diagnostic"),
            code,
        ),
        elapsed,
    ))
}

fn wait_for_compile(
    name: &str,
    spec: sill::Spec,
) -> Result<(WaitOutcome, std::process::ExitStatus, Duration), CompileFailure> {
    let running = spawn(spec).map_err(|error| {
        CompileFailure::before_start(Analysis::terminal(
            ArmOutcome::ParseFailure,
            format!("{name} did not start: {error}"),
        ))
    })?;
    let started = Instant::now();
    let outcome = running.wait().map_err(|error| {
        CompileFailure::after_wait(
            Analysis::terminal(
                ArmOutcome::ParseFailure,
                format!("{name} could not be waited on: {error}"),
            ),
            started.elapsed(),
        )
    })?;
    let elapsed = started.elapsed();
    let status = match outcome.completion() {
        Completion::Exited(status) => *status,
        Completion::TimedOut(_) => {
            let analysis = Analysis::terminal(
                ArmOutcome::Timeout,
                format!("{name} outlived the bounds it was given"),
            );
            return Err(CompileFailure::after_wait(analysis, elapsed));
        }
        Completion::Cancelled(_) => {
            let analysis = Analysis::terminal(
                ArmOutcome::Cancelled,
                format!("{name} was cancelled before it reported"),
            );
            return Err(CompileFailure::after_wait(analysis, elapsed));
        }
    };
    Ok((outcome, status, elapsed))
}

fn configure_output(spec: &mut sill::Spec, capture: CompileCapture, bounds: &Bounds) {
    if matches!(capture, CompileCapture::CargoJsonTail) {
        spec.output = Output::new(
            StdinPolicy::Null,
            StreamPolicy::CaptureTail {
                limit: bounds.capture(),
            },
            StreamPolicy::Capture {
                limit: bounds.capture(),
            },
        );
    }
}

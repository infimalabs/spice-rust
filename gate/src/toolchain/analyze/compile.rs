//! The one bounded compiler process shared by native and Canvas analysis.

use std::ffi::OsStr;
use std::path::Path;
use std::time::{Duration, Instant};

use sill::{Argv, Completion, Environment, WaitPolicy, spawn};

use super::{Analysis, compile_findings, complete_output, plural};
use crate::GateId;
use crate::measure::Bounds;
use crate::toolchain::{ArmOutcome, FindingDetail};

#[derive(Clone, Copy)]
pub(in crate::toolchain) struct CompileInvocation<'a> {
    pub(super) name: &'static str,
    pub(super) program: &'a str,
    pub(super) arguments: &'a [&'a str],
    pub(super) cwd: &'a Path,
}

pub(in crate::toolchain) struct CompileFailure {
    pub(super) analysis: Analysis,
    elapsed: Option<Duration>,
}

impl CompileFailure {
    fn before_start(analysis: Analysis) -> Self {
        Self {
            analysis,
            elapsed: None,
        }
    }

    fn after_wait(analysis: Analysis, elapsed: Duration) -> Self {
        Self {
            analysis,
            elapsed: Some(elapsed),
        }
    }

    pub(super) fn into_parts(self) -> (Analysis, Option<Duration>) {
        (self.analysis, self.elapsed)
    }
}

pub(in crate::toolchain) fn run_compile_analysis(
    id: GateId,
    root: &Path,
    invocation: CompileInvocation<'_>,
    bounds: &Bounds,
    environment: &Environment,
    detail: fn(String, String) -> FindingDetail,
) -> Result<Duration, CompileFailure> {
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
) -> Result<Duration, CompileFailure> {
    let CompileInvocation {
        name,
        program,
        arguments,
        cwd,
    } = invocation;
    let argv = Argv::new(program, arguments.iter().map(OsStr::new)).map_err(|error| {
        CompileFailure::before_start(Analysis::terminal(
            ArmOutcome::ParseFailure,
            format!("{name} argument vector was refused: {error}"),
        ))
    })?;
    let mut spec = bounds.spec(argv, cwd);
    spec.environment = environment.clone();
    if let Some(wait) = wait {
        spec.wait = wait;
    }
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
        Completion::Exited(status) => status,
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
    let printed = complete_output(name, &outcome).map_err(|reason| {
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
            },
            elapsed,
        ));
    }
    if status.success() {
        return Ok(elapsed);
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

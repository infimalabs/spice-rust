//! Live compiler-output observation for the full-verifier progress lease.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use sill::{
    Observation, ObserveError, ObservedRecord, OutputStream, RecordObserver, Spec, WaitOutcome,
};

use super::{OUTPUT_LIMIT, READY_FRAME_LIMIT, READY_QUEUE_CAPACITY, VerifierCompilerStage};

pub(super) fn wait_for_compiler(
    spec: Spec,
    stage: VerifierCompilerStage,
    readiness: Option<Arc<dyn Fn() + Send + Sync>>,
    progress: Option<Arc<dyn Fn() + Send + Sync>>,
) -> io::Result<(WaitOutcome, Option<bool>)> {
    let readiness = readiness
        .map(|ready| CompilerReadinessObserver::new(ready, progress.clone()))
        .map(Arc::new);
    let outcome = match (&readiness, progress) {
        (Some(observer), _) => wait_observed(
            spec,
            OutputStream::Stdout,
            READY_FRAME_LIMIT,
            Arc::clone(observer) as Arc<dyn RecordObserver>,
        )?,
        (None, Some(progress)) => wait_observed(
            spec,
            OutputStream::Stderr,
            OUTPUT_LIMIT,
            Arc::new(CompilerProgressObserver { progress }),
        )?,
        (None, None) => sill::spawn(spec)
            .map_err(io::Error::other)?
            .wait()
            .map_err(io::Error::other)?,
    };
    validate_observation(&outcome, stage)?;
    Ok((
        outcome,
        readiness.as_ref().map(|observer| observer.observed()),
    ))
}

fn wait_observed(
    spec: Spec,
    stream: OutputStream,
    frame_limit: usize,
    observer: Arc<dyn RecordObserver>,
) -> io::Result<WaitOutcome> {
    let observation = Observation::new(stream, frame_limit, READY_QUEUE_CAPACITY, observer)
        .map_err(io::Error::other)?;
    sill::spawn_observed(spec, observation)
        .map_err(io::Error::other)?
        .wait()
        .map_err(io::Error::other)
}

fn validate_observation(outcome: &WaitOutcome, stage: VerifierCompilerStage) -> io::Result<()> {
    if let Some(failure) = outcome.observation_failure() {
        return Err(io::Error::other(format!(
            "{} compiler readiness observation failed: {failure}",
            stage.tag()
        )));
    }
    if !outcome.outputs_drained() {
        return Err(io::Error::other(format!(
            "{} compiler output did not drain before cleanup",
            stage.tag()
        )));
    }
    Ok(())
}

struct CompilerReadinessObserver {
    ready: Arc<dyn Fn() + Send + Sync>,
    progress: Option<Arc<dyn Fn() + Send + Sync>>,
    observed: AtomicBool,
}

impl CompilerReadinessObserver {
    fn new(
        ready: Arc<dyn Fn() + Send + Sync>,
        progress: Option<Arc<dyn Fn() + Send + Sync>>,
    ) -> Self {
        Self {
            ready,
            progress,
            observed: AtomicBool::new(false),
        }
    }

    fn observed(&self) -> bool {
        self.observed.load(Ordering::Acquire)
    }
}

impl RecordObserver for CompilerReadinessObserver {
    fn observe(&self, record: ObservedRecord<'_>) -> Result<(), ObserveError> {
        if let Some(progress) = &self.progress {
            progress();
        }
        if record.bytes().strip_suffix(b"\n")
            == Some(crate::VERIFIER_APP_COMPILER_READY_RECORD.as_bytes())
            && !self.observed.swap(true, Ordering::AcqRel)
        {
            (self.ready)();
        }
        Ok(())
    }
}

struct CompilerProgressObserver {
    progress: Arc<dyn Fn() + Send + Sync>,
}

impl RecordObserver for CompilerProgressObserver {
    fn observe(&self, _: ObservedRecord<'_>) -> Result<(), ObserveError> {
        (self.progress)();
        Ok(())
    }
}

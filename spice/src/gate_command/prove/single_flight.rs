use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, SystemTime};

use gate::{EvidenceLease, EvidenceStore, LeaseAcquisition};
use notify::{RecursiveMode, Watcher as _};

use super::output::{Elapsed, Progress};
use super::{PreparedAction, ProveActionReport, ProveDisposition, ProveFailure, execute};

#[cfg(test)]
#[path = "single_flight/tests.rs"]
mod tests;

#[derive(Clone, Copy)]
struct Timing {
    lease_lifetime: Duration,
    renew_interval: Duration,
    wait_limit: Duration,
}

const PRODUCTION_TIMING: Timing = Timing {
    lease_lifetime: Duration::from_secs(30),
    renew_interval: Duration::from_secs(10),
    wait_limit: Duration::from_secs(600),
};

pub(super) fn run(
    store: &EvidenceStore,
    actions: Vec<PreparedAction>,
    mut validate: impl FnMut() -> Result<(), ProveFailure>,
    cleanup: impl FnOnce() -> Result<(), ProveFailure>,
    progress: &Progress,
    captures: &Path,
) -> Result<Vec<ProveActionReport>, ProveFailure> {
    // One identity correlates this run's leases and every receipt it mints.
    let run = wire::CanonicalUuid::new(uuid::Uuid::new_v4());
    let owner = format!("proof:{}:{}", std::process::id(), run.get().simple());
    let total = actions.len();
    let mut present = Vec::new();
    let mut failure = None;
    for (index, action) in actions.into_iter().enumerate() {
        let step = format!("{}/{total} {}", index + 1, action.action);
        let queue_started = std::time::Instant::now();
        let claimed = match claim(store, &owner, &action.action, &action.action_key) {
            Ok(claimed) => claimed,
            Err(error) => {
                failure = Some(error);
                break;
            }
        };
        let queued = queue_started.elapsed();
        match claimed {
            Claim::Present(report) => {
                progress.line(format_args!("{step} reused"));
                present.push(report);
            }
            Claim::Lease(lease) => {
                progress.line(format_args!("{step} started"));
                let (executed, elapsed) =
                    progress.during(&step, || execute(action, captures, run, queued));
                let receipt = match executed {
                    Ok(receipt) => receipt,
                    Err(error) => {
                        progress.line(format_args!("{step} failed after {}", Elapsed(elapsed)));
                        lease.fail(&error);
                        failure = Some(error);
                        break;
                    }
                };
                if let Err(error) = validate() {
                    lease.fail(&error);
                    failure = Some(error);
                    break;
                }
                if let Err(error) = lease.renew_now() {
                    lease.fail(&error);
                    failure = Some(error);
                    break;
                }
                let current = lease.current();
                let publication = match store
                    .publish_batch_leased([(&receipt.receipt, &current)], SystemTime::now())
                    .map_err(ProveFailure::store)
                {
                    Ok(mut publications) => publications.remove(0),
                    Err(error) => {
                        lease.fail(&error);
                        failure = Some(error);
                        break;
                    }
                };
                progress.line(format_args!("{step} proved in {}", Elapsed(elapsed)));
                present.push(ProveActionReport {
                    action: receipt.action,
                    action_key: receipt.action_key,
                    disposition: ProveDisposition::Published,
                    receipt: publication.receipt().clone(),
                    elapsed: Some(elapsed),
                    capture: receipt.capture,
                });
            }
        }
    }
    if failure.is_none()
        && let Err(error) = validate()
    {
        failure = Some(error);
    }
    if let Err(error) = cleanup()
        && failure.is_none()
    {
        failure = Some(error);
    }
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(present)
}

enum Claim {
    Present(ProveActionReport),
    Lease(Box<LeaseGuard>),
}

fn claim(
    store: &EvidenceStore,
    owner: &str,
    action: &str,
    action_key: &gate::ActionKey,
) -> Result<Claim, ProveFailure> {
    claim_with_timing(store, owner, action, action_key, PRODUCTION_TIMING)
}

fn claim_with_timing(
    store: &EvidenceStore,
    owner: &str,
    action: &str,
    action_key: &gate::ActionKey,
    timing: Timing,
) -> Result<Claim, ProveFailure> {
    if let Some(receipt) = store.lookup(action_key).map_err(ProveFailure::store)? {
        return present(action, action_key, receipt);
    }
    match store
        .acquire_lease(action_key, owner, SystemTime::now(), timing.lease_lifetime)
        .map_err(ProveFailure::store)?
    {
        LeaseAcquisition::Acquired { lease, .. } => {
            return acquired(store, action, action_key, lease, false, timing);
        }
        LeaseAcquisition::Failed {
            reason,
            detail,
            fault,
            ..
        } => return Err(shared_failure(reason, detail, fault)),
        LeaseAcquisition::Busy { .. } => {}
    }
    let started = std::time::Instant::now();
    let (changes, receiver) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(move |event| {
        let _ = changes.send(event);
    })
    .map_err(watch_failure)?;
    watcher
        .watch(store.root(), RecursiveMode::Recursive)
        .map_err(watch_failure)?;
    loop {
        if let Some(receipt) = store.lookup(action_key).map_err(ProveFailure::store)? {
            return present(action, action_key, receipt);
        }
        match store
            .acquire_lease(action_key, owner, SystemTime::now(), timing.lease_lifetime)
            .map_err(ProveFailure::store)?
        {
            LeaseAcquisition::Acquired { lease, recovered } => {
                return acquired(store, action, action_key, lease, !recovered, timing);
            }
            LeaseAcquisition::Busy {
                expires_at_millis, ..
            } => {
                let remaining = timing.wait_limit.saturating_sub(started.elapsed());
                if remaining.is_zero() {
                    return Err(ProveFailure::refusal(
                        "gate::prove-peer-action-timeout",
                        format!("timed out waiting for action {action}"),
                    ));
                }
                let until_expiry = Duration::from_millis(expires_at_millis).saturating_sub(
                    SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default(),
                );
                match receiver.recv_timeout(remaining.min(until_expiry)) {
                    Ok(Ok(_)) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Ok(Err(error)) => return Err(watch_failure(error)),
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        return Err(ProveFailure::fault(
                            "gate::prove-peer-observer-lost",
                            "evidence-store observer disconnected",
                        ));
                    }
                }
            }
            LeaseAcquisition::Failed {
                reason,
                detail,
                fault,
                ..
            } => {
                return Err(shared_failure(reason, detail, fault));
            }
        }
    }
}

fn present(
    action: &str,
    action_key: &gate::ActionKey,
    receipt: gate::PassReceipt,
) -> Result<Claim, ProveFailure> {
    Ok(Claim::Present(ProveActionReport::present(
        action.to_owned(),
        action_key.clone(),
        receipt.id().map_err(ProveFailure::store)?,
    )))
}

fn acquired(
    store: &EvidenceStore,
    action: &str,
    action_key: &gate::ActionKey,
    lease: EvidenceLease,
    released_by_peer: bool,
    timing: Timing,
) -> Result<Claim, ProveFailure> {
    if let Some(receipt) = store.lookup(action_key).map_err(ProveFailure::store)? {
        store.release_lease(&lease).map_err(ProveFailure::store)?;
        return present(action, action_key, receipt);
    }
    if released_by_peer {
        store.release_lease(&lease).map_err(ProveFailure::store)?;
        return Err(ProveFailure::refusal(
            "gate::prove-peer-action-failed",
            format!("action {action} was released without a receipt"),
        ));
    }
    Ok(Claim::Lease(Box::new(LeaseGuard::new(
        store.clone(),
        lease,
        timing,
    ))))
}

fn shared_failure(reason: String, detail: String, fault: bool) -> ProveFailure {
    if fault {
        ProveFailure::fault(reason, detail)
    } else {
        ProveFailure::refusal(reason, detail)
    }
}

fn watch_failure(error: notify::Error) -> ProveFailure {
    ProveFailure::fault("gate::prove-peer-observer-failed", error)
}

struct LeaseGuard {
    store: EvidenceStore,
    lease: Arc<Mutex<EvidenceLease>>,
    failure: Arc<Mutex<Option<(String, String)>>>,
    stop: mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
    terminal: AtomicBool,
    lease_lifetime: Duration,
}

impl LeaseGuard {
    fn new(store: EvidenceStore, lease: EvidenceLease, timing: Timing) -> Self {
        let lease = Arc::new(Mutex::new(lease));
        let failure = Arc::new(Mutex::new(None));
        let (stop, stopped) = mpsc::channel();
        let worker_store = store.clone();
        let worker_lease = Arc::clone(&lease);
        let worker_failure = Arc::clone(&failure);
        let worker = thread::spawn(move || {
            while stopped.recv_timeout(timing.renew_interval).is_err() {
                let mut lease = worker_lease.lock().expect("lease mutex is not poisoned");
                match worker_store.renew_lease(&lease, SystemTime::now(), timing.lease_lifetime) {
                    Ok(renewed) => *lease = renewed,
                    Err(error) => {
                        *worker_failure
                            .lock()
                            .expect("failure mutex is not poisoned") =
                            Some((error.reason().to_owned(), error.detail().to_owned()));
                        break;
                    }
                }
            }
        });
        Self {
            store,
            lease,
            failure,
            stop,
            worker: Some(worker),
            terminal: AtomicBool::new(false),
            lease_lifetime: timing.lease_lifetime,
        }
    }

    fn renew_now(&self) -> Result<(), ProveFailure> {
        if let Some((reason, detail)) = self
            .failure
            .lock()
            .expect("failure mutex is not poisoned")
            .clone()
        {
            return Err(ProveFailure::refusal(reason, detail));
        }
        let mut lease = self.lease.lock().expect("lease mutex is not poisoned");
        *lease = self
            .store
            .renew_lease(&lease, SystemTime::now(), self.lease_lifetime)
            .map_err(ProveFailure::store)?;
        Ok(())
    }

    fn current(&self) -> EvidenceLease {
        self.lease
            .lock()
            .expect("lease mutex is not poisoned")
            .clone()
    }

    fn fail(&self, failure: &ProveFailure) {
        let lease = self.current();
        if self
            .store
            .fail_lease(
                &lease,
                failure.reason(),
                failure.detail(),
                failure.is_fault(),
            )
            .is_ok()
        {
            self.terminal.store(true, Ordering::Release);
        }
    }
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        if !self.terminal.load(Ordering::Acquire)
            && let Ok(lease) = self.lease.lock()
        {
            let _ = self.store.release_lease(&lease);
        }
    }
}

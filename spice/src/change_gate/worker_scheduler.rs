//! One process-root permit pool shared by policy and selected-test execution.

use std::sync::{Arc, Condvar, Mutex};

use super::ChangeGateError;

#[derive(Debug)]
struct State {
    active: usize,
}

#[derive(Clone, Debug)]
pub(super) struct WorkerScheduler {
    capacity: usize,
    state: Arc<(Mutex<State>, Condvar)>,
}

impl WorkerScheduler {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            state: Arc::new((Mutex::new(State { active: 0 }), Condvar::new())),
        }
    }

    pub(super) fn acquire(&self) -> Result<WorkerPermit, ChangeGateError> {
        let (state, changed) = &*self.state;
        let mut state = state.lock().map_err(|_| scheduler_poisoned())?;
        while state.active >= self.capacity {
            state = changed.wait(state).map_err(|_| scheduler_poisoned())?;
        }
        state.active += 1;
        Ok(WorkerPermit {
            state: Arc::clone(&self.state),
        })
    }
}

#[derive(Debug)]
pub(super) struct WorkerPermit {
    state: Arc<(Mutex<State>, Condvar)>,
}

impl Drop for WorkerPermit {
    fn drop(&mut self) {
        let (state, changed) = &*self.state;
        let Ok(mut state) = state.lock() else {
            return;
        };
        state.active = state.active.saturating_sub(1);
        changed.notify_one();
    }
}

fn scheduler_poisoned() -> ChangeGateError {
    ChangeGateError::protocol(
        "change-gate-worker-scheduler-poisoned",
        "shared gate worker scheduler state was poisoned",
    )
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    fn spawn_blocked_process() -> (Child, u32) {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "printf '%s\\n' $$; read release"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("launch observable worker process");
        let mut line = String::new();
        BufReader::new(child.stdout.take().expect("worker stdout"))
            .read_line(&mut line)
            .expect("read worker pid");
        let pid = line.trim().parse().expect("worker publishes numeric pid");
        (child, pid)
    }

    fn process_is_live(pid: u32) -> bool {
        Command::new("/bin/ps")
            .args(["-p", &pid.to_string()])
            .stdout(Stdio::null())
            .status()
            .expect("inspect worker pid")
            .success()
    }

    #[test]
    fn overlapping_policy_and_test_roots_share_three_permits_and_a_fourth_waits() {
        let scheduler = WorkerScheduler::new(3);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let mut workers = Vec::new();
        for ordinal in 0..4 {
            let scheduler = scheduler.clone();
            let entered = entered_tx.clone();
            let release = Arc::clone(&release_rx);
            workers.push(std::thread::spawn(move || {
                let _permit = scheduler.acquire().expect("scheduler admits worker");
                let (mut child, pid) = spawn_blocked_process();
                entered
                    .send((ordinal, pid))
                    .expect("publish admitted worker");
                release
                    .lock()
                    .expect("release receiver")
                    .recv()
                    .expect("release worker");
                drop(child.stdin.take());
                child.wait().expect("reap worker");
            }));
        }
        drop(entered_tx);

        let first = (0..3)
            .map(|_| {
                entered_rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("three proceed")
            })
            .collect::<Vec<_>>();
        assert_eq!(first.len(), 3);
        assert!(first.iter().all(|(_, pid)| process_is_live(*pid)));
        let fourth_waited = entered_rx.recv_timeout(Duration::from_millis(100)).is_err();
        release_tx.send(()).expect("release one permit");
        let fourth = entered_rx.recv_timeout(Duration::from_secs(5));
        if let Ok((_, fourth_pid)) = fourth {
            assert!(process_is_live(fourth_pid));
        }
        for _ in 0..3 {
            release_tx.send(()).expect("release remaining worker");
        }
        for worker in workers {
            worker.join().expect("worker joins");
        }
        fourth.expect("fourth proceeds after one permit returns");
        assert!(
            fourth_waited,
            "the fourth process root waits behind the shared cap"
        );
    }

    #[test]
    fn four_agent_schedulers_collectively_admit_twelve_process_roots() {
        let schedulers = (0..4).map(|_| WorkerScheduler::new(3)).collect::<Vec<_>>();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Arc::new(Mutex::new(release_rx));
        let mut workers = Vec::new();
        for scheduler in schedulers {
            for _ in 0..3 {
                let scheduler = scheduler.clone();
                let entered = entered_tx.clone();
                let release = Arc::clone(&release_rx);
                workers.push(std::thread::spawn(move || {
                    let _permit = scheduler.acquire().expect("agent scheduler admits worker");
                    let (mut child, pid) = spawn_blocked_process();
                    entered.send(pid).expect("publish admitted worker");
                    release
                        .lock()
                        .expect("release receiver")
                        .recv()
                        .expect("release worker");
                    drop(child.stdin.take());
                    child.wait().expect("reap worker");
                }));
            }
        }
        drop(entered_tx);

        let pids = (0..12)
            .map(|_| {
                entered_rx
                    .recv_timeout(Duration::from_secs(5))
                    .expect("four agents admit twelve roots")
            })
            .collect::<Vec<_>>();
        assert!(pids.iter().all(|pid| process_is_live(*pid)));
        for _ in 0..12 {
            release_tx.send(()).expect("release worker");
        }
        for worker in workers {
            worker.join().expect("worker joins");
        }
    }
}

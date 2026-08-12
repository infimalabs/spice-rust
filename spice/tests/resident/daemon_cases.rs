//! Detached and foreground launcher contract cases.

use super::*;

#[cfg(unix)]
struct DetachedResidentGuard<'a> {
    fixture: &'a Fixture,
    armed: bool,
}

#[cfg(unix)]
impl Drop for DetachedResidentGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = invoke(self.fixture, "down");
        }
    }
}

#[cfg(unix)]
#[test]
fn default_up_detaches_proves_authentication_and_retains_no_credential_surface() {
    const TOKEN: &str = "correct-horse-battery-staple";

    let fixture = Fixture::new("daemon-default");
    let launched = invoke_args(
        &fixture,
        &["up", "--port", "0", "--auth-token", TOKEN],
        true,
    );
    assert!(
        launched.status.success(),
        "launcher failed: {}",
        String::from_utf8_lossy(&launched.stderr)
    );
    let mut cleanup = DetachedResidentGuard {
        fixture: &fixture,
        armed: true,
    };
    let Outcome::Reply(ready) =
        wire::decode::<Outcome<wire::CanvasOpen>>(&launched.stdout).expect("readiness decodes")
    else {
        panic!("default up did not return a readiness reply")
    };
    let discovery_bytes = fs::read(fixture.canvas()).expect("daemon discovery reads");
    let discovery: canvas::Discovery =
        wire::decode(&discovery_bytes).expect("daemon discovery decodes");
    assert_eq!(ready.body.endpoint, discovery.endpoint);
    assert_eq!(ready.body.epoch, discovery.epoch);

    let holder = fs::read_to_string(fixture.resident()).expect("resident holder reads");
    let pid = holder
        .strip_prefix("pid=")
        .and_then(|holder| holder.split_once(';'))
        .and_then(|(pid, _)| pid.parse::<u32>().ok())
        .unwrap_or_else(|| panic!("resident holder names its pid: {holder:?}"));
    assert!(
        process_alive(pid),
        "launcher exit did not preserve resident {pid}"
    );
    // SAFETY: getpgid and getpgrp only inspect process identities.
    let group = unsafe { libc::getpgid(i32::try_from(pid).expect("pid fits i32")) };
    assert_eq!(group, i32::try_from(pid).expect("pid fits i32"));
    assert_ne!(group, unsafe { libc::getpgrp() });

    let process = Command::new("ps")
        .args(["eww", "-p", &pid.to_string()])
        .output()
        .expect("resident process listing runs");
    assert!(
        process.status.success(),
        "resident process listing succeeds"
    );
    let log = fs::read(fixture.runtime.join("resident.log")).expect("resident log reads");
    for surface in [
        &launched.stdout[..],
        &discovery_bytes[..],
        &log[..],
        &process.stdout[..],
    ] {
        assert!(
            !surface
                .windows(TOKEN.len())
                .any(|window| window == TOKEN.as_bytes()),
            "auth token escaped its one-shot handoff"
        );
    }
    let down = invoke(&fixture, "down");
    assert!(
        down.status.success(),
        "daemon down failed: {}",
        String::from_utf8_lossy(&down.stderr)
    );
    readiness::process_exit::ProcessExitObserver::or_ended(pid).wait();
    assert!(
        !process_alive(pid),
        "down left detached resident {pid} alive"
    );
    assert!(!fixture.canvas().exists(), "down withdrew daemon discovery");
    cleanup.armed = false;
}

#[cfg(unix)]
#[test]
fn failed_daemon_start_reaps_its_unique_process_group_and_releases_runtime_state() {
    let fixture = Fixture::new("daemon-bind-failure");
    let reservation = TcpListener::bind(("127.0.0.1", 0)).expect("reserve daemon failure port");
    let port = reservation
        .local_addr()
        .expect("reserved daemon failure address")
        .port();
    let port_text = port.to_string();

    let launched = invoke_args(&fixture, &["up", "--port", &port_text], true);

    assert_eq!(launched.status.code(), Some(FAULT_EXIT));
    let Outcome::Fault(fault) = outcome(&launched) else {
        panic!("taken daemon port must return its typed bind fault")
    };
    assert_eq!(fault.reason, "canvas::bind-failed");
    let process_signature = format!("--foreground --host 127.0.0.1 --port {port}");
    let processes = Command::new("ps")
        .args(["-axo", "pid=,stat=,command="])
        .output()
        .expect("post-failure process audit runs");
    assert!(processes.status.success(), "process audit succeeds");
    let processes = String::from_utf8_lossy(&processes.stdout);
    assert!(
        !processes
            .lines()
            .any(|line| line.contains(&process_signature)),
        "failed daemon child or zombie survived: {processes}"
    );
    assert!(
        !fixture.canvas().exists(),
        "failed start withdrew discovery"
    );
    let possession = hold(&fixture.resident(), "post-failure-audit\n");
    drop(possession);
}

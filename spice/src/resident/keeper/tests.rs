//! Contracts of the lifecycle owner that hold on every host: what the task
//! says, what the record relaunches, and when the keeper stops relaunching.

use std::collections::VecDeque;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::definition::{self, Definition};
use super::owner;
use super::policy::{self, Attempt, End, Exit, MAX_PAUSE, RELAUNCH_PAUSE, STABLE_RUN};
use super::record::{self, KeeperRecord};
use super::scheduler::{Scheduler, Status};

/// A scheduler that records what it was asked, in order.
#[derive(Default)]
struct RecordingScheduler {
    registered: bool,
    calls: Vec<String>,
}

impl Scheduler for RecordingScheduler {
    fn register(&mut self, _path: &str, definition: &str) -> io::Result<()> {
        self.registered = true;
        self.calls.push(format!("register:{}", definition.len()));
        Ok(())
    }

    fn status(&mut self, _path: &str) -> io::Result<Option<Status>> {
        self.calls.push("status".to_owned());
        Ok(self.registered.then_some(Status {
            enabled: true,
            running: false,
        }))
    }

    fn set_enabled(&mut self, _path: &str, enabled: bool) -> io::Result<()> {
        self.calls.push(format!("enabled:{enabled}"));
        Ok(())
    }

    fn run(&mut self, _path: &str) -> io::Result<()> {
        self.calls.push("run".to_owned());
        Ok(())
    }

    fn stop(&mut self, _path: &str) -> io::Result<()> {
        self.calls.push("stop".to_owned());
        Ok(())
    }

    fn delete(&mut self, _path: &str) -> io::Result<()> {
        self.registered = false;
        self.calls.push("delete".to_owned());
        Ok(())
    }
}

#[test]
fn apply_registers_before_it_allows_and_starts_and_hold_skips_an_absent_task() {
    let mut scheduler = RecordingScheduler::default();
    owner::hold(&mut scheduler, "\\Spice\\keeper").expect("holding nothing is fine");
    assert_eq!(
        scheduler.calls,
        ["status"],
        "an absent task is never disabled"
    );
    owner::apply(&mut scheduler, "\\Spice\\keeper", "<Task/>").expect("apply");
    owner::hold(&mut scheduler, "\\Spice\\keeper").expect("hold");
    owner::release(&mut scheduler, "\\Spice\\keeper").expect("release");
    assert_eq!(
        scheduler.calls,
        [
            "status",
            "register:7",
            "enabled:true",
            "run",
            "status",
            "enabled:false",
            "stop",
            "delete"
        ]
    );
}

#[test]
fn removing_artifacts_tolerates_entries_already_gone() {
    let root = std::env::temp_dir().join(format!("spice-keeper-artifacts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("a scratch directory");
    let present = root.join("keeper.log");
    std::fs::write(&present, b"log").expect("a log to remove");
    owner::remove_artifacts(&[&present, &root.join("keeper.json")]).expect("removal");
    assert!(!present.exists());
    std::fs::remove_dir_all(&root).expect("scratch cleanup");
}

/// A scheduler whose answers are scripted per `status` call.
struct ScriptedScheduler {
    statuses: VecDeque<Option<Status>>,
}

impl Scheduler for ScriptedScheduler {
    fn register(&mut self, _path: &str, _definition: &str) -> io::Result<()> {
        Ok(())
    }

    fn status(&mut self, _path: &str) -> io::Result<Option<Status>> {
        Ok(self
            .statuses
            .pop_front()
            .expect("the script covers every status call"))
    }

    fn set_enabled(&mut self, _path: &str, _enabled: bool) -> io::Result<()> {
        Ok(())
    }

    fn run(&mut self, _path: &str) -> io::Result<()> {
        Ok(())
    }

    fn stop(&mut self, _path: &str) -> io::Result<()> {
        Ok(())
    }

    fn delete(&mut self, _path: &str) -> io::Result<()> {
        Ok(())
    }
}

const ENABLED: Option<Status> = Some(Status {
    enabled: true,
    running: true,
});
const DISABLED: Option<Status> = Some(Status {
    enabled: false,
    running: true,
});

fn served(ran_for: Duration) -> Attempt {
    Attempt::Ready {
        endpoint: "http://127.0.0.1:4242".to_owned(),
        wait: Box::new(move || Exit {
            detail: "exit status: 0".to_owned(),
            ran_for,
        }),
    }
}

fn record() -> KeeperRecord {
    KeeperRecord {
        task: "\\Spice\\keeper-0123456789abcdef".to_owned(),
        executable: PathBuf::from(r"C:\Spice\spice.exe"),
        working_directory: PathBuf::from(r"C:\Users\c\devel\repo"),
        listen: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 0),
        allow_insecure_bind: true,
        auth_token: Some("0123456789abcdef".to_owned()),
    }
}

#[test]
fn the_keeper_relaunches_after_every_exit_until_the_task_is_disabled() {
    let mut scheduler = ScriptedScheduler {
        statuses: VecDeque::from([ENABLED, ENABLED, ENABLED, DISABLED]),
    };
    let mut attempts = VecDeque::from([
        served(STABLE_RUN * 2),
        Attempt::Failed("port in use".to_owned()),
        served(Duration::from_secs(1)),
    ]);
    let mut launches = 0;
    let mut attempt = || {
        launches += 1;
        attempts.pop_front().expect("no attempt after the disable")
    };
    let mut pauses = Vec::new();
    let mut pause = |delay| pauses.push(delay);
    let mut log = Vec::new();

    let end = policy::keep(
        &mut scheduler,
        "\\Spice\\keeper",
        &mut attempt,
        &mut pause,
        &mut log,
    )
    .expect("the scripted scheduler answers");

    assert_eq!(end, End::Disabled);
    assert_eq!(launches, 3, "clean exit, refusal, and crash each relaunch");
    assert_eq!(
        pauses,
        vec![
            policy::pause_after(0),
            policy::pause_after(1),
            policy::pause_after(2)
        ],
        "a stable run resets the pace; early failures lengthen it"
    );
    let log = String::from_utf8(log).expect("the log is text");
    assert!(log.contains("port in use"), "refusals are durable: {log}");
    assert!(log.contains("is disabled; keeper exits"), "{log}");
}

#[test]
fn a_deleted_task_ends_the_keeper_without_another_launch() {
    let mut scheduler = ScriptedScheduler {
        statuses: VecDeque::from([None]),
    };
    let mut attempt = || panic!("an unregistered owner launches nothing");
    let mut pause = |_| panic!("an unregistered owner waits for nothing");
    let end = policy::keep(
        &mut scheduler,
        "\\Spice\\keeper",
        &mut attempt,
        &mut pause,
        &mut io::sink(),
    )
    .expect("the scripted scheduler answers");
    assert_eq!(end, End::Unregistered);
}

#[test]
fn relaunch_pacing_doubles_from_the_base_and_caps() {
    assert_eq!(policy::pause_after(0), RELAUNCH_PAUSE);
    assert_eq!(policy::pause_after(1), RELAUNCH_PAUSE * 2);
    assert_eq!(policy::pause_after(3), RELAUNCH_PAUSE * 8);
    assert_eq!(policy::pause_after(10), MAX_PAUSE);
    assert_eq!(policy::pause_after(u32::MAX), MAX_PAUSE);
}

#[test]
fn the_record_relaunches_with_its_listen_flags_and_keeps_the_token_off_argv() {
    let record = record();
    let argv = record.launch_argv();
    assert_eq!(
        argv,
        [
            "spice",
            "--json",
            "up",
            "--host",
            "0.0.0.0",
            "--port",
            "0",
            "--allow-insecure-bind"
        ]
    );
    let matches = crate::cli::command()
        .try_get_matches_from(&argv)
        .expect("the synthesized argv parses");
    let leaf = crate::cli::selected_matches(&matches);
    assert!(leaf.get_flag("allow-insecure-bind"));
    assert_eq!(leaf.get_one::<String>("auth-token"), None);
    assert!(!leaf.get_flag("foreground"), "the keeper launches detached");
}

#[test]
fn an_ephemeral_install_pins_the_port_the_first_resident_bound() {
    let mut record = record();
    assert!(record.pin_port("http://127.0.0.1:51234"));
    assert_eq!(record.listen.port(), 51234);
    assert!(
        !record.pin_port("http://127.0.0.1:60000"),
        "a pinned port is never moved"
    );
    assert_eq!(record.listen.port(), 51234);
    assert_eq!(record::endpoint_port("http://[::1]:8080/"), Some(8080));
    assert_eq!(record::endpoint_port("http://localhost"), None);
}

#[test]
fn the_record_round_trips_privately_and_reads_absent_as_uninstalled() {
    let root = std::env::temp_dir().join(format!("spice-keeper-record-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("a scratch directory");
    let path = root.join("keeper.json");
    assert_eq!(record::read(&path).expect("absent reads"), None);
    let written = record();
    record::write(&path, &written).expect("the record writes");
    record::write(&path, &written).expect("rewriting replaces in place");
    assert_eq!(
        record::read(&path).expect("the record reads"),
        Some(written)
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "the token never leaves the owner");
    }
    std::fs::remove_dir_all(&root).expect("scratch cleanup");
}

#[test]
fn the_task_runs_this_executable_as_the_user_at_logon_without_a_password() {
    let xml = definition::render(&Definition {
        user_sid: "S-1-5-21-1-2-3-1001",
        executable: Path::new(r"C:\Program Files\Spice & Co\spice.exe"),
        working_directory: Path::new(r"C:\Users\c\devel\repo"),
        repository: Path::new(r"C:\Users\c\devel\repo\.git"),
    });
    assert!(
        xml.contains("<LogonTrigger><Enabled>true</Enabled><UserId>S-1-5-21-1-2-3-1001</UserId>")
    );
    assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"));
    assert!(xml.contains("<RunLevel>LeastPrivilege</RunLevel>"));
    assert!(xml.contains("<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>"));
    assert!(xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
    assert!(xml.contains("<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>"));
    assert!(xml.contains(r"<Command>C:\Program Files\Spice &amp; Co\spice.exe</Command>"));
    assert!(xml.contains("<Arguments>--json up --keeper</Arguments>"));
    assert!(xml.contains(r"<WorkingDirectory>C:\Users\c\devel\repo</WorkingDirectory>"));
    assert!(
        !xml.contains("Password"),
        "no credential is ever registered"
    );
}

#[test]
fn task_paths_are_stable_per_repository_and_distinct_between_them() {
    let one = definition::task_path(Path::new(r"C:\Users\c\devel\one\.git"));
    let other = definition::task_path(Path::new(r"C:\Users\c\devel\other\.git"));
    assert_eq!(
        one,
        definition::task_path(Path::new(r"C:\Users\c\devel\one\.git"))
    );
    assert_ne!(one, other);
    assert!(one.starts_with(&format!("{}\\keeper-", definition::FOLDER)));
}

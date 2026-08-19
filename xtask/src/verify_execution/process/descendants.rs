//! Process-tree cleanup for verifier actions whose descendants create new groups.

use std::collections::BTreeSet;
use std::io;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

const REAP_POLL: Duration = Duration::from_millis(10);

#[derive(Clone, Debug, Eq, PartialEq)]
struct Process {
    pid: u32,
    parent: u32,
    group: u32,
    state: char,
    started: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ProcessIdentity {
    pid: u32,
    group: u32,
    started: String,
}

impl From<&Process> for ProcessIdentity {
    fn from(process: &Process) -> Self {
        Self {
            pid: process.pid,
            group: process.group,
            started: process.started.clone(),
        }
    }
}

#[derive(Debug)]
pub(super) struct DescendantGroups {
    root: u32,
    groups: BTreeSet<u32>,
    members: BTreeSet<ProcessIdentity>,
}

impl DescendantGroups {
    pub(super) fn discover(root: u32) -> io::Result<Self> {
        let processes = process_snapshot()?;
        let groups = descendant_groups(&processes, root);
        Ok(Self {
            root,
            members: processes
                .iter()
                .filter(|process| groups.contains(&process.group))
                .map(ProcessIdentity::from)
                .collect(),
            groups,
        })
    }

    pub(super) fn terminate(&self) -> io::Result<()> {
        self.signal(libc::SIGTERM)
    }

    pub(super) fn reap(&self, grace: Duration, reap: Duration) -> io::Result<()> {
        if self.groups.is_empty() || self.wait_until_terminal(grace)? {
            return Ok(());
        }
        self.signal(libc::SIGKILL)?;
        if self.wait_until_terminal(reap)? {
            return Ok(());
        }
        let live = owned_live_groups(&process_snapshot()?, self.root, &self.groups, &self.members)?;
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "process groups {} remained live after the {reap:?} reap deadline",
                display_groups(&live)
            ),
        ))
    }

    fn signal(&self, signal: libc::c_int) -> io::Result<()> {
        signal_owned_groups(
            self.root,
            &self.groups,
            &self.members,
            process_snapshot,
            kill_process_group,
            signal,
        )
    }

    fn wait_until_terminal(&self, deadline: Duration) -> io::Result<bool> {
        let started = Instant::now();
        loop {
            if owned_live_groups(&process_snapshot()?, self.root, &self.groups, &self.members)?
                .is_empty()
            {
                return Ok(true);
            }
            if started.elapsed() >= deadline {
                return Ok(false);
            }
            thread::sleep(REAP_POLL.min(deadline.saturating_sub(started.elapsed())));
        }
    }
}

fn process_snapshot() -> io::Result<Vec<Process>> {
    let output = Command::new("ps")
        .args(["-Ao", "pid=,ppid=,pgid=,stat=,lstart="])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "ps exited {:?} while discovering verifier descendants",
            output.status.code()
        )));
    }
    let listing = String::from_utf8(output.stdout).map_err(io::Error::other)?;
    listing
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(parse_process)
        .collect()
}

fn parse_process(line: &str) -> io::Result<Process> {
    let mut fields = line.split_whitespace();
    let pid = parse_pid(fields.next(), "PID", line, false)?;
    let parent = parse_pid(fields.next(), "parent PID", line, true)?;
    let group = parse_pid(fields.next(), "process-group ID", line, false)?;
    let state = fields
        .next()
        .and_then(|state| state.chars().next())
        .ok_or_else(|| malformed_listing("state", line))?;
    let started = fields.collect::<Vec<_>>();
    if started.len() != 5 {
        return Err(malformed_listing("five-field start time", line));
    }
    Ok(Process {
        pid,
        parent,
        group,
        state,
        started: started.join(" "),
    })
}

fn parse_pid(field: Option<&str>, name: &str, line: &str, zero_allowed: bool) -> io::Result<u32> {
    field
        .and_then(|value| value.parse().ok())
        .filter(|value| zero_allowed || *value > 0)
        .ok_or_else(|| malformed_listing(name, line))
}

fn malformed_listing(expected: &str, line: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("ps row {line:?} has no valid {expected}"),
    )
}

fn descendant_groups(processes: &[Process], root: u32) -> BTreeSet<u32> {
    let mut descendants = BTreeSet::from([root]);
    loop {
        let before = descendants.len();
        for process in processes {
            if descendants.contains(&process.parent) {
                descendants.insert(process.pid);
            }
        }
        if descendants.len() == before {
            break;
        }
    }
    processes
        .iter()
        .filter(|process| process.pid != root && descendants.contains(&process.pid))
        .map(|process| process.group)
        .filter(|group| *group != root)
        .collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GroupOwnership {
    Absent,
    Owned,
    Drifted,
}

fn group_ownership(
    processes: &[Process],
    root: u32,
    group: u32,
    members: &BTreeSet<ProcessIdentity>,
) -> GroupOwnership {
    let live = processes
        .iter()
        .filter(|process| process.group == group && process.state != 'Z')
        .collect::<Vec<_>>();
    if live.is_empty() {
        return GroupOwnership::Absent;
    }
    if live
        .iter()
        .any(|process| members.contains(&ProcessIdentity::from(*process)))
        || descendant_groups(processes, root).contains(&group)
    {
        GroupOwnership::Owned
    } else {
        GroupOwnership::Drifted
    }
}

fn owned_live_groups(
    processes: &[Process],
    root: u32,
    groups: &BTreeSet<u32>,
    members: &BTreeSet<ProcessIdentity>,
) -> io::Result<BTreeSet<u32>> {
    let mut live = BTreeSet::new();
    for group in groups {
        match group_ownership(processes, root, *group, members) {
            GroupOwnership::Absent => {}
            GroupOwnership::Owned => {
                live.insert(*group);
            }
            GroupOwnership::Drifted => return Err(ownership_drift(root, *group)),
        }
    }
    Ok(live)
}

fn signal_owned_groups<Snapshot, Signal>(
    root: u32,
    groups: &BTreeSet<u32>,
    members: &BTreeSet<ProcessIdentity>,
    mut snapshot: Snapshot,
    mut signal_group: Signal,
    signal: libc::c_int,
) -> io::Result<()>
where
    Snapshot: FnMut() -> io::Result<Vec<Process>>,
    Signal: FnMut(u32, libc::c_int) -> io::Result<()>,
{
    let mut failures = Vec::new();
    for group in groups {
        match group_ownership(&snapshot()?, root, *group, members) {
            GroupOwnership::Absent => continue,
            GroupOwnership::Drifted => return Err(ownership_drift(root, *group)),
            GroupOwnership::Owned => {}
        }
        if let Err(error) = signal_group(*group, signal) {
            match error.raw_os_error() {
                Some(libc::ESRCH) => {}
                Some(libc::EPERM) => match group_ownership(&snapshot()?, root, *group, members) {
                    GroupOwnership::Absent => {}
                    GroupOwnership::Drifted => return Err(ownership_drift(root, *group)),
                    GroupOwnership::Owned => failures.push(format!("{group}: {error}")),
                },
                _ => failures.push(format!("{group}: {error}")),
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "signal {signal} failed for {}",
            failures.join(", ")
        )))
    }
}

fn kill_process_group(group: u32, signal: libc::c_int) -> io::Result<()> {
    let group = libc::pid_t::try_from(group)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "group identity exceeds pid_t"))?;
    // SAFETY: the caller immediately revalidated this owned process group.
    if unsafe { libc::kill(-group, signal) } == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn ownership_drift(root: u32, group: u32) -> io::Error {
    io::Error::other(format!(
        "process group {group} is live but no longer descends from owned root {root}"
    ))
}

fn display_groups(groups: &BTreeSet<u32>) -> String {
    groups
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descendant_groups_follow_parentage_across_process_group_boundaries() {
        let processes = [
            process(10, 1, 10),
            process(11, 10, 10),
            process(12, 11, 12),
            process(13, 12, 13),
            process(14, 99, 14),
        ];

        assert_eq!(descendant_groups(&processes, 10), BTreeSet::from([12, 13]));
    }

    #[test]
    fn malformed_process_rows_refuse_discovery() {
        let error = parse_process("12 no-parent 12 S").expect_err("malformed parent refuses");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("parent PID"), "{error}");
    }

    #[test]
    fn process_rows_capture_the_complete_start_generation() {
        let process = parse_process("12 10 12 S Tue Aug 18 12:34:56 2026")
            .expect("complete process row parses");

        assert_eq!(process.pid, 12);
        assert_eq!(process.parent, 10);
        assert_eq!(process.group, 12);
        assert_eq!(process.state, 'S');
        assert_eq!(process.started, "Tue Aug 18 12:34:56 2026");
    }

    #[test]
    fn ownership_drift_refuses_before_a_foreign_group_is_signalled() {
        let groups = BTreeSet::from([12]);
        let owned = [process_with_start(12, 10, 12, "owned-start")];
        let members = owned.iter().map(ProcessIdentity::from).collect();
        let foreign = vec![process_with_start(12, 99, 12, "foreign-start")];
        let mut signals = Vec::new();

        let error = signal_owned_groups(
            10,
            &groups,
            &members,
            || Ok(foreign.clone()),
            |group, signal| {
                signals.push((group, signal));
                Ok(())
            },
            libc::SIGTERM,
        )
        .expect_err("foreign ownership refuses");

        assert!(signals.is_empty(), "foreign group received a signal");
        assert!(error.to_string().contains("no longer descends"), "{error}");
    }

    #[test]
    fn captured_generation_remains_owned_after_the_root_exits() {
        let groups = BTreeSet::from([12]);
        let detached = vec![process_with_start(12, 1, 12, "owned-start")];
        let members = detached.iter().map(ProcessIdentity::from).collect();
        let mut signals = Vec::new();

        signal_owned_groups(
            10,
            &groups,
            &members,
            || Ok(detached.clone()),
            |group, signal| {
                signals.push((group, signal));
                Ok(())
            },
            libc::SIGTERM,
        )
        .expect("captured detached generation remains owned");

        assert_eq!(signals, [(12, libc::SIGTERM)]);
    }

    #[test]
    fn permission_refusal_rechecks_ownership_without_retrying_a_recycled_group() {
        let groups = BTreeSet::from([12]);
        let owned = vec![
            process(10, 1, 10),
            process_with_start(12, 10, 12, "owned-start"),
        ];
        let members = owned.iter().map(ProcessIdentity::from).collect();
        let foreign = vec![process_with_start(12, 99, 12, "foreign-start")];
        let mut snapshots = vec![foreign, owned].into_iter().rev();
        let mut signals = 0;

        let error = signal_owned_groups(
            10,
            &groups,
            &members,
            || Ok(snapshots.next().expect("two ownership snapshots")),
            |_, _| {
                signals += 1;
                Err(io::Error::from_raw_os_error(libc::EPERM))
            },
            libc::SIGTERM,
        )
        .expect_err("recycled group is a typed ownership drift");

        assert_eq!(signals, 1, "permission refusal must not be retried");
        assert!(error.to_string().contains("no longer descends"), "{error}");
    }

    #[test]
    fn permission_refusal_is_benign_when_the_owned_group_is_then_absent() {
        let groups = BTreeSet::from([12]);
        let owned = vec![
            process(10, 1, 10),
            process_with_start(12, 10, 12, "owned-start"),
        ];
        let members = owned.iter().map(ProcessIdentity::from).collect();
        let mut snapshots = vec![Vec::new(), owned].into_iter().rev();
        let mut signals = 0;

        signal_owned_groups(
            10,
            &groups,
            &members,
            || Ok(snapshots.next().expect("two ownership snapshots")),
            |_, _| {
                signals += 1;
                Err(io::Error::from_raw_os_error(libc::EPERM))
            },
            libc::SIGTERM,
        )
        .expect("an exited owned group makes the stale permission refusal benign");

        assert_eq!(signals, 1, "permission refusal must not be retried");
    }

    fn process(pid: u32, parent: u32, group: u32) -> Process {
        process_with_start(pid, parent, group, &format!("fixture-start-{pid}"))
    }

    fn process_with_start(pid: u32, parent: u32, group: u32, started: &str) -> Process {
        Process {
            pid,
            parent,
            group,
            state: 'S',
            started: started.to_owned(),
        }
    }
}

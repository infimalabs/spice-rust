//! Process-tree cleanup for verifier actions whose descendants create new groups.

use std::collections::BTreeSet;
use std::io;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

const REAP_POLL: Duration = Duration::from_millis(10);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Process {
    pid: u32,
    parent: u32,
    group: u32,
    state: char,
}

#[derive(Debug)]
pub(super) struct DescendantGroups {
    groups: BTreeSet<u32>,
}

impl DescendantGroups {
    pub(super) fn discover(root: u32) -> io::Result<Self> {
        let processes = process_snapshot()?;
        Ok(Self {
            groups: descendant_groups(&processes, root),
        })
    }

    pub(super) fn terminate(&self) -> io::Result<()> {
        signal_groups(&self.groups, libc::SIGTERM)
    }

    pub(super) fn reap(&self, grace: Duration, reap: Duration) -> io::Result<()> {
        if self.groups.is_empty() || wait_until_terminal(&self.groups, grace)? {
            return Ok(());
        }
        signal_groups(&self.groups, libc::SIGKILL)?;
        if wait_until_terminal(&self.groups, reap)? {
            return Ok(());
        }
        let live = live_groups(&self.groups)?;
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "process groups {} remained live after the {reap:?} reap deadline",
                display_groups(&live)
            ),
        ))
    }
}

fn process_snapshot() -> io::Result<Vec<Process>> {
    let output = Command::new("ps")
        .args(["-Ao", "pid=,ppid=,pgid=,stat="])
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
    if fields.next().is_some() {
        return Err(malformed_listing("exactly four fields", line));
    }
    Ok(Process {
        pid,
        parent,
        group,
        state,
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

fn signal_groups(groups: &BTreeSet<u32>, signal: libc::c_int) -> io::Result<()> {
    let mut failures = Vec::new();
    for group in groups {
        let Ok(group) = libc::pid_t::try_from(*group) else {
            failures.push(format!("{group}: identity exceeds pid_t"));
            continue;
        };
        // SAFETY: the negative identity targets exactly the observed process group.
        if unsafe { libc::kill(-group, signal) } == -1 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                failures.push(format!("{group}: {error}"));
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

fn wait_until_terminal(groups: &BTreeSet<u32>, deadline: Duration) -> io::Result<bool> {
    let started = Instant::now();
    loop {
        if live_groups(groups)?.is_empty() {
            return Ok(true);
        }
        if started.elapsed() >= deadline {
            return Ok(false);
        }
        thread::sleep(REAP_POLL.min(deadline.saturating_sub(started.elapsed())));
    }
}

fn live_groups(groups: &BTreeSet<u32>) -> io::Result<BTreeSet<u32>> {
    Ok(process_snapshot()?
        .into_iter()
        .filter(|process| process.state != 'Z' && groups.contains(&process.group))
        .map(|process| process.group)
        .collect())
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

    const fn process(pid: u32, parent: u32, group: u32) -> Process {
        Process {
            pid,
            parent,
            group,
            state: 'S',
        }
    }
}

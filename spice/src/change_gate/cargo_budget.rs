//! One fleet-aware Cargo budget shared by focused policy and selected tests.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::thread;

use sill::Environment;

use super::ChangeGateError;

const CARGO_BUILD_JOBS: &str = "CARGO_BUILD_JOBS";
const MAX_TEST_HARNESS_THREADS: usize = 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CargoBudget {
    cpus: usize,
    peer_cargo_roots: Option<usize>,
    lane_jobs: usize,
    test_concurrency: usize,
    shard_jobs: usize,
    test_threads: usize,
}

impl CargoBudget {
    pub(super) fn detect(test_concurrency_cap: usize) -> Self {
        let cpus = thread::available_parallelism().map_or(1, usize::from);
        let peers = active_peer_cargo_roots().ok();
        Self::from_observation(cpus, peers, test_concurrency_cap)
    }

    pub(super) fn from_observation(
        cpus: usize,
        peer_cargo_roots: Option<usize>,
        test_concurrency_cap: usize,
    ) -> Self {
        let cpus = cpus.max(1);
        let lane_jobs = peer_cargo_roots
            .map(|peers| cpus / peers.saturating_add(1))
            .unwrap_or(1)
            .max(1);
        let test_concurrency = lane_jobs.min(test_concurrency_cap.max(1));
        let shard_jobs = (lane_jobs / test_concurrency).max(1);
        let test_threads = lane_jobs
            .div_ceil(test_concurrency)
            .clamp(1, MAX_TEST_HARNESS_THREADS);
        Self {
            cpus,
            peer_cargo_roots,
            lane_jobs,
            test_concurrency,
            shard_jobs,
            test_threads,
        }
    }

    pub(super) const fn policy_jobs(self) -> usize {
        self.lane_jobs
    }

    pub(super) const fn test_concurrency(self) -> usize {
        self.test_concurrency
    }

    pub(super) const fn shard_jobs(self) -> usize {
        self.shard_jobs
    }

    pub(super) const fn test_threads(self) -> usize {
        self.test_threads
    }

    pub(super) fn peer_cargo_roots(self) -> String {
        self.peer_cargo_roots
            .map_or_else(|| "unknown".to_owned(), |peers| peers.to_string())
    }

    pub(super) const fn cpus(self) -> usize {
        self.cpus
    }

    pub(super) fn bind_test_environment(
        self,
        environment: Environment,
    ) -> Result<Environment, ChangeGateError> {
        let mut entries = environment
            .entries()
            .iter()
            .cloned()
            .collect::<BTreeMap<OsString, OsString>>();
        entries.insert(
            OsString::from(CARGO_BUILD_JOBS),
            OsString::from(self.shard_jobs.to_string()),
        );
        Environment::new(entries).map_err(|error| {
            ChangeGateError::environment("change-gate-environment-unavailable", error)
        })
    }
}

fn active_peer_cargo_roots() -> io::Result<usize> {
    let output = Command::new("ps")
        .args(["-Ao", "pid=,ppid=,comm="])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "ps exited {:?} while counting peer Cargo roots",
            output.status.code()
        )));
    }
    let listing = String::from_utf8(output.stdout).map_err(io::Error::other)?;
    Ok(count_peer_cargo_roots(&listing, std::process::id()))
}

fn count_peer_cargo_roots(listing: &str, current: u32) -> usize {
    let processes = listing
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some((
                fields.next()?.parse::<u32>().ok()?,
                fields.next()?.parse::<u32>().ok()?,
                fields.next()?.to_owned(),
            ))
        })
        .collect::<Vec<_>>();
    let parents = processes
        .iter()
        .map(|(pid, parent, _)| (*pid, *parent))
        .collect::<BTreeMap<_, _>>();
    let mut ancestors = BTreeSet::new();
    let mut cursor = current;
    while let Some(parent) = parents.get(&cursor).copied() {
        if parent == 0 || !ancestors.insert(parent) {
            break;
        }
        cursor = parent;
    }
    processes
        .iter()
        .filter(|(pid, _, command)| {
            !ancestors.contains(pid)
                && !descends_from(*pid, current, &parents)
                && PathBuf::from(command)
                    .file_name()
                    .is_some_and(|name| name == OsStr::new("cargo"))
        })
        .count()
}

fn descends_from(process: u32, ancestor: u32, parents: &BTreeMap<u32, u32>) -> bool {
    let mut cursor = process;
    let mut visited = BTreeSet::new();
    while let Some(parent) = parents.get(&cursor).copied() {
        if parent == ancestor {
            return true;
        }
        if parent == 0 || !visited.insert(parent) {
            return false;
        }
        cursor = parent;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_divides_one_lane_across_peer_pressure() {
        let idle = CargoBudget::from_observation(12, Some(0), 4);
        assert_eq!(idle.policy_jobs(), 12);
        assert_eq!(idle.test_concurrency(), 4);
        assert_eq!(idle.shard_jobs(), 3);
        assert_eq!(idle.test_threads(), 3);

        let balanced = CargoBudget::from_observation(12, Some(1), 4);
        assert_eq!(balanced.policy_jobs(), 6);
        assert_eq!(balanced.test_concurrency(), 4);
        assert_eq!(balanced.shard_jobs(), 1);
        assert_eq!(balanced.test_threads(), 2);

        let pressured = CargoBudget::from_observation(12, Some(5), 4);
        assert_eq!(pressured.policy_jobs(), 2);
        assert_eq!(pressured.test_concurrency(), 2);
        assert_eq!(pressured.shard_jobs(), 1);
        assert_eq!(pressured.test_threads(), 1);

        let unknown = CargoBudget::from_observation(12, None, 4);
        assert_eq!(unknown.policy_jobs(), 1);
        assert_eq!(unknown.test_concurrency(), 1);
        assert_eq!(unknown.shard_jobs(), 1);
        assert_eq!(unknown.test_threads(), 1);
    }

    #[test]
    fn peer_probe_excludes_the_current_tree_and_its_cargo_ancestor() {
        let listing = concat!(
            "10 1 cargo\n",
            "20 10 spice\n",
            "21 20 cargo\n",
            "22 21 rustc\n",
            "30 1 /toolchain/bin/cargo\n",
            "31 30 rustc\n",
            "40 1 cargo\n",
        );
        assert_eq!(count_peer_cargo_roots(listing, 20), 2);
    }

    #[test]
    fn test_environment_replaces_an_unbounded_job_count() {
        let environment = Environment::new([
            (OsString::from("PATH"), OsString::from("/bin")),
            (OsString::from(CARGO_BUILD_JOBS), OsString::from("99")),
        ])
        .expect("fixture environment");
        let budget = CargoBudget::from_observation(8, Some(3), 4);

        let bounded = budget
            .bind_test_environment(environment)
            .expect("bind test budget");
        assert_eq!(
            bounded
                .entries()
                .iter()
                .find(|(name, _)| name == OsStr::new(CARGO_BUILD_JOBS))
                .map(|(_, value)| value),
            Some(&OsString::from("1"))
        );
    }
}

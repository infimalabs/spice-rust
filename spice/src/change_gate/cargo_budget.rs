//! One slice-local Cargo budget shared by focused policy and selected tests.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::thread;

use sill::Environment;

use super::ChangeGateError;

const CARGO_BUILD_JOBS: &str = "CARGO_BUILD_JOBS";
const CONCURRENT_ORDINARY_GATE_SLICES: usize = 4;
const TEST_HARNESS_THREADS_PER_SHARD: usize = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CargoBudget {
    cpus: usize,
    slice_jobs: usize,
    policy_concurrency: usize,
    test_concurrency: usize,
    shard_jobs: usize,
    test_threads: usize,
}

impl CargoBudget {
    pub(super) fn detect(test_concurrency_cap: usize) -> Self {
        let cpus = thread::available_parallelism().map_or(1, usize::from);
        Self::from_capacity(cpus, test_concurrency_cap)
    }

    pub(super) fn from_capacity(cpus: usize, test_concurrency_cap: usize) -> Self {
        let cpus = cpus.max(1);
        let slice_jobs = cpus;
        let test_concurrency = slice_jobs
            .div_ceil(CONCURRENT_ORDINARY_GATE_SLICES)
            .min(test_concurrency_cap.max(1));
        let policy_concurrency = test_concurrency;
        let shard_jobs = (slice_jobs / test_concurrency).max(1);
        Self {
            cpus,
            slice_jobs,
            policy_concurrency,
            test_concurrency,
            shard_jobs,
            test_threads: TEST_HARNESS_THREADS_PER_SHARD,
        }
    }

    pub(super) const fn policy_jobs(self) -> usize {
        self.slice_jobs
    }

    pub(super) const fn policy_concurrency(self) -> usize {
        self.policy_concurrency
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

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    #[test]
    fn budget_reserves_four_ordinary_gate_slices_and_single_threads_each_shard() {
        let budget = CargoBudget::from_capacity(12, 4);
        assert_eq!(budget.policy_jobs(), 12);
        assert_eq!(budget.policy_concurrency(), 3);
        assert_eq!(budget.test_concurrency(), 3);
        assert_eq!(budget.shard_jobs(), 4);
        assert_eq!(budget.test_threads(), 1);

        let minimum = CargoBudget::from_capacity(0, 4);
        assert_eq!(minimum.policy_jobs(), 1);
        assert_eq!(minimum.policy_concurrency(), 1);
        assert_eq!(minimum.test_concurrency(), 1);
        assert_eq!(minimum.shard_jobs(), 1);
        assert_eq!(minimum.test_threads(), 1);
    }

    #[test]
    fn test_environment_replaces_an_unbounded_job_count() {
        let environment = Environment::new([
            (OsString::from("PATH"), OsString::from("/bin")),
            (OsString::from(CARGO_BUILD_JOBS), OsString::from("99")),
        ])
        .expect("fixture environment");
        let budget = CargoBudget::from_capacity(8, 4);

        let bounded = budget
            .bind_test_environment(environment)
            .expect("bind test budget");
        assert_eq!(
            bounded
                .entries()
                .iter()
                .find(|(name, _)| name == OsStr::new(CARGO_BUILD_JOBS))
                .map(|(_, value)| value),
            Some(&OsString::from("4"))
        );
    }
}

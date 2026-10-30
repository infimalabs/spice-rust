//! The installed Windows lifecycle owner of one repository's resident.
//!
//! Windows has no per-user service manager that runs a program as the
//! logged-on user without a stored password, and a service under another
//! identity would own the discovery file that user's app and CLI must read.
//! The owner is therefore a per-user Task Scheduler task: triggered by the
//! repository user's own logon, logging on with that user's interactive
//! token, and running this executable as `spice up --keeper`. The keeper
//! holds one possession per repository and relaunches the resident after
//! every exit, clean or not, until `spice down` disables the task or
//! `spice up --uninstall` deletes it.
//!
//! Everything that decides is cross-platform and exercised in process; only
//! the Task Scheduler binding and the process-level entry points are Windows.

#[cfg(any(windows, test))]
mod definition;
#[cfg(any(windows, test))]
mod owner;
#[cfg(any(windows, test))]
mod policy;
#[cfg(any(windows, test))]
mod record;
#[cfg(any(windows, test))]
mod scheduler;
#[cfg(windows)]
mod windows;

#[cfg(test)]
#[path = "keeper/tests.rs"]
mod tests;

#[cfg(windows)]
pub(crate) use windows::{hold_stopped, install, keep, uninstall, up_detached};

#[cfg(not(windows))]
pub(crate) use unsupported::{hold_stopped, install, keep, uninstall, up_detached};

/// Hosts without the facility refuse the owner by name and keep the plain
/// detached launch.
#[cfg(not(windows))]
mod unsupported {
    use super::super::*;

    pub(crate) fn install(_leaf: &clap::ArgMatches, _ambient: &Ambient) -> Outcome<wire::AppOpen> {
        refusal()
    }

    pub(crate) fn uninstall(_ambient: &Ambient) -> Outcome<wire::KeeperRemoved> {
        refusal()
    }

    pub(crate) fn keep(_ambient: &Ambient) -> Outcome<()> {
        refusal()
    }

    pub(crate) fn up_detached(
        leaf: &clap::ArgMatches,
        ambient: &Ambient,
    ) -> Outcome<wire::AppOpen> {
        launch(leaf, None, leaf.get_flag("open"), ambient)
    }

    pub(crate) fn hold_stopped(_repository: &Repository) -> CommandResult<()> {
        Ok(())
    }

    fn refusal<B>() -> Outcome<B> {
        Outcome::Refusal(Refusal::bare(
            wire::RefusalDiagnostic::new(
                "resident::keeper-unsupported".to_owned(),
                Some("target-os".to_owned()),
                Some("windows".to_owned()),
                Some(std::env::consts::OS.to_owned()),
            )
            .with_detail(
                "the lifecycle owner is a per-user Task Scheduler registration; \
                 this host keeps residents through its own service manager",
            ),
        ))
    }
}

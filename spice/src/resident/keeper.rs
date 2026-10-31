//! The installed Windows lifecycle owner of one repository's resident.
//!
//! Windows has no per-user service manager that runs a program as the
//! logged-on user without a stored password, and a service under another
//! identity would own the discovery file that user's app and CLI must read.
//! The owner is therefore a per-user Task Scheduler task: triggered by the
//! repository user's own logon, logging on with that user's interactive
//! token, and running this executable through its private keeper-worker entry
//! point. The keeper holds one possession per repository and relaunches the
//! resident after every exit, clean or not, until `spice down` disables the
//! task or `spice down --keeper` deletes it.
//!
//! Everything that decides is cross-platform and exercised in process; only
//! the Task Scheduler binding and the process-level entry points are Windows.

/// Exact private argument used only by the native lifecycle registration.
pub(crate) const WORKER_ARGUMENT: &str = "--spice-keeper-worker";

/// Whether this process invocation is exactly the private keeper worker.
pub(crate) fn worker_selected(mut arguments: impl Iterator<Item = std::ffi::OsString>) -> bool {
    let _program = arguments.next();
    arguments
        .next()
        .is_some_and(|argument| argument == WORKER_ARGUMENT)
        && arguments.next().is_none()
}

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
pub(crate) use windows::{
    finish_unapply, hold_stopped, install, install_standing, keep, owner_installed,
    quiesce_for_install, uninstall, up_detached,
};

#[cfg(not(windows))]
pub(crate) use unsupported::{
    finish_unapply, hold_stopped, install, install_standing, keep, owner_installed,
    quiesce_for_install, uninstall, up_detached,
};

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

    pub(crate) fn install_standing(
        _repository: &Repository,
    ) -> CommandResult<Option<spice::install::LifecycleOwnerStanding>> {
        Ok(None)
    }

    pub(crate) fn owner_installed(_repository: &Repository) -> CommandResult<bool> {
        Ok(false)
    }
    pub(crate) fn finish_unapply(_repository: &Repository) -> CommandResult<()> {
        Ok(())
    }

    pub(crate) fn quiesce_for_install(
        _repository: &Repository,
        _expected: &spice::install::LifecycleOwnerStanding,
        _ambient: &Ambient,
    ) -> CommandResult<bool> {
        Ok(false)
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

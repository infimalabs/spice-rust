use std::io;

#[cfg(any(target_os = "linux", test))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(test)]
mod tests;

/// What a probe actually managed to observe about a process or process group.
///
/// A bare `bool` collapses two unrelated answers into `false`: the kernel was
/// read and something is still running, and the kernel would not answer at all.
/// Callers cannot tell those apart, so an unreadable probe is indistinguishable
/// from a positively observed live member, and the faults that follow move with
/// machine load rather than with the code under test. Naming the third answer is
/// what stops an unread kernel from impersonating a live one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProcessState {
    /// The kernel was read, and the process or group has ended.
    Terminal,
    /// The kernel was read, and something is still running.
    Live,
    /// The kernel could not be read, so neither answer was observed.
    Indeterminate,
}

impl ProcessState {
    /// Collapse to the single bit today's consumers still take.
    ///
    /// Only [`ProcessState::Terminal`] is an observed end. `Live` and
    /// `Indeterminate` both answer "not observed terminal", which is exactly
    /// what the bare `bool` probes returned before the third answer had a name,
    /// so adopting the enum changes no consumer's behavior. Deciding what each
    /// consumer should instead do when the answer is `Indeterminate` is
    /// SPAWN-1kKYfJw2; keeping the collapse in one named place is what gives
    /// that task a single edit rather than a hunt through the call sites.
    pub(crate) fn observed_terminal(self) -> bool {
        matches!(self, Self::Terminal)
    }
}

pub(super) fn process_absent(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    kernel_identity_absent(pid)
}

pub(crate) fn process_terminal(pid: u32) -> bool {
    process_state(pid).observed_terminal()
}

/// Answer for one process, settling an unreadable platform probe against the
/// kernel's own identity table.
///
/// The platform probe reads a status record, which is the only thing that can
/// distinguish a zombie from a running process. When that record cannot be read
/// the identity probe can still prove the process is gone, and proving absence
/// is a real observation rather than a guess. Only when both fail to establish
/// anything does the answer stay [`ProcessState::Indeterminate`].
pub(super) fn process_state(pid: u32) -> ProcessState {
    match process_zombie(pid) {
        ProcessState::Terminal => ProcessState::Terminal,
        _ if process_absent(pid) => ProcessState::Terminal,
        observed => observed,
    }
}

pub(super) fn process_zombie(pid: u32) -> ProcessState {
    platform::process_zombie(pid)
}

pub(crate) fn process_group_terminal(pid: u32) -> bool {
    process_group_absent(pid) || platform::process_group_members_terminal(pid).observed_terminal()
}

pub(super) fn process_group_absent(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    kernel_identity_absent(-pid)
}

/// Fold the members' individual answers into one answer for the group.
///
/// A single live member is a positive observation and settles the group
/// immediately. An unreadable member settles nothing, so it lowers the group to
/// [`ProcessState::Indeterminate`] only once every member has been consulted and
/// none was observed live. A scan that found no members at all observed nothing,
/// so it is settled by whether the kernel still knows the group identity.
pub(super) fn membership(
    members: impl Iterator<Item = ProcessState>,
    group_absent: impl FnOnce() -> bool,
) -> ProcessState {
    let mut observed = false;
    let mut unreadable = false;
    for member in members {
        observed = true;
        match member {
            ProcessState::Live => return ProcessState::Live,
            ProcessState::Indeterminate => unreadable = true,
            ProcessState::Terminal => {}
        }
    }
    if unreadable {
        return ProcessState::Indeterminate;
    }
    if observed || group_absent() {
        ProcessState::Terminal
    } else {
        ProcessState::Indeterminate
    }
}

pub(super) fn kernel_identity_absent(pid: libc::pid_t) -> bool {
    // SAFETY: signal zero probes identity existence without delivering a signal.
    unsafe {
        libc::kill(pid, 0) == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
}

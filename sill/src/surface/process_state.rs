use std::io;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;

pub(super) fn process_absent(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    kernel_identity_absent(pid)
}

pub(crate) fn process_terminal(pid: u32) -> bool {
    process_zombie(pid) || process_absent(pid)
}

pub(super) fn process_zombie(pid: u32) -> bool {
    platform::process_zombie(pid)
}

pub(crate) fn process_group_terminal(pid: u32) -> bool {
    process_group_absent(pid) || platform::process_group_members_terminal(pid)
}

pub(super) fn process_group_absent(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    kernel_identity_absent(-pid)
}

pub(super) fn kernel_identity_absent(pid: libc::pid_t) -> bool {
    // SAFETY: signal zero probes identity existence without delivering a signal.
    unsafe {
        libc::kill(pid, 0) == -1 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
    }
}

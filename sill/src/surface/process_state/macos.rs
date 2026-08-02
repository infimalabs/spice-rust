use std::io;
use std::mem::{MaybeUninit, size_of, size_of_val};

use super::{kernel_identity_absent, process_terminal};

pub(super) fn process_zombie(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    let mut information = MaybeUninit::<libc::proc_bsdshortinfo>::uninit();
    let Ok(size) = libc::c_int::try_from(size_of::<libc::proc_bsdshortinfo>()) else {
        return false;
    };
    // SAFETY: the process information destination is writable for its complete size.
    let bytes = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDT_SHORTBSDINFO,
            0,
            information.as_mut_ptr().cast(),
            size,
        )
    };
    if bytes == 0 {
        return io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
    }
    if bytes != size {
        return false;
    }
    // SAFETY: an exact-size result initialized the process information.
    unsafe { information.assume_init() }.pbsi_status == libc::SZOMB
}

pub(super) fn process_group_members_terminal(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: a null buffer asks libproc for the current member count.
    let required = unsafe { libc::proc_listpgrppids(pid, std::ptr::null_mut(), 0) };
    let Ok(required) = usize::try_from(required) else {
        return false;
    };
    let Some(capacity) = required.checked_add(1) else {
        return false;
    };
    let mut members = vec![0 as libc::pid_t; capacity];
    let Ok(bytes) = libc::c_int::try_from(size_of_val(members.as_slice())) else {
        return false;
    };
    // SAFETY: the PID buffer is writable for its complete byte capacity.
    let found = unsafe { libc::proc_listpgrppids(pid, members.as_mut_ptr().cast(), bytes) };
    let Ok(found) = usize::try_from(found) else {
        return false;
    };
    if found >= capacity {
        return false;
    }
    let mut observed = false;
    for member in members.into_iter().take(found).filter(|member| *member > 0) {
        observed = true;
        if !process_terminal(member as u32) {
            return false;
        }
    }
    observed || kernel_identity_absent(-pid)
}

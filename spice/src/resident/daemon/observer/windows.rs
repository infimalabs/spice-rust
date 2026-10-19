//! Windows observers: a change notification on the watched directory and a
//! synchronize handle on the watched process.

use std::io;
use std::os::windows::ffi::OsStrExt as _;
use std::path::Path;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, HANDLE, INVALID_HANDLE_VALUE,
    WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_NOTIFY_CHANGE_DIR_NAME, FILE_NOTIFY_CHANGE_FILE_NAME, FILE_NOTIFY_CHANGE_LAST_WRITE,
    FindCloseChangeNotification, FindFirstChangeNotificationW,
};
use windows_sys::Win32::System::Threading::{
    INFINITE, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

pub(crate) struct DirectObserver {
    notification: HANDLE,
}

// SAFETY: a change-notification handle may be waited on and closed from any
// thread.
unsafe impl Send for DirectObserver {}

impl DirectObserver {
    /// Arm one notification for entries created, removed, renamed, or
    /// rewritten directly in `watched`.
    pub(crate) fn arm(watched: &Path) -> io::Result<Self> {
        let path: Vec<u16> = watched
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: `path` is NUL-terminated for the call.
        let notification = unsafe {
            FindFirstChangeNotificationW(
                path.as_ptr(),
                0,
                FILE_NOTIFY_CHANGE_FILE_NAME
                    | FILE_NOTIFY_CHANGE_DIR_NAME
                    | FILE_NOTIFY_CHANGE_LAST_WRITE,
            )
        };
        if notification == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { notification })
    }

    /// Block until the watched directory changes once.
    ///
    /// # Errors
    ///
    /// Returns the kernel's refusal to deliver the change.
    pub(crate) fn wait(self) -> io::Result<()> {
        signaled(self.notification)
    }
}

impl Drop for DirectObserver {
    fn drop(&mut self) {
        // SAFETY: this observer exclusively owns the notification handle.
        unsafe { FindCloseChangeNotification(self.notification) };
    }
}

pub(super) enum ProcessExitObserver {
    Watching(HANDLE),
    Ended,
}

// SAFETY: a process handle may be waited on and closed from any thread.
unsafe impl Send for ProcessExitObserver {}

impl ProcessExitObserver {
    pub(super) fn or_ended(pid: u32) -> io::Result<Self> {
        // SAFETY: synchronize access only observes the named process.
        let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if !process.is_null() {
            return Ok(Self::Watching(process));
        }
        let error = io::Error::last_os_error();
        // Windows names a process that no longer exists an invalid parameter.
        if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) {
            return Ok(Self::Ended);
        }
        Err(error)
    }

    pub(super) fn wait(self) -> io::Result<()> {
        match &self {
            Self::Watching(process) => signaled(*process),
            Self::Ended => Ok(()),
        }
    }
}

impl Drop for ProcessExitObserver {
    fn drop(&mut self) {
        if let Self::Watching(process) = self {
            // SAFETY: this observer exclusively owns the process handle.
            unsafe { CloseHandle(*process) };
        }
    }
}

/// Whether process `pid` is still running. A process this user may not open
/// belongs to someone, so it counts as running.
pub(crate) fn process_running(pid: u32) -> io::Result<bool> {
    // SAFETY: synchronize access only observes the named process.
    let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if process.is_null() {
        let error = io::Error::last_os_error();
        return match error.raw_os_error() {
            // Windows names a process that no longer exists an invalid parameter.
            Some(code) if code == ERROR_INVALID_PARAMETER as i32 => Ok(false),
            Some(code) if code == ERROR_ACCESS_DENIED as i32 => Ok(true),
            _ => Err(error),
        };
    }
    // SAFETY: `process` is the handle opened above; a zero timeout only polls.
    let waited = unsafe { WaitForSingleObject(process, 0) };
    let failure = io::Error::last_os_error();
    // SAFETY: this probe exclusively owns the process handle.
    unsafe { CloseHandle(process) };
    match waited {
        WAIT_TIMEOUT => Ok(true),
        WAIT_OBJECT_0 => Ok(false),
        _ => Err(failure),
    }
}

fn signaled(handle: HANDLE) -> io::Result<()> {
    // SAFETY: the caller owns `handle` for the duration of the wait.
    if unsafe { WaitForSingleObject(handle, INFINITE) } == WAIT_OBJECT_0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

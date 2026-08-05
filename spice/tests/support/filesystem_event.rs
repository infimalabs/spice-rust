//! The one direct filesystem observer used by asynchronous test waits.
//!
//! A kernel notification is the sole mechanism that announces an arriving
//! state. Callers arm it before a controlled trigger and read their state once
//! immediately afterwards, closing the only setup race without adding a
//! time-driven second path. After that read, they block until this notification,
//! another explicit completion signal, or their own horizon answers.

use std::io;
use std::path::Path;

/// One direct kernel notification for a change to an existing path.
///
/// `notify` selects FSEvents on macOS. FSEvents is recursive, but it is also a
/// deferred delivery service: under lane load it has left an already-published
/// state unannounced past the readiness horizon. This observer uses the local
/// kernel primitive directly instead—kqueue on macOS and inotify on Linux—so a
/// write wakes the blocked waiter rather than entering the FSEvents delivery
/// queue.
pub(crate) struct DirectObserver {
    descriptor: libc::c_int,
    #[cfg(target_os = "macos")]
    watched: libc::c_int,
}

impl DirectObserver {
    /// Register before the caller reads its state, closing the setup race.
    pub(crate) fn arm(watched: &Path, what: &str) -> Self {
        #[cfg(target_os = "linux")]
        let observer = linux_observer(watched, what);
        #[cfg(target_os = "macos")]
        let observer = macos_observer(watched, what);
        observer
    }

    /// Block until the kernel reports one change to the watched path.
    pub(crate) fn wait(self) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        let result = wait_linux(self.descriptor);
        #[cfg(target_os = "macos")]
        let result = wait_macos(self.descriptor);
        result
    }
}

impl Drop for DirectObserver {
    fn drop(&mut self) {
        // SAFETY: the observer exclusively owns these descriptors.
        unsafe {
            libc::close(self.descriptor);
            #[cfg(target_os = "macos")]
            libc::close(self.watched);
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_observer(watched: &Path, what: &str) -> DirectObserver {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    // SAFETY: inotify_init1 creates an owned notification descriptor.
    let descriptor = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
    assert_ne!(
        descriptor,
        -1,
        "{what} observer starts: {}",
        io::Error::last_os_error()
    );
    let path =
        CString::new(watched.as_os_str().as_bytes()).expect("watched path has no interior nul");
    let mask = libc::IN_ATTRIB
        | libc::IN_CLOSE_WRITE
        | libc::IN_CREATE
        | libc::IN_DELETE
        | libc::IN_DELETE_SELF
        | libc::IN_MODIFY
        | libc::IN_MOVED_FROM
        | libc::IN_MOVED_TO
        | libc::IN_MOVE_SELF;
    // SAFETY: the path remains valid for this call.
    let watch = unsafe { libc::inotify_add_watch(descriptor, path.as_ptr(), mask) };
    if watch == -1 {
        let error = io::Error::last_os_error();
        // SAFETY: initialization above returned this owned descriptor.
        unsafe { libc::close(descriptor) };
        panic!("{what} observer arms on {}: {error}", watched.display());
    }
    DirectObserver { descriptor }
}

#[cfg(target_os = "linux")]
fn wait_linux(descriptor: libc::c_int) -> io::Result<()> {
    let mut event = [0_u8; 4096];
    loop {
        // SAFETY: the event destination is writable for its complete length.
        let read = unsafe { libc::read(descriptor, event.as_mut_ptr().cast(), event.len()) };
        if read > 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_observer(watched: &Path, what: &str) -> DirectObserver {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path =
        CString::new(watched.as_os_str().as_bytes()).expect("watched path has no interior nul");
    // SAFETY: the path remains valid for this call.
    let watched_descriptor = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    assert_ne!(
        watched_descriptor,
        -1,
        "{what} observer arms on {}: {}",
        watched.display(),
        io::Error::last_os_error()
    );
    // SAFETY: kqueue creates an owned file-event descriptor.
    let descriptor = unsafe { libc::kqueue() };
    if descriptor == -1 {
        let error = io::Error::last_os_error();
        // SAFETY: the open call above returned this owned descriptor.
        unsafe { libc::close(watched_descriptor) };
        panic!("{what} observer starts: {error}");
    }
    let change = libc::kevent {
        ident: watched_descriptor as libc::uintptr_t,
        filter: libc::EVFILT_VNODE,
        flags: libc::EV_ADD | libc::EV_ONESHOT,
        fflags: libc::NOTE_WRITE
            | libc::NOTE_EXTEND
            | libc::NOTE_ATTRIB
            | libc::NOTE_LINK
            | libc::NOTE_RENAME
            | libc::NOTE_DELETE
            | libc::NOTE_REVOKE,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    // SAFETY: the immutable registration is valid for this one syscall.
    let result = unsafe {
        libc::kevent(
            descriptor,
            &change,
            1,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
        )
    };
    if result == -1 {
        let error = io::Error::last_os_error();
        // SAFETY: both initialization calls above returned owned descriptors.
        unsafe {
            libc::close(descriptor);
            libc::close(watched_descriptor);
        }
        panic!("{what} observer arms on {}: {error}", watched.display());
    }
    DirectObserver {
        descriptor,
        watched: watched_descriptor,
    }
}

#[cfg(target_os = "macos")]
fn wait_macos(descriptor: libc::c_int) -> io::Result<()> {
    let mut event = std::mem::MaybeUninit::<libc::kevent>::uninit();
    loop {
        // SAFETY: the event destination is writable and no timeout is used.
        let result = unsafe {
            libc::kevent(
                descriptor,
                std::ptr::null(),
                0,
                event.as_mut_ptr(),
                1,
                std::ptr::null(),
            )
        };
        if result == 1 {
            // SAFETY: kevent initialized one complete event.
            let event = unsafe { event.assume_init() };
            if event.flags & libc::EV_ERROR != 0 {
                return Err(io::Error::from_raw_os_error(event.data as i32));
            }
            return Ok(());
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

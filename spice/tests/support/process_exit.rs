//! One blocking observation of a child process ending.
//!
//! The kernel announces the exit, so nothing here polls for it. Asking to watch
//! a process is itself a race the caller cannot win: a child that was live when
//! its status was last read can be gone before the registration lands. Both
//! kernels report that as `ESRCH`.
//!
//! Whether that refusal is an answer or a defect belongs to the caller, so it
//! is spelled in the constructor rather than decided here for everyone.
//! [`ProcessExitObserver::or_ended`] treats it as the exit having already
//! happened, which is what a caller racing its own child needs.
//! [`ProcessExitObserver::new`] refuses it by name, because a caller that arms
//! an observer on a process it is about to signal has lost the thing it meant
//! to watch, and a silent `Ended` there would let it pass having observed
//! nothing.

use std::io;

/// A kernel subscription to one child's exit, or the exit already having happened.
pub(crate) enum ProcessExitObserver {
    /// The kernel will announce the exit through this descriptor.
    Watching(libc::c_int),
    /// The process was already gone when the subscription was attempted.
    Ended,
}

impl ProcessExitObserver {
    /// Observe a process that must still be there, naming it when it is not.
    #[allow(
        dead_code,
        reason = "each test target compiles its own copy of this module, and the batteries that only wait on a child of their own reach it through the tolerant constructor alone"
    )]
    pub(crate) fn new(pid: u32) -> Self {
        match Self::or_ended(pid) {
            watching @ Self::Watching(_) => watching,
            Self::Ended => {
                panic!("process {pid} had already ended before its exit observer was armed")
            }
        }
    }

    /// Observe a process that may already have ended, which answers at once.
    pub(crate) fn or_ended(pid: u32) -> Self {
        #[cfg(target_os = "linux")]
        let observed = linux_observer(pid);
        #[cfg(target_os = "macos")]
        let observed = macos_observer(pid);
        match observed {
            Some(descriptor) => Self::Watching(descriptor),
            None => Self::Ended,
        }
    }

    /// Block until the observed process has ended, returning at once if it has.
    pub(crate) fn wait(self) {
        let Self::Watching(descriptor) = &self else {
            return;
        };
        #[cfg(target_os = "linux")]
        wait_linux(*descriptor);
        #[cfg(target_os = "macos")]
        wait_macos(*descriptor);
    }
}

impl Drop for ProcessExitObserver {
    fn drop(&mut self) {
        if let Self::Watching(descriptor) = self {
            // SAFETY: the observer exclusively owns this descriptor.
            unsafe {
                libc::close(*descriptor);
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_observer(pid: u32) -> Option<libc::c_int> {
    // SAFETY: pidfd_open observes the named process without changing it.
    let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) as libc::c_int };
    if descriptor != -1 {
        return Some(descriptor);
    }
    let refusal = io::Error::last_os_error();
    assert_eq!(
        refusal.raw_os_error(),
        Some(libc::ESRCH),
        "pidfd observer: {refusal}"
    );
    None
}

#[cfg(target_os = "linux")]
fn wait_linux(descriptor: libc::c_int) {
    let mut event = libc::pollfd {
        fd: descriptor,
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: the single poll descriptor remains writable during the call.
        let result = unsafe { libc::poll(&mut event, 1, -1) };
        if result == 1 {
            assert_ne!(event.revents & libc::POLLIN, 0);
            return;
        }
        assert_eq!(
            io::Error::last_os_error().kind(),
            io::ErrorKind::Interrupted
        );
    }
}

#[cfg(target_os = "macos")]
fn macos_observer(pid: u32) -> Option<libc::c_int> {
    // SAFETY: kqueue creates an owned process-event descriptor.
    let descriptor = unsafe { libc::kqueue() };
    assert_ne!(
        descriptor,
        -1,
        "kqueue observer: {}",
        io::Error::last_os_error()
    );
    let change = libc::kevent {
        ident: pid as libc::uintptr_t,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
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
    if result == 0 {
        return Some(descriptor);
    }
    let refusal = io::Error::last_os_error();
    // SAFETY: registration failed, so nothing else refers to this descriptor.
    unsafe {
        libc::close(descriptor);
    }
    assert_eq!(
        refusal.raw_os_error(),
        Some(libc::ESRCH),
        "kqueue registration: {refusal}"
    );
    None
}

#[cfg(target_os = "macos")]
fn wait_macos(descriptor: libc::c_int) {
    let mut event = std::mem::MaybeUninit::<libc::kevent>::uninit();
    loop {
        // SAFETY: the event destination is writable and no timeout is requested.
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
            return;
        }
        assert_eq!(
            io::Error::last_os_error().kind(),
            io::ErrorKind::Interrupted
        );
    }
}

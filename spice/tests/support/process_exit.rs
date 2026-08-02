use std::io;

pub(super) struct ProcessExitObserver {
    descriptor: libc::c_int,
}

impl ProcessExitObserver {
    pub(super) fn new(pid: u32) -> Self {
        #[cfg(target_os = "linux")]
        let descriptor = linux_observer(pid);
        #[cfg(target_os = "macos")]
        let descriptor = macos_observer(pid);
        Self { descriptor }
    }

    pub(super) fn wait(self) {
        #[cfg(target_os = "linux")]
        wait_linux(self.descriptor);
        #[cfg(target_os = "macos")]
        wait_macos(self.descriptor);
    }
}

impl Drop for ProcessExitObserver {
    fn drop(&mut self) {
        // SAFETY: the observer exclusively owns this descriptor.
        unsafe {
            libc::close(self.descriptor);
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_observer(pid: u32) -> libc::c_int {
    // SAFETY: pidfd_open observes the named process without changing it.
    let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) as libc::c_int };
    assert_ne!(
        descriptor,
        -1,
        "pidfd observer: {}",
        io::Error::last_os_error()
    );
    descriptor
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
fn macos_observer(pid: u32) -> libc::c_int {
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
    assert_eq!(
        result,
        0,
        "kqueue registration: {}",
        io::Error::last_os_error()
    );
    descriptor
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

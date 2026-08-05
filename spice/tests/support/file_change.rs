use std::io;
use std::path::Path;

/// How long a wait tolerates silence before its caller reports what never came.
const WAIT_TIMEOUT_MILLISECONDS: libc::c_int = 30_000;

/// Blocks until one file is written, so a fixture can await a record rather
/// than sample for it.
///
/// The observer is registered before its caller reads the file it watches, so
/// a write landing between the read and the wait is still delivered: the wait
/// returns for a change that already happened rather than missing it.
pub(super) struct FileChangeObserver {
    descriptor: libc::c_int,
    #[cfg(target_os = "macos")]
    watched: libc::c_int,
}

impl FileChangeObserver {
    pub(super) fn new(path: &Path) -> Self {
        #[cfg(target_os = "linux")]
        let observer = linux_observer(path);
        #[cfg(target_os = "macos")]
        let observer = macos_observer(path);
        observer
    }

    /// Block for one write, reporting whether one arrived before the deadline.
    ///
    /// The deadline exists only so that a caller whose write never comes fails
    /// with what it was waiting for instead of hanging; a write that does come
    /// returns immediately, so no wait is ever paced by it.
    #[must_use]
    pub(super) fn wait(&self) -> bool {
        #[cfg(target_os = "linux")]
        let written = wait_linux(self.descriptor);
        #[cfg(target_os = "macos")]
        let written = wait_macos(self.descriptor);
        written
    }
}

impl Drop for FileChangeObserver {
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
fn linux_observer(path: &Path) -> FileChangeObserver {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    // SAFETY: inotify_init1 creates an owned notification descriptor.
    let descriptor = unsafe { libc::inotify_init1(0) };
    assert_ne!(
        descriptor,
        -1,
        "inotify observer: {}",
        io::Error::last_os_error()
    );
    let watched = CString::new(path.as_os_str().as_bytes()).expect("watched path has no interior nul");
    // SAFETY: the path stays valid for the duration of this call.
    let watch = unsafe { libc::inotify_add_watch(descriptor, watched.as_ptr(), libc::IN_MODIFY) };
    assert_ne!(
        watch,
        -1,
        "inotify watch on {}: {}",
        path.display(),
        io::Error::last_os_error()
    );
    FileChangeObserver { descriptor }
}

#[cfg(target_os = "linux")]
fn wait_linux(descriptor: libc::c_int) -> bool {
    let mut ready = libc::pollfd {
        fd: descriptor,
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: the single poll descriptor remains writable during the call.
        let result = unsafe { libc::poll(&mut ready, 1, WAIT_TIMEOUT_MILLISECONDS) };
        if result == 0 {
            return false;
        }
        if result == 1 {
            let mut event = [0_u8; size_of::<libc::inotify_event>() + libc::PATH_MAX as usize];
            // SAFETY: the event destination is writable for its complete length.
            let read = unsafe { libc::read(descriptor, event.as_mut_ptr().cast(), event.len()) };
            assert!(read > 0, "inotify read: {}", io::Error::last_os_error());
            return true;
        }
        assert_eq!(
            io::Error::last_os_error().kind(),
            io::ErrorKind::Interrupted
        );
    }
}

#[cfg(target_os = "macos")]
fn macos_observer(path: &Path) -> FileChangeObserver {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let watched = CString::new(path.as_os_str().as_bytes()).expect("watched path has no interior nul");
    // SAFETY: the path stays valid for the duration of this call.
    let watched = unsafe { libc::open(watched.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    assert_ne!(
        watched,
        -1,
        "open {} to watch: {}",
        path.display(),
        io::Error::last_os_error()
    );
    // SAFETY: kqueue creates an owned file-event descriptor.
    let descriptor = unsafe { libc::kqueue() };
    assert_ne!(
        descriptor,
        -1,
        "kqueue observer: {}",
        io::Error::last_os_error()
    );
    let change = libc::kevent {
        ident: watched as libc::uintptr_t,
        filter: libc::EVFILT_VNODE,
        flags: libc::EV_ADD | libc::EV_CLEAR,
        fflags: libc::NOTE_WRITE | libc::NOTE_EXTEND,
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
    FileChangeObserver {
        descriptor,
        watched,
    }
}

#[cfg(target_os = "macos")]
fn wait_macos(descriptor: libc::c_int) -> bool {
    let deadline = libc::timespec {
        tv_sec: WAIT_TIMEOUT_MILLISECONDS as libc::time_t / 1_000,
        tv_nsec: 0,
    };
    let mut event = std::mem::MaybeUninit::<libc::kevent>::uninit();
    loop {
        // SAFETY: the event destination and the deadline are valid for the call.
        let result = unsafe {
            libc::kevent(
                descriptor,
                std::ptr::null(),
                0,
                event.as_mut_ptr(),
                1,
                &deadline,
            )
        };
        if result == 0 {
            return false;
        }
        if result == 1 {
            return true;
        }
        assert_eq!(
            io::Error::last_os_error().kind(),
            io::ErrorKind::Interrupted
        );
    }
}

//! Direct filesystem and process-exit observers for detached startup.

use std::io;
#[cfg(unix)]
use std::path::Path;
use std::sync::mpsc;
use std::thread;

pub(super) enum Signal {
    Changed,
    Exited,
    ObserverFailed(String),
}

impl Signal {
    pub(super) fn woke(woke: io::Result<()>) -> Self {
        woke.map_or_else(
            |error| Self::ObserverFailed(error.to_string()),
            |()| Self::Changed,
        )
    }
}

/// Send what `signal` makes of the armed observer's wake, once it wakes.
///
/// The observer is armed before the reader looks, so a change between the
/// look and the wait is not lost; it fires once, and the reader arms another
/// after each wake. A watch the directory never wakes again holds its thread
/// until this process ends.
pub(in crate::resident) fn wait_for_change<T: Send + 'static>(
    observer: DirectObserver,
    sender: mpsc::Sender<T>,
    signal: impl FnOnce(io::Result<()>) -> T + Send + 'static,
) {
    thread::spawn(move || {
        let _ = sender.send(signal(observer.wait()));
    });
}

pub(in crate::resident) fn wait_for_exit(pid: u32, sender: impl ExitSender) {
    thread::spawn(move || {
        match ProcessExitObserver::or_ended(pid).and_then(ProcessExitObserver::wait) {
            Ok(()) => sender.exited(),
            Err(error) => sender.failed(error.to_string()),
        }
    });
}

pub(in crate::resident) trait ExitSender: Send + 'static {
    fn exited(self);
    fn failed(self, error: String);
}

impl ExitSender for mpsc::Sender<Signal> {
    fn exited(self) {
        let _ = self.send(Signal::Exited);
    }

    fn failed(self, error: String) {
        let _ = self.send(Signal::ObserverFailed(error));
    }
}

impl ExitSender for mpsc::SyncSender<()> {
    fn exited(self) {
        let _ = self.send(());
    }

    fn failed(self, _error: String) {
        let _ = self.send(());
    }
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::DirectObserver;
#[cfg(windows)]
pub(crate) use windows::ProcessExitObserver;

#[cfg(unix)]
pub(crate) struct DirectObserver {
    descriptor: libc::c_int,
    #[cfg(target_os = "macos")]
    watched: libc::c_int,
}

#[cfg(unix)]
impl DirectObserver {
    pub(crate) fn arm(watched: &Path) -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            linux_directory_observer(watched)
        }
        #[cfg(target_os = "macos")]
        {
            macos_directory_observer(watched)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = watched;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "direct directory observer is unavailable on this Unix",
            ))
        }
    }

    /// Block until the watched directory changes once.
    ///
    /// # Errors
    ///
    /// Returns the kernel's refusal to deliver the change.
    pub(crate) fn wait(self) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        {
            wait_linux_directory(self.descriptor)
        }
        #[cfg(target_os = "macos")]
        {
            wait_macos_event(self.descriptor)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "direct directory observer is unavailable on this Unix",
            ))
        }
    }
}

#[cfg(unix)]
impl Drop for DirectObserver {
    fn drop(&mut self) {
        // SAFETY: this observer exclusively owns these descriptors.
        unsafe {
            libc::close(self.descriptor);
            #[cfg(target_os = "macos")]
            libc::close(self.watched);
        }
    }
}

#[cfg(target_os = "linux")]
fn linux_directory_observer(watched: &Path) -> io::Result<DirectObserver> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let path = CString::new(watched.as_os_str().as_bytes())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    // SAFETY: inotify_init1 creates an owned descriptor.
    let descriptor = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
    if descriptor == -1 {
        return Err(io::Error::last_os_error());
    }
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
    if unsafe { libc::inotify_add_watch(descriptor, path.as_ptr(), mask) } == -1 {
        let error = io::Error::last_os_error();
        // SAFETY: initialization returned this owned descriptor.
        unsafe { libc::close(descriptor) };
        return Err(error);
    }
    Ok(DirectObserver { descriptor })
}

#[cfg(target_os = "linux")]
fn wait_linux_directory(descriptor: libc::c_int) -> io::Result<()> {
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
fn macos_directory_observer(watched: &Path) -> io::Result<DirectObserver> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    let path = CString::new(watched.as_os_str().as_bytes())
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
    // SAFETY: open and kqueue each create an owned descriptor.
    let watched = unsafe { libc::open(path.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
    if watched == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: kqueue creates an owned descriptor.
    let descriptor = unsafe { libc::kqueue() };
    if descriptor == -1 {
        let error = io::Error::last_os_error();
        // SAFETY: open returned this owned descriptor.
        unsafe { libc::close(watched) };
        return Err(error);
    }
    let change = libc::kevent {
        ident: watched as libc::uintptr_t,
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
    // SAFETY: the immutable registration is valid for this syscall.
    if unsafe {
        libc::kevent(
            descriptor,
            &change,
            1,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
        )
    } == -1
    {
        let error = io::Error::last_os_error();
        // SAFETY: both initialization calls returned owned descriptors.
        unsafe {
            libc::close(descriptor);
            libc::close(watched);
        }
        return Err(error);
    }
    Ok(DirectObserver {
        descriptor,
        watched,
    })
}

#[cfg(target_os = "macos")]
fn wait_macos_event(descriptor: libc::c_int) -> io::Result<()> {
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

/// One process's exit, armed while the process can still be named.
#[cfg(unix)]
pub(crate) enum ProcessExitObserver {
    Watching(libc::c_int),
    Ended,
}

#[cfg(unix)]
impl ProcessExitObserver {
    /// This process's parent, or `Ended` once this process has been
    /// reparented by the time the parent's exit is armed.
    ///
    /// # Errors
    ///
    /// Returns the kernel's refusal to arm the observer, or the parent this
    /// process was already orphaned to.
    pub(crate) fn parent() -> io::Result<Self> {
        let parent = current_parent()?;
        let observer = Self::or_ended(parent)?;
        // A parent that ended before the observer was armed may already
        // have handed this process to another, whose id then differs.
        if current_parent()? != parent {
            return Ok(Self::Ended);
        }
        Ok(observer)
    }

    pub(crate) fn or_ended(pid: u32) -> io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            // SAFETY: pidfd_open observes the named process without changing it.
            let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) as libc::c_int };
            if descriptor != -1 {
                return Ok(Self::Watching(descriptor));
            }
            let error = io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ESRCH) {
                return Ok(Self::Ended);
            }
            Err(error)
        }
        #[cfg(target_os = "macos")]
        {
            macos_process_observer(pid)
                .map(|descriptor| descriptor.map_or(Self::Ended, Self::Watching))
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = pid;
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "direct process observer is unavailable on this Unix",
            ))
        }
    }

    pub(crate) fn wait(self) -> io::Result<()> {
        let Self::Watching(descriptor) = &self else {
            return Ok(());
        };
        #[cfg(target_os = "linux")]
        {
            let mut event = libc::pollfd {
                fd: *descriptor,
                events: libc::POLLIN,
                revents: 0,
            };
            loop {
                // SAFETY: the single poll descriptor is valid and writable.
                let result = unsafe { libc::poll(&mut event, 1, -1) };
                if result == 1 {
                    return Ok(());
                }
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    return Err(error);
                }
            }
        }
        #[cfg(target_os = "macos")]
        {
            wait_macos_event(*descriptor)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = descriptor;
            Ok(())
        }
    }
}

#[cfg(unix)]
impl Drop for ProcessExitObserver {
    fn drop(&mut self) {
        if let Self::Watching(descriptor) = self {
            // SAFETY: this observer exclusively owns the descriptor.
            unsafe { libc::close(*descriptor) };
        }
    }
}

#[cfg(unix)]
fn current_parent() -> io::Result<u32> {
    // SAFETY: getppid has no preconditions and observes this process only.
    let parent = unsafe { libc::getppid() };
    u32::try_from(parent)
        .ok()
        .filter(|parent| *parent > 1)
        .ok_or_else(|| io::Error::other(format!("parent process is unavailable: {parent}")))
}

#[cfg(target_os = "macos")]
fn macos_process_observer(pid: u32) -> io::Result<Option<libc::c_int>> {
    // SAFETY: kqueue creates an owned process-event descriptor.
    let descriptor = unsafe { libc::kqueue() };
    if descriptor == -1 {
        return Err(io::Error::last_os_error());
    }
    let change = libc::kevent {
        ident: pid as libc::uintptr_t,
        filter: libc::EVFILT_PROC,
        flags: libc::EV_ADD | libc::EV_ONESHOT,
        fflags: libc::NOTE_EXIT,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    // SAFETY: the immutable registration is valid for this syscall.
    if unsafe {
        libc::kevent(
            descriptor,
            &change,
            1,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
        )
    } == 0
    {
        Ok(Some(descriptor))
    } else {
        let error = io::Error::last_os_error();
        // SAFETY: registration failed and nothing else owns the descriptor.
        unsafe { libc::close(descriptor) };
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(None)
        } else {
            Err(error)
        }
    }
}

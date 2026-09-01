//! Direct parent-exit cancellation for the pre-commit supervisor.

use std::io;
use std::thread;

use sill::{Cancellation, CancellationToken};

pub(super) struct ParentCancellation {
    cancellation: Cancellation,
}

impl ParentCancellation {
    pub(super) fn capture() -> io::Result<Self> {
        let parent = current_parent()?;
        let observer = ProcessExitObserver::or_ended(parent)?;
        let token = CancellationToken::new();
        if matches!(observer, ProcessExitObserver::Ended) || current_parent()? != parent {
            token.cancel();
        } else {
            let cancellation = token.clone();
            thread::Builder::new()
                .name("spice-pre-commit-parent".to_owned())
                .spawn(move || {
                    let _ = observer.wait();
                    cancellation.cancel();
                })?;
        }
        Ok(Self {
            cancellation: Cancellation::Token(token),
        })
    }

    pub(super) fn cancellation(&self) -> Cancellation {
        self.cancellation.clone()
    }
}

fn current_parent() -> io::Result<u32> {
    // SAFETY: getppid has no preconditions and observes this process only.
    let parent = unsafe { libc::getppid() };
    u32::try_from(parent)
        .ok()
        .filter(|parent| *parent > 1)
        .ok_or_else(|| io::Error::other(format!("pre-commit parent is unavailable: {parent}")))
}

pub(super) enum ProcessExitObserver {
    Watching(libc::c_int),
    Ended,
}

impl ProcessExitObserver {
    pub(super) fn or_ended(pid: u32) -> io::Result<Self> {
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
            return Err(error);
        }
        #[cfg(target_os = "macos")]
        return macos_observer(pid)
            .map(|descriptor| descriptor.map_or(Self::Ended, Self::Watching));
    }

    pub(super) fn wait(self) -> io::Result<()> {
        let Self::Watching(descriptor) = &self else {
            return Ok(());
        };
        #[cfg(target_os = "linux")]
        return wait_linux(*descriptor);
        #[cfg(target_os = "macos")]
        return wait_macos(*descriptor);
    }
}

impl Drop for ProcessExitObserver {
    fn drop(&mut self) {
        if let Self::Watching(descriptor) = self {
            // SAFETY: the observer exclusively owns this descriptor.
            unsafe { libc::close(*descriptor) };
        }
    }
}

#[cfg(target_os = "linux")]
fn wait_linux(descriptor: libc::c_int) -> io::Result<()> {
    let mut event = libc::pollfd {
        fd: descriptor,
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: the single poll descriptor remains writable during the call.
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
fn macos_observer(pid: u32) -> io::Result<Option<libc::c_int>> {
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
        return Ok(Some(descriptor));
    }
    let error = io::Error::last_os_error();
    // SAFETY: registration failed and nothing else owns the descriptor.
    unsafe { libc::close(descriptor) };
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(None)
    } else {
        Err(error)
    }
}

#[cfg(target_os = "macos")]
fn wait_macos(descriptor: libc::c_int) -> io::Result<()> {
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

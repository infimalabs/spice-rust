//! Independent lock possession tied to one protected process group's lifetime.
//!
//! The child reports its new group before `exec`, while the wrapper retains the
//! notification endpoint as a liveness token. The guardian blocks on that token
//! and begins watching group absence only after the wrapper completes or dies.

use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::thread::JoinHandle;

const GUARDIAN_LOCK_FD: RawFd = 3;
const GUARDIAN_NOTICE_FD: RawFd = 4;
const DUPLICATE_MINIMUM_FD: RawFd = 10;
const POLL_NANOSECONDS: libc::c_long = 10_000_000;

#[derive(Debug)]
pub(super) struct PendingGuardian {
    notifier: Option<UnixStream>,
    guardian: Option<Guardian>,
}

impl PendingGuardian {
    pub(super) fn start(file: &File) -> io::Result<Self> {
        let (notifier, receiver) = UnixStream::pair()?;
        let lock = duplicate(file.as_raw_fd())?;
        let notice = duplicate(receiver.as_raw_fd())?;
        let inherited_descriptors = inherited_descriptors()?;
        let maximum_fd = maximum_fd();

        // SAFETY: the child branch invokes only async-signal-safe system calls and
        // exits with `_exit`, without touching shared Rust runtime state.
        let pid = unsafe { libc::fork() };
        if pid == -1 {
            return Err(io::Error::last_os_error());
        }
        if pid == 0 {
            guardian_main(
                lock.as_raw_fd(),
                notice.as_raw_fd(),
                maximum_fd,
                &inherited_descriptors,
            );
        }

        drop(receiver);
        drop(lock);
        drop(notice);
        let reaper = match std::thread::Builder::new()
            .name("sill-lock-guardian".to_owned())
            .spawn(move || reap(pid))
        {
            Ok(reaper) => reaper,
            Err(error) => {
                drop(notifier);
                // SAFETY: `pid` identifies the guardian forked above.
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
                let _reap_result = reap(pid);
                return Err(error);
            }
        };
        Ok(Self {
            notifier: Some(notifier),
            guardian: Some(Guardian {
                notifier: None,
                reaper: Some(reaper),
            }),
        })
    }

    pub(super) fn notifier_fd(&self) -> RawFd {
        self.notifier
            .as_ref()
            .expect("pending guardian retains its notifier")
            .as_raw_fd()
    }

    pub(super) fn activate(mut self) -> Guardian {
        let mut guardian = self
            .guardian
            .take()
            .expect("pending guardian retains its process");
        guardian.notifier = self.notifier.take();
        guardian
    }
}

impl Drop for PendingGuardian {
    fn drop(&mut self) {
        drop(self.notifier.take());
        if let Some(guardian) = self.guardian.take() {
            let _wait_result = guardian.wait();
        }
    }
}

#[derive(Debug)]
pub(super) struct Guardian {
    notifier: Option<UnixStream>,
    reaper: Option<JoinHandle<io::Result<()>>>,
}

impl Guardian {
    pub(super) fn wait(mut self) -> io::Result<()> {
        drop(self.notifier.take());
        self.reaper
            .take()
            .expect("guardian retains its reaper")
            .join()
            .map_err(|_| io::Error::other("lock guardian reaper panicked"))?
    }
}

pub(super) fn notify_group(notifier: RawFd) -> io::Result<()> {
    // SAFETY: this child-only hook uses process-local async-signal-safe calls.
    if unsafe { libc::setpgid(0, 0) } == -1 {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: `getpid` has no preconditions and returns this child process's id.
    let pid = unsafe { libc::getpid() };
    let bytes = pid.to_ne_bytes();
    let mut written = 0;
    while written < bytes.len() {
        // SAFETY: the byte slice remains valid for the duration of this call.
        let result = unsafe {
            libc::send(
                notifier,
                bytes[written..].as_ptr().cast(),
                bytes.len() - written,
                libc::MSG_NOSIGNAL,
            )
        };
        if result > 0 {
            written += result as usize;
            continue;
        }
        if result == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "lock guardian notification closed early",
            ));
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    Ok(())
}

pub(super) fn group_absent(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    raw_group_absent(pid)
}

fn duplicate(descriptor: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: `fcntl` duplicates the live descriptor into a newly owned slot.
    let duplicate = unsafe { libc::fcntl(descriptor, libc::F_DUPFD_CLOEXEC, DUPLICATE_MINIMUM_FD) };
    if duplicate == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the successful `fcntl` result is a fresh owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicate) })
}

fn maximum_fd() -> RawFd {
    // SAFETY: `_SC_OPEN_MAX` is a process-local read-only query.
    let maximum = unsafe { libc::sysconf(libc::_SC_OPEN_MAX) };
    RawFd::try_from(maximum).unwrap_or(65_536).max(5)
}

#[cfg(target_os = "linux")]
fn inherited_descriptors() -> io::Result<Vec<RawFd>> {
    Ok(Vec::new())
}

#[cfg(target_os = "macos")]
fn inherited_descriptors() -> io::Result<Vec<RawFd>> {
    std::fs::read_dir("/dev/fd")?
        .map(|entry| {
            entry.and_then(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .parse()
                    .map_err(|source| io::Error::new(io::ErrorKind::InvalidData, source))
            })
        })
        .collect()
}

fn guardian_main(
    lock: RawFd,
    notice: RawFd,
    maximum_fd: RawFd,
    inherited_descriptors: &[RawFd],
) -> ! {
    // SAFETY: the guardian is the only thread in this forked process and uses
    // only raw descriptors until it exits without running Rust destructors.
    unsafe {
        if libc::setpgid(0, 0) == -1
            || libc::dup2(lock, GUARDIAN_LOCK_FD) == -1
            || libc::dup2(notice, GUARDIAN_NOTICE_FD) == -1
        {
            libc::_exit(125);
        }
        close_unneeded_descriptors(maximum_fd, inherited_descriptors);

        let mut bytes = [0_u8; size_of::<u32>()];
        let mut received = 0;
        while received < bytes.len() {
            let result = libc::read(
                GUARDIAN_NOTICE_FD,
                bytes[received..].as_mut_ptr().cast(),
                bytes.len() - received,
            );
            if result > 0 {
                received += result as usize;
            } else if result == 0 {
                libc::_exit(0);
            } else if errno() != libc::EINTR {
                libc::_exit(125);
            }
        }
        let pid = libc::pid_t::try_from(u32::from_ne_bytes(bytes)).unwrap_or(0);
        if pid <= 0 {
            libc::_exit(125);
        }
        wait_for_wrapper_close();
        libc::close(GUARDIAN_NOTICE_FD);
        while !raw_group_absent(pid) {
            let request = libc::timespec {
                tv_sec: 0,
                tv_nsec: POLL_NANOSECONDS,
            };
            libc::nanosleep(&request, std::ptr::null_mut());
        }
        libc::_exit(0);
    }
}

unsafe fn wait_for_wrapper_close() {
    let mut byte = 0_u8;
    loop {
        // SAFETY: the one-byte destination remains valid for the duration of the call.
        let result = unsafe { libc::read(GUARDIAN_NOTICE_FD, (&raw mut byte).cast(), 1) };
        if result == 0 {
            return;
        }
        if result == -1 && unsafe { errno() } != libc::EINTR {
            return;
        }
    }
}

#[cfg(target_os = "linux")]
unsafe fn close_unneeded_descriptors(maximum_fd: RawFd, _inherited_descriptors: &[RawFd]) {
    for descriptor in 0..GUARDIAN_LOCK_FD {
        // SAFETY: closing an inherited descriptor is valid and failures are irrelevant.
        unsafe {
            libc::close(descriptor);
        }
    }

    // SAFETY: this closes only descriptors above the guardian's two retained slots.
    if unsafe {
        libc::syscall(
            libc::SYS_close_range,
            GUARDIAN_NOTICE_FD + 1,
            libc::c_uint::MAX,
            0,
        )
    } == 0
    {
        return;
    }

    for descriptor in (GUARDIAN_NOTICE_FD + 1)..maximum_fd {
        // SAFETY: closing an inherited descriptor is valid and failures are irrelevant.
        unsafe {
            libc::close(descriptor);
        }
    }
}

#[cfg(target_os = "macos")]
unsafe fn close_unneeded_descriptors(_maximum_fd: RawFd, inherited_descriptors: &[RawFd]) {
    for &descriptor in inherited_descriptors {
        if descriptor == GUARDIAN_LOCK_FD || descriptor == GUARDIAN_NOTICE_FD {
            continue;
        }
        // SAFETY: closing an inherited descriptor is valid and failures are irrelevant.
        unsafe {
            libc::close(descriptor);
        }
    }
}

fn raw_group_absent(pid: libc::pid_t) -> bool {
    // SAFETY: signal zero probes process-group existence without delivering a signal.
    unsafe { libc::kill(-pid, 0) == -1 && errno() == libc::ESRCH }
}

unsafe fn errno() -> libc::c_int {
    #[cfg(target_os = "linux")]
    // SAFETY: libc exposes the calling thread's errno slot.
    unsafe {
        *libc::__errno_location()
    }
    #[cfg(target_os = "macos")]
    // SAFETY: libc exposes the calling thread's errno slot.
    unsafe {
        *libc::__error()
    }
}

fn reap(pid: libc::pid_t) -> io::Result<()> {
    let mut status = 0;
    loop {
        // SAFETY: `pid` is this process's guardian child and `status` is writable.
        let result = unsafe { libc::waitpid(pid, &mut status, 0) };
        if result == pid {
            return (status == 0).then_some(()).ok_or_else(|| {
                io::Error::other(format!("lock guardian exited with wait status {status}"))
            });
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

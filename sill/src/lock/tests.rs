use super::{Acquisition, LockFile, LockName};
use std::fs;
use std::path::PathBuf;
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn possession_is_exclusive_and_release_is_automatic() {
    let path = unique_lock_path("exclusive");
    let lock = LockFile::new(LockName::new("exclusive").expect("valid name"), &path);
    let first = acquired(lock.try_acquire().expect("first acquisition"));

    assert!(matches!(
        lock.try_acquire().expect("contending acquisition"),
        Acquisition::Contended
    ));

    drop(first);
    let second = acquired(lock.try_acquire().expect("acquisition after release"));
    drop(second);
    fs::remove_file(path).expect("remove test lock file");
}

#[cfg(unix)]
#[test]
fn explicit_child_inheritance_retains_possession_until_the_child_exits() {
    let path = unique_lock_path("child-inheritance");
    let lock = LockFile::new(
        LockName::new("child-inheritance").expect("valid name"),
        &path,
    );
    let mut possession = acquired(lock.try_acquire().expect("parent acquisition"));
    let mut command = process::Command::new("/usr/bin/tail");
    command.args(["-f", "/dev/null"]);
    possession.retain_through_child(&mut command);
    let mut child = command.spawn().expect("spawn inheriting child");

    drop(possession);
    assert_eq!(
        lock.availability().expect("probe inherited possession"),
        super::Availability::Held,
    );

    child.kill().expect("stop inheriting child");
    child.wait().expect("reap inheriting child");
    assert_eq!(
        lock.availability().expect("probe released possession"),
        super::Availability::Available,
    );
    fs::remove_file(path).expect("remove test lock file");
}

#[test]
fn lock_names_are_single_safe_path_components() {
    for name in [
        "",
        ".",
        "..",
        "../outside",
        "with/slash",
        "with\\backslash",
        "nul\0byte",
    ] {
        assert!(LockName::new(name).is_err(), "{name:?} must be refused");
    }
    for name in ["worker", "a.b", "two-words", "UPPER", "9"] {
        assert_eq!(
            LockName::new(name).expect("normal path component").as_str(),
            name
        );
    }
}

/// The lock file, opened the way every synchronized path opens it.
///
/// The caller keeps this descriptor open across the whole answer, because
/// it is what the owner record was read from, and probes with a clone of
/// it. That is the arrangement the probe has to give possession back
/// under, so it is the one tested.
#[cfg(unix)]
fn opened(path: &std::path::Path) -> fs::File {
    use std::os::unix::fs::OpenOptionsExt;

    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(super::platform::O_CLOEXEC | super::platform::O_NOFOLLOW)
        .open(path)
        .expect("lock file opens")
}

/// A probe answers a question; it must not keep what it took to answer it.
#[cfg(unix)]
#[test]
fn an_availability_probe_gives_back_the_possession_it_took() {
    let path = unique_lock_path("probe-release");
    let lock = LockFile::new(LockName::new("probe-release").expect("valid name"), &path);
    let file = opened(&path);

    assert_eq!(
        lock.availability_opened(file.try_clone().expect("probe clone"))
            .expect("probe answers"),
        super::Availability::Available
    );

    // Closing the clone cannot be what released it: the clone shared an
    // open file description with `file`, which is still open here, and
    // flock binds to the description. Only an explicit unlock gets here.
    acquired(lock.try_acquire().expect("acquisition after a probe"));

    drop(file);
    fs::remove_file(path).expect("remove test lock file");
}

/// A fork during the probe window inherits the description, not possession.
///
/// This is the shape that made a whole-battery run flake where the fixture
/// never did in isolation: `sill` forks a guardian that does not `exec`, so
/// CLOEXEC clears nothing, and a probe lock left on the description outlives
/// the descriptor the prober itself closes.
#[cfg(unix)]
#[test]
fn a_fork_during_a_probe_inherits_no_possession() {
    let path = unique_lock_path("probe-fork");
    let lock = LockFile::new(LockName::new("probe-fork").expect("valid name"), &path);
    let file = opened(&path);

    assert_eq!(
        lock.availability_opened(file.try_clone().expect("probe clone"))
            .expect("probe answers"),
        super::Availability::Available
    );

    // SAFETY: the child only pauses and exits, both async-signal-safe, and
    // is signalled and reaped below.
    let child = unsafe { libc::fork() };
    assert!(
        child >= 0,
        "fork fails: {}",
        std::io::Error::last_os_error()
    );
    if child == 0 {
        // Hold every inherited descriptor open until the parent is done.
        unsafe {
            libc::pause();
            libc::_exit(0);
        }
    }

    // The prober closes its own descriptor. Only the child's copy remains.
    drop(file);
    let attempt = lock
        .try_acquire()
        .expect("acquisition beside a forked child");

    // SAFETY: this signals the child forked above and nothing else.
    unsafe {
        libc::kill(child, libc::SIGKILL);
        libc::waitpid(child, std::ptr::null_mut(), 0);
    }
    acquired(attempt);
    fs::remove_file(path).expect("remove test lock file");
}

fn acquired(acquisition: Acquisition) -> super::Possession {
    match acquisition {
        Acquisition::Acquired(possession) => possession,
        Acquisition::Contended => panic!("lock should have been acquired"),
    }
}

pub(crate) fn unique_lock_path(stem: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time must follow Unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("sill-{stem}-{}-{nonce}.lock", process::id()))
}

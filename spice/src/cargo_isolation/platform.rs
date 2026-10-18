//! Filesystem permission compatibility for Cargo isolation.

#[cfg(windows)]
use std::fs;

/// The permission operations used by the source store on Windows.
///
/// Windows exposes a read-only attribute rather than Unix mode bits. The
/// writable bits are therefore authoritative; execute bits remain present so
/// package executables retain their role across a Windows-hosted store.
#[cfg(windows)]
pub(super) trait PermissionsExt {
    fn mode(&self) -> u32;
    fn from_mode(mode: u32) -> Self;
}

#[cfg(windows)]
impl PermissionsExt for fs::Permissions {
    fn mode(&self) -> u32 {
        if self.readonly() { 0o555 } else { 0o755 }
    }

    fn from_mode(mode: u32) -> Self {
        let mut permissions = fs::metadata(".")
            .expect("the current directory has permissions")
            .permissions();
        permissions.set_readonly(mode & 0o222 == 0);
        permissions
    }
}

#[cfg(windows)]
pub(super) trait DirBuilderExt {
    fn mode(&mut self, mode: u32) -> &mut Self;
}

#[cfg(windows)]
impl DirBuilderExt for fs::DirBuilder {
    fn mode(&mut self, _mode: u32) -> &mut Self {
        self
    }
}

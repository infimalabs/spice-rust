//! The app ZIP linked into this executable.
//!
//! The release target build and `spice dev xtask -- install` link one
//! relocatable object that holds the staged archive in its own read-only
//! section, `__TEXT,spice_app_zip` on Mach-O and `.spice_app_zip` on ELF, and
//! its length beside it. Linked before symbols, stripping, and signing, the
//! bytes survive `strip -S` and sit inside the signed image; linker symbols
//! expose them as an ordinary static, so nothing parses the executable at run
//! time.
//! Only those builds resolve this crate, so every other build links nothing.

unsafe extern "C" {
    /// First byte of the app ZIP section.
    static spice_app_zip: u8;
    /// Byte length of the app ZIP section.
    static spice_app_zip_len: u64;
}

/// The archive bytes this executable was linked with.
#[must_use]
pub fn archive() -> &'static [u8] {
    // SAFETY: the object xtask links defines both symbols: an immutable
    // section of `spice_app_zip_len` bytes starting at `spice_app_zip`, live
    // for the whole process.
    unsafe {
        let length = usize::try_from(spice_app_zip_len)
            .expect("the linked archive length fits this address space");
        std::slice::from_raw_parts(&raw const spice_app_zip, length)
    }
}

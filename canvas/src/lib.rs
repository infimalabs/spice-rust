//! The canvas archive linked into this executable.
//!
//! The release target build and `spice dev xtask -- install` link one
//! relocatable object that holds the staged archive in its own read-only
//! section, `__TEXT,__spice_canvas` on Mach-O and `.spice_canvas` on ELF, and
//! its length beside it. Linked before symbols, stripping, and signing, the
//! bytes survive `strip -S` and sit inside the signed image; the process reads
//! them as an ordinary static, and nothing parses the executable at run time.
//! Only those builds resolve this crate, so every other build links nothing.

unsafe extern "C" {
    /// First byte of the archive section.
    static spice_canvas_archive: u8;
    /// Byte length of the archive section.
    static spice_canvas_archive_len: u64;
}

/// The archive bytes this executable was linked with.
#[must_use]
pub fn archive() -> &'static [u8] {
    // SAFETY: the object xtask links defines both symbols: an immutable
    // section of `spice_canvas_archive_len` bytes starting at
    // `spice_canvas_archive`, live for the whole process.
    unsafe {
        let length = usize::try_from(spice_canvas_archive_len)
            .expect("the linked archive length fits this address space");
        std::slice::from_raw_parts(&raw const spice_canvas_archive, length)
    }
}

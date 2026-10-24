//! The spice rack linked into this executable.
//!
//! The release target build and `spice dev xtask -- install` link one
//! relocatable object that holds the assembled rack archive in its own
//! read-only section, `__TEXT,__spicerack` on Mach-O, `spicerack` on ELF, and
//! `spicerac` on PE32+, whose section names hold eight bytes, and its length
//! beside it. Linked before symbols, stripping, and signing, the bytes survive
//! `strip` and sit inside the signed image; linker symbols expose them as an
//! ordinary static, so nothing parses the executable at run time.
//! Only those builds resolve this crate, so every other build links nothing.

unsafe extern "C" {
    /// First byte of the rack section.
    static spicerack: u8;
    /// Byte length of the rack section.
    static spicerack_len: u64;
}

/// The rack archive bytes this executable was linked with.
#[must_use]
pub fn archive() -> &'static [u8] {
    // SAFETY: the object xtask links defines both symbols: an immutable
    // section of `spicerack_len` bytes starting at `spicerack`, live for the
    // whole process.
    unsafe {
        let length = usize::try_from(spicerack_len)
            .expect("the linked archive length fits this address space");
        std::slice::from_raw_parts(&raw const spicerack, length)
    }
}

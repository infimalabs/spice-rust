//! The app ZIP linked into one appliance executable.
//!
//! The release target build and `spice dev xtask -- install` link one object
//! that places the staged archive in a retained read-only section. A build
//! reads that section back out of its final image, after symbols, stripping,
//! and signing, and refuses unless it is exactly the archive it staged. The
//! archive inside the section is a standard ZIP whose offsets are relative to
//! its own start; the executable around it is not.
mod coff;

use wire::{Digest, Domain};

/// Segment holding the app ZIP section on Mach-O.
const MACHO_SEGMENT: &[u8] = b"__TEXT";
/// App ZIP section on Mach-O: lowercase after two underscores, as Apple's
/// Mach-O File Format Reference names sections.
const MACHO_SECTION: &[u8] = b"__spice_app_zip";
/// App ZIP section on ELF: undotted, since the System V gABI reserves
/// dot-prefixed section names for the system.
const ELF_SECTION: &[u8] = b"spice_app_zip";
/// First ZIP byte, as the `canvas` crate declares it.
const ARCHIVE_SYMBOL: &[u8] = b"spice_app_zip";
/// The ZIP length as a little-endian `u64`, as the `canvas` crate declares it.
const LENGTH_SYMBOL: &[u8] = b"spice_app_zip_len";

const MACHO_MAGIC_64: u32 = 0xfeed_facf;
const MACHO_HEADER_BYTES: usize = 32;
const LC_SEGMENT_64: u32 = 0x19;
const LC_SYMTAB: u32 = 0x2;
const LC_DYSYMTAB: u32 = 0xb;
const LC_BUILD_VERSION: u32 = 0x32;
const SEGMENT_COMMAND_BYTES: usize = 72;
const SECTION_64_BYTES: usize = 80;
const ELF_MAGIC: &[u8] = b"\x7fELF";
const ELF_CLASS_64: u8 = 2;
const ELF_LITTLE_ENDIAN: u8 = 1;
const ELF_SECTION_HEADER_BYTES: u64 = 64;
const SHT_PROGBITS: u32 = 1;
const SHT_SYMTAB: u32 = 2;
const SHT_STRTAB: u32 = 3;
const SHT_NOBITS: u32 = 8;
const SHF_ALLOC: u64 = 2;

/// An executable target the app ZIP object is written for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectTarget {
    MacAarch64,
    MacX86_64,
    LinuxAarch64,
    LinuxX86_64,
    WindowsX86_64,
}

impl ObjectTarget {
    /// The target `os` and `arch` name, spelled as [`std::env::consts`] spells them.
    #[must_use]
    pub fn new(os: &str, arch: &str) -> Option<Self> {
        match (os, arch) {
            ("macos", "aarch64") => Some(Self::MacAarch64),
            ("macos", "x86_64") => Some(Self::MacX86_64),
            ("linux", "aarch64") => Some(Self::LinuxAarch64),
            ("linux", "x86_64") => Some(Self::LinuxX86_64),
            ("windows", "x86_64") => Some(Self::WindowsX86_64),
            _ => None,
        }
    }

    /// The name this target gives its object file.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::MacAarch64 => "macos-aarch64",
            Self::MacX86_64 => "macos-x86_64",
            Self::LinuxAarch64 => "linux-aarch64",
            Self::LinuxX86_64 => "linux-x86_64",
            Self::WindowsX86_64 => "windows-x86_64",
        }
    }

    /// The object-file extension this target's linker expects.
    #[must_use]
    pub const fn object_extension(self) -> &'static str {
        match self {
            Self::WindowsX86_64 => "obj",
            _ => "o",
        }
    }
}

/// The relocatable object that links `archive` into one executable for `target`.
///
/// The app ZIP section holds the archive bytes alone, so the final image's
/// section is byte-for-byte what [`Canvas::verify_linked`] compares; its
/// length sits in a read-only constant section. Both are named by global data
/// symbols the `canvas` crate declares, which is what keeps the linker from
/// discarding them.
#[must_use]
pub fn canvas_object(archive: &[u8], target: ObjectTarget) -> Vec<u8> {
    match target {
        // The oldest macOS each architecture's Rust target supports, so the
        // linker never sees an object newer than the executable it links.
        ObjectTarget::MacAarch64 => macho_object(archive, 0x0100_000c, 0, 0x000b_0000),
        ObjectTarget::MacX86_64 => macho_object(archive, 0x0100_0007, 3, 0x000a_0c00),
        ObjectTarget::LinuxAarch64 => elf_object(archive, 183),
        ObjectTarget::LinuxX86_64 => elf_object(archive, 62),
        ObjectTarget::WindowsX86_64 => coff::object(archive),
    }
}

fn macho_object(archive: &[u8], cpu: u32, subtype: u32, minos: u32) -> Vec<u8> {
    const COMMANDS: usize = SEGMENT_COMMAND_BYTES + 2 * SECTION_64_BYTES + 24 + 24 + 80;
    let data = MACHO_HEADER_BYTES + COMMANDS;
    let length_at = archive.len().next_multiple_of(8);
    let segment = length_at + 8;
    let symbols = data + segment;
    let strings = [&b"\0_"[..], ARCHIVE_SYMBOL, b"\0_", LENGTH_SYMBOL, b"\0"].concat();
    let strings_at = symbols + 2 * 16;
    let string_bytes = strings.len().next_multiple_of(8);

    let mut out = Vec::with_capacity(strings_at + string_bytes);
    put32(
        &mut out,
        &[MACHO_MAGIC_64, cpu, subtype, 1, 4, word(COMMANDS), 0, 0],
    );
    put32(
        &mut out,
        &[
            LC_SEGMENT_64,
            word(SEGMENT_COMMAND_BYTES + 2 * SECTION_64_BYTES),
        ],
    );
    out.extend_from_slice(&[0; 16]);
    put64(&mut out, &[0, wide(segment), wide(data), wide(segment)]);
    put32(&mut out, &[7, 7, 2, 0]);
    for (name, address, size, align) in [
        (MACHO_SECTION, 0, archive.len(), 0),
        (&b"__const"[..], length_at, 8, 3),
    ] {
        out.extend_from_slice(&padded_name(name));
        out.extend_from_slice(&padded_name(MACHO_SEGMENT));
        put64(&mut out, &[wide(address), wide(size)]);
        put32(&mut out, &[word(data + address), align, 0, 0, 0, 0, 0, 0]);
    }
    put32(&mut out, &[LC_BUILD_VERSION, 24, 1, minos, 0, 0]);
    put32(
        &mut out,
        &[
            LC_SYMTAB,
            24,
            word(symbols),
            2,
            word(strings_at),
            word(string_bytes),
        ],
    );
    put32(&mut out, &[LC_DYSYMTAB, 80, 0, 0, 0, 2, 2, 0]);
    out.extend_from_slice(&[0; 48]);
    out.extend_from_slice(archive);
    out.resize(data + length_at, 0);
    put64(&mut out, &[wide(archive.len())]);
    // N_SECT | N_EXT, in section 1 at its start and section 2 at its address.
    for (name, section, value) in [(1, 1_u8, 0), (ARCHIVE_SYMBOL.len() + 3, 2, length_at)] {
        put32(&mut out, &[word(name)]);
        out.extend_from_slice(&[0x0f, section, 0, 0]);
        put64(&mut out, &[wide(value)]);
    }
    out.extend_from_slice(&strings);
    out.resize(strings_at + string_bytes, 0);
    out
}

fn elf_object(archive: &[u8], machine: u16) -> Vec<u8> {
    let (section_names, section_offsets) = string_table(&[
        ELF_SECTION,
        b".rodata.spice_app_zip_len",
        b".note.GNU-stack",
        b".symtab",
        b".strtab",
        b".shstrtab",
    ]);
    let (symbol_names, symbol_offsets) = string_table(&[ARCHIVE_SYMBOL, LENGTH_SYMBOL]);
    let length_at = (64 + archive.len()).next_multiple_of(8);
    let symbols_at = length_at + 8;
    let symbol_names_at = symbols_at + 3 * 24;
    let section_names_at = symbol_names_at + symbol_names.len();
    let headers_at = (section_names_at + section_names.len()).next_multiple_of(8);

    let mut out = Vec::with_capacity(headers_at + 7 * 64);
    out.extend_from_slice(ELF_MAGIC);
    out.extend_from_slice(&[ELF_CLASS_64, ELF_LITTLE_ENDIAN, 1]);
    out.resize(16, 0);
    // A relocatable object for `machine`.
    for half in [1, machine] {
        out.extend_from_slice(&half.to_le_bytes());
    }
    put32(&mut out, &[1]);
    put64(&mut out, &[0, 0, wide(headers_at)]);
    put32(&mut out, &[0]);
    // Header, no program headers, seven section headers, names in the last.
    for half in [64_u16, 0, 0, 64, 7, 6] {
        out.extend_from_slice(&half.to_le_bytes());
    }
    out.extend_from_slice(archive);
    out.resize(length_at, 0);
    put64(&mut out, &[wide(archive.len())]);
    out.resize(symbols_at + 24, 0);
    for (name, section, size) in [
        (symbol_offsets[0], 1_u16, archive.len()),
        (symbol_offsets[1], 2, 8),
    ] {
        put32(&mut out, &[name]);
        // STB_GLOBAL, STT_OBJECT, default visibility.
        out.extend_from_slice(&[0x11, 0]);
        out.extend_from_slice(&section.to_le_bytes());
        put64(&mut out, &[0, wide(size)]);
    }
    out.extend_from_slice(&symbol_names);
    out.extend_from_slice(&section_names);
    out.resize(headers_at + 64, 0);
    let sections = [
        (SHT_PROGBITS, SHF_ALLOC, 64, archive.len(), 0, 0, 1, 0),
        (SHT_PROGBITS, SHF_ALLOC, length_at, 8, 0, 0, 8, 0),
        // Without this note GNU linkers infer an executable stack.
        (SHT_PROGBITS, 0, symbols_at, 0, 0, 0, 1, 0),
        (SHT_SYMTAB, 0, symbols_at, 3 * 24, 5, 1, 8, 24),
        (
            SHT_STRTAB,
            0,
            symbol_names_at,
            symbol_names.len(),
            0,
            0,
            1,
            0,
        ),
        (
            SHT_STRTAB,
            0,
            section_names_at,
            section_names.len(),
            0,
            0,
            1,
            0,
        ),
    ];
    for ((kind, flags, offset, size, link, info, align, entry), name) in
        sections.into_iter().zip(section_offsets)
    {
        put32(&mut out, &[name, kind]);
        put64(&mut out, &[flags, 0, wide(offset), wide(size)]);
        put32(&mut out, &[link, info]);
        put64(&mut out, &[align, entry]);
    }
    out
}

/// A NUL-led string table and each name's offset in it.
fn string_table<const N: usize>(names: &[&[u8]; N]) -> (Vec<u8>, [u32; N]) {
    let mut table = vec![0];
    let offsets = names.map(|name| {
        let offset = word(table.len());
        table.extend_from_slice(name);
        table.push(0);
        offset
    });
    (table, offsets)
}

fn padded_name(name: &[u8]) -> [u8; 16] {
    let mut field = [0; 16];
    field[..name.len()].copy_from_slice(name);
    field
}

fn put32(out: &mut Vec<u8>, values: &[u32]) {
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

fn put64(out: &mut Vec<u8>, values: &[u64]) {
    for value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

fn word(value: usize) -> u32 {
    u32::try_from(value).expect("an app ZIP object offset fits in 32 bits")
}

fn wide(value: usize) -> u64 {
    value as u64
}

/// The staged app ZIP one build links: generation G and its exact bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Canvas {
    generation: Digest,
    archive: Digest,
    bytes: u64,
}

impl Canvas {
    /// Name the archive bytes a build admitted as `generation`.
    #[must_use]
    pub fn new(generation: Digest, archive: &[u8]) -> Self {
        Self {
            generation,
            archive: Digest::compute(Domain::DistArtifact, archive),
            bytes: archive.len() as u64,
        }
    }

    /// The app generation the archive carries.
    #[must_use]
    pub const fn generation(&self) -> &Digest {
        &self.generation
    }

    /// Identity of the exact archive bytes.
    #[must_use]
    pub const fn archive(&self) -> &Digest {
        &self.archive
    }

    /// Length of the archive, and so of the executable growth it causes.
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Refuse a final executable whose app ZIP section is not exactly this archive.
    ///
    /// # Errors
    ///
    /// Names an image that is not a supported 64-bit Mach-O, ELF, or PE image,
    /// a malformed header, an absent or repeated section, or different bytes.
    pub fn verify_linked(&self, executable: &[u8]) -> Result<(), CanvasError> {
        let observed = Digest::compute(Domain::DistArtifact, linked_archive(executable)?);
        if observed == self.archive {
            Ok(())
        } else {
            Err(CanvasError::Mismatch {
                expected: self.archive.clone(),
                observed,
            })
        }
    }

    /// Refuse a final executable whose version report does not name this generation.
    ///
    /// # Errors
    ///
    /// Names the report when its last word is not `app G`.
    pub fn verify_report(&self, report: &str) -> Result<(), CanvasError> {
        let named = report
            .trim_end()
            .rsplit_once(" app ")
            .is_some_and(|(_, generation)| generation == self.generation.as_str());
        if named {
            Ok(())
        } else {
            Err(CanvasError::Report(report.to_owned()))
        }
    }
}

/// Why a final executable's app ZIP section was refused.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum CanvasError {
    #[error("executable is neither a supported 64-bit Mach-O, ELF, nor PE image")]
    Format,
    #[error("executable {0} is malformed")]
    Malformed(&'static str),
    #[error("executable carries no app ZIP section")]
    Absent,
    #[error("executable carries more than one app ZIP section")]
    Repeated,
    #[error("executable app ZIP section {observed} differs from the staged archive {expected}")]
    Mismatch { expected: Digest, observed: Digest },
    #[error("executable version report does not name its app generation: {0:?}")]
    Report(String),
}

/// The archive bytes inside a final executable's app ZIP section.
///
/// # Errors
///
/// Refuses as [`Canvas::verify_linked`] does, short of comparing bytes.
pub fn linked_archive(executable: &[u8]) -> Result<&[u8], CanvasError> {
    if read_u32(executable, 0).ok() == Some(MACHO_MAGIC_64) {
        return macho(executable);
    }
    if executable.starts_with(ELF_MAGIC) {
        return elf(executable);
    }
    if executable.starts_with(b"MZ") {
        return coff::pe_archive(executable);
    }
    Err(CanvasError::Format)
}

fn macho(image: &[u8]) -> Result<&[u8], CanvasError> {
    let commands = read_u32(image, 16)?;
    let mut command = MACHO_HEADER_BYTES;
    let mut found = None;
    for _ in 0..commands {
        let kind = read_u32(image, command)?;
        let size = read_u32(image, command + 4)? as usize;
        if size < 8 {
            return Err(CanvasError::Malformed("load command"));
        }
        if kind == LC_SEGMENT_64 {
            let sections = read_u32(image, command + 64)? as usize;
            let span = sections
                .checked_mul(SECTION_64_BYTES)
                .and_then(|bytes| bytes.checked_add(SEGMENT_COMMAND_BYTES))
                .filter(|span| *span <= size)
                .ok_or(CanvasError::Malformed("segment command"))?;
            for header in
                (command + SEGMENT_COMMAND_BYTES..command + span).step_by(SECTION_64_BYTES)
            {
                if name(image, header)? == MACHO_SECTION
                    && name(image, header + 16)? == MACHO_SEGMENT
                {
                    let bytes = read_u64(image, header + 40)?;
                    let offset = u64::from(read_u32(image, header + 48)?);
                    record(&mut found, range(image, offset, bytes)?)?;
                }
            }
        }
        command = command
            .checked_add(size)
            .ok_or(CanvasError::Malformed("load command"))?;
    }
    found.ok_or(CanvasError::Absent)
}

fn elf(image: &[u8]) -> Result<&[u8], CanvasError> {
    if image.get(4) != Some(&ELF_CLASS_64) || image.get(5) != Some(&ELF_LITTLE_ENDIAN) {
        return Err(CanvasError::Format);
    }
    let table = read_u64(image, 0x28)?;
    let entry = u64::from(read_u16(image, 0x3a)?);
    let count = u64::from(read_u16(image, 0x3c)?);
    let names = u64::from(read_u16(image, 0x3e)?);
    if entry != ELF_SECTION_HEADER_BYTES || count == 0 || names >= count {
        return Err(CanvasError::Malformed("section header table"));
    }
    let header = |index: u64| -> Result<usize, CanvasError> {
        index
            .checked_mul(ELF_SECTION_HEADER_BYTES)
            .and_then(|offset| offset.checked_add(table))
            .and_then(|offset| usize::try_from(offset).ok())
            .ok_or(CanvasError::Malformed("section header table"))
    };
    let names = header(names)?;
    let names = range(
        image,
        read_u64(image, names + 24)?,
        read_u64(image, names + 32)?,
    )?;
    let mut found = None;
    for index in 0..count {
        let section = header(index)?;
        let start = read_u32(image, section)? as usize;
        let name = names
            .get(start..)
            .and_then(|tail| tail.split(|byte| *byte == 0).next())
            .ok_or(CanvasError::Malformed("section name"))?;
        if name == ELF_SECTION {
            if read_u32(image, section + 4)? == SHT_NOBITS {
                return Err(CanvasError::Malformed(
                    "app ZIP section holds no file bytes",
                ));
            }
            let offset = read_u64(image, section + 24)?;
            let bytes = read_u64(image, section + 32)?;
            record(&mut found, range(image, offset, bytes)?)?;
        }
    }
    found.ok_or(CanvasError::Absent)
}

fn record<'a>(found: &mut Option<&'a [u8]>, section: &'a [u8]) -> Result<(), CanvasError> {
    if found.replace(section).is_some() {
        Err(CanvasError::Repeated)
    } else {
        Ok(())
    }
}

/// One fixed 16-byte Mach-O name, without its NUL padding.
fn name(image: &[u8], offset: usize) -> Result<&[u8], CanvasError> {
    let field = field(image, offset, 16)?;
    Ok(field.split(|byte| *byte == 0).next().unwrap_or(field))
}

fn range(image: &[u8], offset: u64, bytes: u64) -> Result<&[u8], CanvasError> {
    let start = usize::try_from(offset).map_err(|_| CanvasError::Malformed("section range"))?;
    let length = usize::try_from(bytes).map_err(|_| CanvasError::Malformed("section range"))?;
    start
        .checked_add(length)
        .and_then(|end| image.get(start..end))
        .ok_or(CanvasError::Malformed("section range"))
}

fn field(image: &[u8], offset: usize, length: usize) -> Result<&[u8], CanvasError> {
    offset
        .checked_add(length)
        .and_then(|end| image.get(offset..end))
        .ok_or(CanvasError::Malformed("header"))
}

fn read_u16(image: &[u8], offset: usize) -> Result<u16, CanvasError> {
    let bytes = field(image, offset, 2)?;
    Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
}

fn read_u32(image: &[u8], offset: usize) -> Result<u32, CanvasError> {
    let bytes = field(image, offset, 4)?;
    Ok(u32::from_le_bytes(
        bytes.try_into().expect("the field is four bytes"),
    ))
}

fn read_u64(image: &[u8], offset: usize) -> Result<u64, CanvasError> {
    let bytes = field(image, offset, 8)?;
    Ok(u64::from_le_bytes(
        bytes.try_into().expect("the field is eight bytes"),
    ))
}

//! Bounds this crate is compiled against, read from the declaration itself.
//!
//! `spice.toml` states the project depth law once. This module reads that
//! statement at compile time instead of restating it, so the classifier and the
//! declaration cannot drift: there is only one of them. Changing the declared
//! number alone changes what the classifier accepts, with no Rust edit.
//!
//! The declaration is baked in by [`include_str!`], so the crate still performs
//! no I/O, and the compiler rebuilds this module when the file changes.
//!
//! A declaration this module cannot read fails the build rather than falling
//! back to a number nobody wrote.

/// The declaration every bound below is read out of.
const DECLARATION: &str = include_str!("../../spice.toml");

/// The exact spelling that separates a declared key from its value.
const ASSIGNMENT: &[u8] = b" = ";

/// Fewest dotted segments a public project may carry.
pub const PROJECT_MIN_DEPTH: usize = declared_depth("project_min_depth");

/// Most dotted segments a public project may carry.
pub const PROJECT_MAX_DEPTH: usize = declared_depth("project_max_depth");

/// A range the classifier can answer with is the only one worth compiling.
///
/// `split` always yields at least one segment, so a floor below one would
/// describe a depth no project can have, and an inverted pair would refuse
/// every project while looking like it declared something.
const _: () = assert!(
    PROJECT_MIN_DEPTH >= 1 && PROJECT_MIN_DEPTH <= PROJECT_MAX_DEPTH,
    "spice.toml must declare project_min_depth between 1 and project_max_depth",
);

/// Read one `key = <integer>` declaration out of [`DECLARATION`].
///
/// This is deliberately not a TOML parser. It resolves exactly the shape these
/// bounds are declared in — a key opening a line, followed by [`ASSIGNMENT`]
/// and a decimal integer — and refuses every other shape at compile time. A key
/// that only looks like the one asked for, such as a longer name sharing its
/// prefix, fails the assignment check and the scan continues past it.
const fn declared_depth(key: &str) -> usize {
    let declaration = DECLARATION.as_bytes();
    let key = key.as_bytes();
    let mut at = 0;
    while at < declaration.len() {
        if opens_line(declaration, at)
            && matches_at(declaration, at, key)
            && matches_at(declaration, at + key.len(), ASSIGNMENT)
        {
            return decimal_at(declaration, at + key.len() + ASSIGNMENT.len());
        }
        at += 1;
    }
    panic!("spice.toml must declare this bound as `<key> = <integer>` at the start of a line");
}

/// Whether this position is the first byte of a line.
const fn opens_line(declaration: &[u8], at: usize) -> bool {
    at == 0 || declaration[at - 1] == b'\n'
}

/// Whether `expected` appears at exactly this position.
const fn matches_at(declaration: &[u8], at: usize, expected: &[u8]) -> bool {
    if at + expected.len() > declaration.len() {
        return false;
    }
    let mut offset = 0;
    while offset < expected.len() {
        if declaration[at + offset] != expected[offset] {
            return false;
        }
        offset += 1;
    }
    true
}

/// Read the decimal integer starting at this position.
const fn decimal_at(declaration: &[u8], at: usize) -> usize {
    let mut at = at;
    let mut value = 0;
    let mut digits = 0;
    while at < declaration.len() && declaration[at].is_ascii_digit() {
        value = value * 10 + (declaration[at] - b'0') as usize;
        at += 1;
        digits += 1;
    }
    assert!(
        digits > 0,
        "a declared depth bound in spice.toml must be a decimal integer",
    );
    value
}

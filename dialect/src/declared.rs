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

/// Table that owns the public-project depth law.
const PROJECT_TABLE: &str = "tasks";

/// Fewest dotted segments a public project may carry.
pub const PROJECT_MIN_DEPTH: usize =
    declared_depth(DECLARATION, PROJECT_TABLE, "project_min_depth");

/// Most dotted segments a public project may carry.
pub const PROJECT_MAX_DEPTH: usize =
    declared_depth(DECLARATION, PROJECT_TABLE, "project_max_depth");

/// A range the classifier can answer with is the only one worth compiling.
///
/// A public project needs a stem and a child, so a floor below two would admit
/// the undifferentiated stem the declaration exists to refuse. Keeping the
/// floor at two also leaves one validly spelled depth below it, making the
/// classifier's lower-bound refusal observable rather than conflating it with
/// an empty-segment refusal. An inverted pair would refuse every project while
/// looking like it declared something.
const _: () = assert!(
    PROJECT_MIN_DEPTH >= 2 && PROJECT_MIN_DEPTH <= PROJECT_MAX_DEPTH,
    "spice.toml must declare project_min_depth between 2 and project_max_depth",
);

/// Read one `key = <integer>` declaration from its named table.
///
/// This is deliberately not a TOML parser. It resolves exactly the shape these
/// bounds are declared in: an exact `[table]` header, then a key opening a line,
/// followed by [`ASSIGNMENT`] and a decimal integer. A same-named key at the
/// root or in another table is not this declaration and is skipped. A key that
/// only looks like the one asked for, such as a longer name sharing its prefix,
/// fails the assignment check and the scan continues past it.
const fn declared_depth(declaration: &str, table: &str, key: &str) -> usize {
    let declaration = declaration.as_bytes();
    let table = table.as_bytes();
    let key = key.as_bytes();
    let mut at = 0;
    let mut in_table = false;
    while at < declaration.len() {
        if opens_line(declaration, at) {
            if declaration[at] == b'[' {
                in_table = is_table_header(declaration, at, table);
            } else if in_table
                && matches_at(declaration, at, key)
                && matches_at(declaration, at + key.len(), ASSIGNMENT)
            {
                return decimal_at(declaration, at + key.len() + ASSIGNMENT.len());
            }
        }
        at += 1;
    }
    panic!("spice.toml must declare this bound as `<key> = <integer>` inside its named table");
}

/// Whether this line is exactly the requested table header.
const fn is_table_header(declaration: &[u8], at: usize, table: &[u8]) -> bool {
    matches_at(declaration, at, b"[")
        && matches_at(declaration, at + 1, table)
        && matches_at(declaration, at + 1 + table.len(), b"]")
        && closes_header(declaration, at + table.len() + 2)
}

/// Whether only TOML whitespace or a comment follows a table header.
const fn closes_header(declaration: &[u8], mut at: usize) -> bool {
    while at < declaration.len() && (declaration[at] == b' ' || declaration[at] == b'\t') {
        at += 1;
    }
    at == declaration.len()
        || declaration[at] == b'\n'
        || declaration[at] == b'\r'
        || declaration[at] == b'#'
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

#[cfg(test)]
mod tests {
    use super::declared_depth;

    const TASK_DEPTH: usize = 2;

    #[test]
    fn a_declared_key_is_resolved_only_inside_its_named_table() {
        let declaration = "project_min_depth = 91\n\
                           [other]\n\
                           project_min_depth = 17\n\
                           [tasks]\n\
                           project_min_depth = 2\n";

        assert_eq!(
            declared_depth(declaration, "tasks", "project_min_depth"),
            TASK_DEPTH,
        );
    }
}

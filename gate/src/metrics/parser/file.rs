use super::{ParseError, canonical_path};

/// Native line/byte counts for one source file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileShapeMeasurement {
    path: String,
    lines: u64,
    bytes: u64,
}

impl FileShapeMeasurement {
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub const fn lines(&self) -> u64 {
        self.lines
    }

    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }
}

/// Native decoded-character count for one repository-truth document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RepositoryDocumentMeasurement {
    path: String,
    characters: u64,
}

impl RepositoryDocumentMeasurement {
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub const fn characters(&self) -> u64 {
        self.characters
    }
}

/// Measure predecessor-equivalent source lines and exact file bytes.
///
/// # Errors
///
/// Binary and invalid UTF-8 source inputs are explicit unsupported states;
/// they never become a zero-line measurement.
pub fn measure_file_shape(path: &str, bytes: &[u8]) -> Result<FileShapeMeasurement, ParseError> {
    let path = canonical_path(path)?;
    if bytes.contains(&0) {
        return Err(ParseError::unsupported(
            "policy.file_shape.source_suffixes",
            format!("selected source {path:?} contains a NUL byte"),
            "remove the path from the source file class or replace it with UTF-8 text",
        ));
    }
    let text = std::str::from_utf8(bytes).map_err(|error| {
        ParseError::unsupported(
            "policy.file_shape.source_suffixes",
            format!("selected source {path:?} is not UTF-8: {error}"),
            "remove the path from the source file class or encode it as UTF-8",
        )
    })?;
    Ok(FileShapeMeasurement {
        path,
        lines: python_splitlines_count(text),
        bytes: u64::try_from(bytes.len()).expect("file byte length fits in u64"),
    })
}

/// Measure Python-compatible decoded character count for a doctrine document.
///
/// Invalid UTF-8 sequences become one replacement scalar each, matching the
/// predecessor's `errors="replace"`; NUL-bearing binary documents are refused.
pub fn measure_repository_document(
    path: &str,
    bytes: &[u8],
) -> Result<RepositoryDocumentMeasurement, ParseError> {
    let path = canonical_path(path)?;
    if bytes.contains(&0) {
        return Err(ParseError::unsupported(
            "policy.repo_truth.docs",
            format!("selected repository document {path:?} contains a NUL byte"),
            "remove the binary path from repository-truth docs or replace it with text",
        ));
    }
    let characters = String::from_utf8_lossy(bytes).chars().count();
    Ok(RepositoryDocumentMeasurement {
        path,
        characters: u64::try_from(characters).expect("decoded character count fits in u64"),
    })
}

fn python_splitlines_count(text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    let mut count = 0_u64;
    let mut characters = text.chars().peekable();
    let mut ended = false;
    while let Some(character) = characters.next() {
        ended = line_boundary(character);
        if character == '\r' && characters.peek() == Some(&'\n') {
            characters.next();
            ended = true;
        }
        if ended {
            count += 1;
        }
    }
    if ended { count } else { count + 1 }
}

const fn line_boundary(character: char) -> bool {
    matches!(
        character,
        '\n' | '\r'
            | '\u{000b}'
            | '\u{000c}'
            | '\u{001c}'
            | '\u{001d}'
            | '\u{001e}'
            | '\u{0085}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

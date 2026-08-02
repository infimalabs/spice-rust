use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::{ParseError, canonical_path};

const LIZARD_COLUMNS: usize = 11;
const NLOC: usize = 0;
const CCN: usize = 1;
const LENGTH: usize = 4;
const PATH: usize = 6;
const FUNCTION: usize = 7;

/// One strict row from the pinned `lizard --csv` grammar.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ComplexityRecord {
    path: String,
    function: String,
    ccn: u64,
    length: u64,
    nloc: u64,
}

impl ComplexityRecord {
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub fn function(&self) -> &str {
        &self.function
    }

    #[must_use]
    pub const fn ccn(&self) -> u64 {
        self.ccn
    }

    #[must_use]
    pub const fn length(&self) -> u64 {
        self.length
    }

    #[must_use]
    pub const fn nloc(&self) -> u64 {
        self.nloc
    }
}

/// Strictly parse bounded output from the sole compiled lizard invocation.
///
/// # Errors
///
/// Any malformed, foreign, or duplicate row fails the complete parse. The
/// predecessor skipped those rows; current execution preserves them as a
/// parser failure so missing routine measurements cannot disappear.
pub fn parse_lizard_csv(
    root: &Path,
    selected_paths: &[String],
    stdout: &[u8],
) -> Result<Vec<ComplexityRecord>, ParseError> {
    let selected = selected_paths
        .iter()
        .map(|path| canonical_path(path))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(false)
        .from_reader(stdout);
    let mut records = Vec::new();
    let mut keys = BTreeSet::new();
    for (index, row) in reader.records().enumerate() {
        let row_number = index + 1;
        let row = row.map_err(|error| {
            ParseError::malformed(format!("lizard CSV row {row_number}"), error.to_string())
        })?;
        let record = parse_record(root, &selected, row_number, &row)?;
        let key = (record.path.clone(), record.function.clone());
        if !keys.insert(key.clone()) {
            return Err(ParseError::collision(
                format!("{}:{}", key.0, key.1),
                "lizard emitted the same path/function identity more than once",
            ));
        }
        records.push(record);
    }
    records.sort();
    Ok(records)
}

fn parse_record(
    root: &Path,
    selected: &BTreeSet<String>,
    row_number: usize,
    row: &csv::StringRecord,
) -> Result<ComplexityRecord, ParseError> {
    if row.len() != LIZARD_COLUMNS {
        return Err(ParseError::malformed(
            format!("lizard CSV row {row_number}"),
            format!("expected {LIZARD_COLUMNS} fields, observed {}", row.len()),
        ));
    }
    let path = normalize_lizard_path(root, &row[PATH])?;
    if !selected.contains(&path) {
        return Err(ParseError::malformed(
            format!("lizard CSV row {row_number}"),
            format!("path {path:?} was not in the selected invocation"),
        ));
    }
    let function = row[FUNCTION].trim();
    if function.is_empty() || function.contains(['\r', '\n']) {
        return Err(ParseError::malformed(
            format!("lizard CSV row {row_number}"),
            "function identity is empty or multiline",
        ));
    }
    Ok(ComplexityRecord {
        path,
        function: function.to_owned(),
        ccn: count(row_number, "ccn", &row[CCN])?,
        length: count(row_number, "length", &row[LENGTH])?,
        nloc: count(row_number, "nloc", &row[NLOC])?,
    })
}

fn count(row: usize, field: &str, value: &str) -> Result<u64, ParseError> {
    value.parse::<u64>().map_err(|_| {
        ParseError::malformed(
            format!("lizard CSV row {row}.{field}"),
            format!("{value:?} is not a non-negative integer"),
        )
    })
}

fn normalize_lizard_path(root: &Path, raw: &str) -> Result<String, ParseError> {
    let path = PathBuf::from(raw.trim());
    let relative = if path.is_absolute() {
        path.strip_prefix(root).map_err(|_| {
            ParseError::malformed(
                "lizard CSV path",
                format!("absolute path {raw:?} is outside root {}", root.display()),
            )
        })?
    } else {
        path.as_path()
    };
    canonical_path(&relative.to_string_lossy())
}

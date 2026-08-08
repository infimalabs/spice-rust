//! Strict bounded JSONL store with serialized atomic whole-file replacement.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
#[cfg(test)]
use std::fs::OpenOptions;
use std::io;
use std::path::{Path, PathBuf};

use atomic::Destination;
use sill::{Acquisition, LockFile, LockName};

use super::{LearningCandidate, LearningRecord, normalize_statement};

const RECORD_VERSION: u8 = 1;
const STORE_LIMIT: usize = 200;
const TOP_LIMIT: usize = 5;
const MAX_STORE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_RECORD_BYTES: usize = 8 * 1024;
const MAX_STATEMENT_CHARS: usize = 1_000;
const MAX_EVIDENCE_CHARS: usize = 240;
const MAX_TURN_IDS: usize = 16;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct LearningStore {
    directory: PathBuf,
    stem: String,
    path: PathBuf,
}

impl LearningStore {
    pub(super) fn new(directory: &Path, stem: &str) -> Result<Self, String> {
        validate_stem(stem)?;
        Ok(Self {
            directory: directory.to_path_buf(),
            stem: stem.to_owned(),
            path: directory.join(format!("{stem}.jsonl")),
        })
    }

    pub(super) fn confirm(
        &self,
        candidates: &[LearningCandidate],
        now: &str,
    ) -> Result<Vec<LearningRecord>, String> {
        if candidates.len() > super::MAX_EXTRACTED_CANDIDATES {
            return Err("learning candidate batch exceeds extraction bound".to_owned());
        }
        validate_time(now, "confirmation time")?;
        self.prepare_directory()?;
        let lock_name =
            LockName::new(format!("learning-{}", self.stem)).map_err(|error| error.to_string())?;
        let lock = LockFile::new(
            lock_name,
            self.directory.join(format!("{}.lock", self.stem)),
        );
        let _possession = match lock.try_acquire().map_err(|error| error.to_string())? {
            Acquisition::Acquired(possession) => possession,
            Acquisition::Contended => {
                return Err("learning store is busy; retry on a later completion".to_owned());
            }
        };
        let mut by_normalized = self
            .load()?
            .into_iter()
            .map(|record| (record.normalized_statement.clone(), record))
            .collect::<BTreeMap<_, _>>();
        let mut confirmed = Vec::new();
        for candidate in candidates {
            let fresh = record_for_candidate(candidate, &self.stem, now)?;
            let record = match by_normalized.get(&fresh.normalized_statement) {
                Some(previous) => LearningRecord {
                    last_confirmed_at: previous.last_confirmed_at.clone().max(now.to_owned()),
                    confirmation_count: previous
                        .confirmation_count
                        .checked_add(1)
                        .ok_or_else(|| "learning confirmation count exhausted".to_owned())?,
                    ..previous.clone()
                },
                None => fresh,
            };
            by_normalized.insert(record.normalized_statement.clone(), record.clone());
            confirmed.push(record);
        }
        let mut records = by_normalized.into_values().collect::<Vec<_>>();
        rank(&mut records);
        records.truncate(STORE_LIMIT);
        self.replace(&records)?;
        let survivors = records
            .into_iter()
            .map(|record| (record.normalized_statement.clone(), record))
            .collect::<BTreeMap<_, _>>();
        Ok(confirmed
            .into_iter()
            .filter_map(|record| survivors.get(&record.normalized_statement).cloned())
            .collect())
    }

    pub(super) fn top(&self) -> Result<Vec<LearningRecord>, String> {
        let mut records = self.load()?;
        rank(&mut records);
        records.truncate(TOP_LIMIT);
        Ok(records)
    }

    pub(super) fn load(&self) -> Result<Vec<LearningRecord>, String> {
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(format!("inspect {}: {error}", self.path.display())),
        };
        if !metadata.file_type().is_file() {
            return Err(format!(
                "learning store is not a real regular file: {}",
                self.path.display()
            ));
        }
        if metadata.len() > MAX_STORE_BYTES {
            return Err(format!(
                "learning store is {} bytes; maximum is {MAX_STORE_BYTES}",
                metadata.len()
            ));
        }
        let bytes = fs::read(&self.path)
            .map_err(|error| format!("read {}: {error}", self.path.display()))?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|error| format!("learning store is not UTF-8: {error}"))?;
        let mut records = Vec::new();
        let mut normalized = BTreeSet::new();
        for (index, line) in text.lines().enumerate() {
            let line_number = index + 1;
            if line.is_empty() {
                return Err(format!("learning store line {line_number} is empty"));
            }
            if line.len() > MAX_RECORD_BYTES {
                return Err(format!(
                    "learning store line {line_number} exceeds {MAX_RECORD_BYTES} bytes"
                ));
            }
            let record: LearningRecord = serde_json::from_str(line)
                .map_err(|error| format!("invalid learning JSON line {line_number}: {error}"))?;
            validate_record(&record, &self.stem, line_number)?;
            if !normalized.insert(record.normalized_statement.clone()) {
                return Err(format!(
                    "learning store line {line_number} repeats normalized statement"
                ));
            }
            records.push(record);
            if records.len() > STORE_LIMIT {
                return Err(format!("learning store exceeds {STORE_LIMIT} records"));
            }
        }
        if !bytes.is_empty() && !bytes.ends_with(b"\n") {
            return Err("learning store has an unterminated final record".to_owned());
        }
        Ok(records)
    }

    fn prepare_directory(&self) -> Result<(), String> {
        if let Some(runtime) = self.directory.parent() {
            fs::create_dir_all(runtime)
                .map_err(|error| format!("create {}: {error}", runtime.display()))?;
        }
        match fs::create_dir(&self.directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(format!("create {}: {error}", self.directory.display()));
            }
        }
        let metadata = fs::symlink_metadata(&self.directory)
            .map_err(|error| format!("inspect {}: {error}", self.directory.display()))?;
        if !metadata.file_type().is_dir() {
            return Err(format!(
                "learning store parent is not a real directory: {}",
                self.directory.display()
            ));
        }
        Ok(())
    }

    fn replace(&self, records: &[LearningRecord]) -> Result<(), String> {
        let mut bytes = Vec::new();
        for record in records {
            let mut line = serde_json::to_vec(record)
                .map_err(|error| format!("encode learning record: {error}"))?;
            if line.len() > MAX_RECORD_BYTES {
                return Err(format!("encoded learning exceeds {MAX_RECORD_BYTES} bytes"));
            }
            line.push(b'\n');
            bytes.extend(line);
        }
        if bytes.len() as u64 > MAX_STORE_BYTES {
            return Err(format!(
                "encoded learning store exceeds {MAX_STORE_BYTES} bytes"
            ));
        }
        let destination = Destination::new(&self.path).map_err(|error| error.to_string())?;
        destination
            .publish(|staged| std::io::Write::write_all(staged, &bytes))
            .map_err(|error| error.to_string())
    }

    #[cfg(test)]
    pub(super) fn path(&self) -> &Path {
        &self.path
    }
}

fn record_for_candidate(
    candidate: &LearningCandidate,
    fallback_stem: &str,
    now: &str,
) -> Result<LearningRecord, String> {
    let stem = if candidate.project_stem.is_empty() {
        fallback_stem
    } else {
        &candidate.project_stem
    };
    validate_stem(stem)?;
    if stem != fallback_stem {
        return Err("learning candidate project stem crosses its store".to_owned());
    }
    let statement = candidate
        .statement
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let normalized = normalize_statement(&statement)?;
    if normalized != candidate.normalized_statement {
        return Err("learning candidate normalized statement is not canonical".to_owned());
    }
    let record = LearningRecord {
        version: RECORD_VERSION,
        statement,
        normalized_statement: normalized,
        source_task: candidate.source_task.trim().to_owned(),
        project_stem: stem.to_owned(),
        evidence: candidate
            .evidence
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
        source_slice_id: candidate.source_slice_id.trim().to_owned(),
        source_turn_ids: candidate.source_turn_ids.clone(),
        created_at: now.to_owned(),
        last_confirmed_at: now.to_owned(),
        confirmation_count: 1,
    };
    validate_record(&record, fallback_stem, 0)?;
    Ok(record)
}

fn validate_record(record: &LearningRecord, stem: &str, line: usize) -> Result<(), String> {
    let at = if line == 0 {
        "candidate".to_owned()
    } else {
        format!("line {line}")
    };
    if record.version != RECORD_VERSION {
        return Err(format!(
            "learning {at} has unsupported version {}",
            record.version
        ));
    }
    if record.project_stem != stem {
        return Err(format!("learning {at} crosses project stems"));
    }
    validate_stem(&record.project_stem)?;
    if record.statement.is_empty()
        || record.statement.chars().count() > MAX_STATEMENT_CHARS
        || normalize_statement(&record.statement)? != record.normalized_statement
    {
        return Err(format!("learning {at} has an invalid canonical statement"));
    }
    if record.evidence.chars().count() > MAX_EVIDENCE_CHARS {
        return Err(format!(
            "learning {at} evidence exceeds {MAX_EVIDENCE_CHARS} characters"
        ));
    }
    if record.source_task.is_empty() || record.source_slice_id.is_empty() {
        return Err(format!("learning {at} omits exact source provenance"));
    }
    if record.source_turn_ids.len() > MAX_TURN_IDS
        || record.source_turn_ids.iter().any(|id| id.trim().is_empty())
        || record.source_turn_ids.iter().collect::<BTreeSet<_>>().len()
            != record.source_turn_ids.len()
    {
        return Err(format!("learning {at} has invalid source turn identities"));
    }
    validate_time(&record.created_at, "created_at")?;
    validate_time(&record.last_confirmed_at, "last_confirmed_at")?;
    if record.created_at > record.last_confirmed_at || record.confirmation_count == 0 {
        return Err(format!("learning {at} has invalid confirmation chronology"));
    }
    Ok(())
}

fn validate_time(value: &str, field: &str) -> Result<(), String> {
    value
        .parse::<wire::Instant>()
        .map(|_| ())
        .map_err(|error| format!("invalid learning {field}: {error}"))
}

fn rank(records: &mut [LearningRecord]) {
    records.sort_by(|left, right| {
        right
            .last_confirmed_at
            .cmp(&left.last_confirmed_at)
            .then_with(|| right.confirmation_count.cmp(&left.confirmation_count))
            .then_with(|| left.normalized_statement.cmp(&right.normalized_statement))
    });
}

pub(super) fn validate_stem(stem: &str) -> Result<(), String> {
    let valid = !stem.is_empty()
        && stem
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && stem.as_bytes()[0].is_ascii_alphanumeric();
    if !valid {
        return Err(format!("invalid learning project stem {stem:?}"));
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn write_unchecked(path: &Path, body: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("fixture learning parent");
    }
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    let mut file = options.open(path).expect("fixture learning file");
    std::io::Write::write_all(&mut file, body).expect("fixture learning bytes");
}

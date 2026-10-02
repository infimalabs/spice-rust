//! One bounded page for every session history view.
//!
//! A history view lists records in relevance order, newest first, and admits
//! them through the [`wire::output`] contract: `--limit`, `--max-bytes`, and
//! `--max-text` raise detail only up to the [`HISTORY`] maxima, every record is
//! charged its encoded length, and a page that withholds records names the
//! exact command that continues at the first of them.
//!
//! The continuation repeats the caller's own words and adds `--after CURSOR`.
//! A timestamped view's cursor is the first withheld record's instant, with
//! `#N` when N records sharing that instant were already shown; resuming reads
//! only records at or before that instant and skips those N, so records that
//! arrive later never shift a page boundary. A view whose records are not
//! timestamped names its own stable record key instead.
//!
//! `--export FILE` writes the page as one complete document to a named file
//! with its digest receipt. Its byte budget is the repository ceiling rather
//! than the interactive one, so a record too large for the terminal is still
//! reachable; a record beyond the ceiling is refused, never streamed.

use std::path::PathBuf;

use clap::ArgMatches;
use clap::parser::ValueSource;
use serde::Serialize;
use wire::output::{
    self, Admission, Bounded, Budget, CEILING, ClippedText, Export, Limits, Request, Resume,
    Surface,
};
use wire::{Refusal, SpiceCommand};

/// Defaults and maxima every session history view reads under.
pub(crate) const HISTORY: Surface = output::INTERACTIVE;

/// The same rows and text an export writes, charged against the repository
/// ceiling instead of the interactive byte budget.
const EXPORT: Surface = Surface::new(
    Limits {
        rows: HISTORY.default_limits().rows,
        bytes: CEILING.bytes,
        text: HISTORY.default_limits().text,
    },
    CEILING,
);

/// Caller words a continuation states for itself rather than repeating, and
/// the global output-form flag, which the caller chooses per invocation.
const RESTATED: [&str; 4] = ["after", "max-bytes", "export", crate::cli::JSON];

/// One parsed `--after`, `--limit`, `--max-bytes`, `--max-text`, `--export`.
pub(crate) struct Page {
    budget: Budget,
    after: Option<String>,
    bytes: Option<usize>,
    export: Option<PathBuf>,
    export_name: String,
    words: Vec<String>,
}

/// A page shown on the terminal, or written whole to the named file.
pub(crate) enum Paged<T> {
    Shown(Bounded<T>),
    Exported(Export),
}

/// The instant a timestamped page resumes at, and how many records sharing it
/// were already shown.
struct Cursor {
    ts: String,
    skip: usize,
}

impl Page {
    pub(crate) fn from_matches(matches: &ArgMatches, path: &[&str]) -> Result<Self, Refusal> {
        let request = Request {
            rows: count(matches, "limit")?,
            bytes: count(matches, "max-bytes")?,
            text: count(matches, "max-text")?,
        };
        let export = one(matches, "export").map(PathBuf::from);
        let surface = if export.is_some() { EXPORT } else { HISTORY };
        let budget = surface.budget(request).map_err(|error| {
            refusal(
                "session::bound-exceeded",
                Some(error.to_string()),
                "pass a session read bound within the surface maximum, or --export FILE for a \
                 complete page",
            )
        })?;
        Ok(Self {
            budget,
            after: one(matches, "after").cloned(),
            bytes: request.bytes,
            export,
            export_name: format!("spice-{}.json", path.join("-")),
            words: continuation_words(matches, path),
        })
    }

    pub(crate) const fn limits(&self) -> Limits {
        self.budget.limits()
    }

    /// One text field with whitespace runs collapsed, cut to the text limit.
    pub(crate) fn clip(&self, text: &str) -> ClippedText {
        clip(text, self.budget.limits().text)
    }

    /// The caller's `--after` cursor, verbatim.
    pub(crate) fn after(&self) -> Option<&str> {
        self.after.as_deref()
    }

    /// Records a keyed scan must retain so the page can close on the first
    /// record it withholds.
    pub(crate) const fn retain(&self) -> usize {
        self.budget.limits().rows.saturating_add(1)
    }

    /// The ordinal a keyed view's cursor `PREFIXN` names.
    pub(crate) fn after_ordinal(&self, prefix: &str) -> Result<Option<usize>, Refusal> {
        let Some(after) = &self.after else {
            return Ok(None);
        };
        after
            .strip_prefix(prefix)
            .and_then(|ordinal| ordinal.parse::<usize>().ok())
            .filter(|ordinal| *ordinal > 0)
            .map(Some)
            .ok_or_else(|| {
                refusal(
                    "session::cursor-invalid",
                    Some(after.clone()),
                    "pass the --after cursor exactly as a continuation named it",
                )
            })
    }

    /// The newest instant a timestamped scan considers: the caller's `--end`
    /// tightened to the cursor.
    pub(crate) fn end(&self, end: Option<&str>) -> Result<Option<String>, Refusal> {
        Ok(match (self.cursor()?, end) {
            (Some(cursor), Some(end)) if end < cursor.ts.as_str() => Some(end.to_owned()),
            (Some(cursor), _) => Some(cursor.ts),
            (None, end) => end.map(str::to_owned),
        })
    }

    /// Newest records a timestamped scan must retain so the page can close
    /// on the first record it withholds.
    pub(crate) fn frontier(&self) -> Result<usize, Refusal> {
        let skip = self.cursor()?.map_or(0, |cursor| cursor.skip);
        Ok(self
            .budget
            .limits()
            .rows
            .saturating_add(skip)
            .saturating_add(1))
    }

    /// Page newest-first timestamped records; `population` counts every
    /// record the scan matched at or before [`Page::end`], retained or not.
    pub(crate) fn newest<T: Serialize>(
        self,
        rows: Vec<T>,
        population: usize,
        ts: impl Fn(&T) -> &str,
    ) -> Result<Paged<T>, Refusal> {
        let cursor = self.cursor()?;
        let mut keyed = Vec::with_capacity(rows.len());
        let mut skipped = 0;
        let mut run: Option<(String, usize)> = None;
        for row in rows {
            let at = ts(&row).to_owned();
            let index = match &run {
                Some((previous, count)) if *previous == at => *count,
                _ => 0,
            };
            run = Some((at.clone(), index + 1));
            if cursor
                .as_ref()
                .is_some_and(|cursor| cursor.ts == at && index < cursor.skip)
            {
                skipped += 1;
                continue;
            }
            let key = if index == 0 {
                at
            } else {
                format!("{at}#{index}")
            };
            keyed.push((row, key));
        }
        self.admit(keyed, population.saturating_sub(skipped))
    }

    /// Page records already in relevance order and already past the
    /// caller's cursor, each with the key that resumes at it.
    pub(crate) fn admit<T: Serialize>(
        self,
        rows: Vec<(T, String)>,
        population: usize,
    ) -> Result<Paged<T>, Refusal> {
        let offered = rows.len();
        let mut admission = Admission::new(self.budget);
        let mut stop = None;
        for (row, key) in rows {
            let admitted = admission.offer(row).map_err(|error| {
                refusal(
                    "session::record-unencodable",
                    Some(error.to_string()),
                    "report the transcript record that cannot be encoded",
                )
            })?;
            if !admitted && stop.is_none() {
                stop = Some(key);
            }
        }
        let unretained = population.saturating_sub(offered);
        if unretained > 0 {
            if !admission.is_closed() {
                return Err(refusal(
                    "session::page-frontier-short",
                    Some(format!("population={population} retained={offered}")),
                    "report this session read; its scan retained fewer records than one page",
                ));
            }
            admission.withhold(unretained);
        }
        let bounded = admission.finish(|resume| {
            let key = stop
                .as_deref()
                .expect("a closed admission refused one keyed record");
            self.continuation(resume, key)
        });
        let Some(path) = self.export else {
            return Ok(Paged::Shown(bounded));
        };
        if bounded.rows.is_empty() && bounded.omission.is_some() {
            return Err(refusal(
                "session::record-exceeds-ceiling",
                bounded.omission.map(|omission| {
                    format!(
                        "next record {} bytes; ceiling {}",
                        omission.next_bytes, CEILING.bytes
                    )
                }),
                "narrow the record with --max-text; no read carries a record beyond the \
                 repository ceiling",
            ));
        }
        let document = wire::encode(&bounded).map_err(|error| {
            refusal(
                "session::record-unencodable",
                Some(error.to_string()),
                "report the transcript record that cannot be encoded",
            )
        })?;
        spice::export::export(&path, &document).map(Paged::Exported)
    }

    fn continuation(&self, resume: Resume<'_, impl Sized>, key: &str) -> SpiceCommand {
        let mut words = self.words.clone();
        words.extend(["--after".to_owned(), key.to_owned()]);
        let bytes = match resume {
            Resume::Widened { bytes, .. } => Some(bytes),
            Resume::At(_) | Resume::Export(_) => self.bytes,
        };
        if let Some(bytes) = bytes {
            words.extend(["--max-bytes".to_owned(), bytes.to_string()]);
        }
        if matches!(resume, Resume::Export(_)) || self.export.is_some() {
            let path = self.export.as_ref().map_or_else(
                || self.export_name.clone(),
                |path| path.display().to_string(),
            );
            words.extend(["--export".to_owned(), path]);
        }
        SpiceCommand::try_from(words).expect("a session read path is one Spice command")
    }

    fn cursor(&self) -> Result<Option<Cursor>, Refusal> {
        let Some(after) = &self.after else {
            return Ok(None);
        };
        let invalid = || {
            refusal(
                "session::cursor-invalid",
                Some(after.clone()),
                "pass the --after cursor exactly as a continuation named it: TIMESTAMP or \
                 TIMESTAMP#N",
            )
        };
        let (ts, skip) = after
            .split_once('#')
            .map_or(Ok((after.as_str(), 0)), |(ts, skip)| {
                skip.parse::<usize>()
                    .ok()
                    .filter(|skip| *skip > 0)
                    .map(|skip| (ts, skip))
                    .ok_or_else(invalid)
            })?;
        let ts = super::project::normalize_bound(ts).map_err(|_| invalid())?;
        Ok(Some(Cursor { ts, skip }))
    }
}

/// Collapse whitespace runs and keep at most `max_bytes` of the result.
pub(crate) fn clip(text: &str, max_bytes: usize) -> ClippedText {
    output::clip(
        &text.split_whitespace().collect::<Vec<_>>().join(" "),
        max_bytes,
    )
}

/// The caller's invocation, less the words a continuation restates.
fn continuation_words(matches: &ArgMatches, path: &[&str]) -> Vec<String> {
    let mut positionals = Vec::new();
    let mut options = Vec::new();
    for id in matches.ids() {
        let name = id.as_str();
        if RESTATED.contains(&name) || matches.value_source(name) != Some(ValueSource::CommandLine)
        {
            continue;
        }
        if matches.try_get_one::<bool>(name).is_ok() {
            options.push(format!("--{name}"));
            continue;
        }
        let values = matches
            .get_raw(name)
            .into_iter()
            .flatten()
            .map(|value| value.to_string_lossy().into_owned());
        if name
            .chars()
            .all(|character| !character.is_ascii_lowercase())
        {
            positionals.extend(values);
        } else {
            for value in values {
                options.extend([format!("--{name}"), value]);
            }
        }
    }
    path.iter()
        .map(|word| (*word).to_owned())
        .chain(positionals)
        .chain(options)
        .collect()
}

fn count(matches: &ArgMatches, name: &str) -> Result<Option<usize>, Refusal> {
    one(matches, name)
        .map(|value| {
            value.parse::<usize>().map_err(|_| {
                refusal(
                    "session::bound-invalid",
                    Some(format!("--{name}={value}")),
                    "pass a nonnegative decimal integer bound",
                )
            })
        })
        .transpose()
}

fn one<'a>(matches: &'a ArgMatches, name: &str) -> Option<&'a String> {
    matches.try_get_one::<String>(name).ok().flatten()
}

fn refusal(reason: &str, observed: Option<String>, detail: &str) -> Refusal {
    Refusal::bare(
        wire::RefusalDiagnostic::new(
            reason.to_owned(),
            Some("session-query".to_owned()),
            Some("one bounded session read".to_owned()),
            observed,
        )
        .with_detail(detail),
    )
}

#[cfg(test)]
mod tests;

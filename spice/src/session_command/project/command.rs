//! What a session actually ran.
//!
//! The command fold reports the shell an agent issued and audits how much
//! of it went through the recorded wrapper rather than around it, which is why
//! the pipeline reading here is structural rather than a substring search.

use std::collections::BTreeMap;

use clap::ArgMatches;
use wire::Refusal;

use super::{
    CommandAudit, CommandRecord, CommandView, DEFAULT_COMMAND_TEXT, DEFAULT_COMMANDS_LIMIT,
    Dataset, Report, clip, flag, number,
};

pub(crate) fn commands(dataset: &Dataset, matches: &ArgMatches) -> Result<Report, Refusal> {
    let limit = number(matches, "limit", DEFAULT_COMMANDS_LIMIT)?;
    let max_text = number(matches, "max-text", DEFAULT_COMMAND_TEXT)?;
    let population = dataset
        .commands
        .iter()
        .filter(|row| row.exit_code.is_some() && row.status != "running")
        .collect::<Vec<_>>();
    let latest_compaction = dataset.compactions.last().map(|row| row.ts.as_str());
    let mut filters = Vec::new();
    let mut rows = population.clone();
    if flag(matches, "since-compaction") {
        filters.push("since-compaction".to_owned());
        if let Some(ts) = latest_compaction {
            rows.retain(|row| row.ts.as_str() > ts);
        }
    }
    if flag(matches, "failed") {
        filters.push("failed".to_owned());
        rows.retain(|row| row.exit_code.is_some_and(|code| code != 0));
    }
    if flag(matches, "pipelines") {
        filters.push("pipelines".to_owned());
        rows.retain(|row| has_pipeline(&row.command));
    }
    if flag(matches, "noncanonical-pipelines") {
        filters.push("noncanonical-pipelines".to_owned());
        rows.retain(|row| noncanonical_pipeline(&row.command));
    }
    if flag(matches, "summary") {
        return Ok(Report::Commands {
            sources: dataset.sources.clone(),
            audit: Some(command_audit(population.len(), &rows, filters)),
            rows: Vec::new(),
            residues: dataset.residues.clone(),
        });
    }
    let newest = flag(matches, "newest-first");
    let selected = if newest {
        rows.into_iter().rev().take(limit).collect::<Vec<_>>()
    } else {
        rows.into_iter()
            .rev()
            .take(limit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    };
    Ok(Report::Commands {
        sources: dataset.sources.clone(),
        rows: selected
            .into_iter()
            .map(|row| command_view(row, max_text))
            .collect(),
        audit: None,
        residues: dataset.residues.clone(),
    })
}
fn command_view(row: &CommandRecord, max_text: usize) -> CommandView {
    CommandView {
        source: row.source.clone(),
        ts: row.ts.clone(),
        turn_id: row.turn_id.clone(),
        cwd: row.cwd.clone(),
        command: clip(&row.command, max_text),
        exit_code: row.exit_code,
        status: row.status.clone(),
        wrapper: starts_with_wrapper(&row.command),
        pipeline: if noncanonical_pipeline(&row.command) {
            Some("noncanonical".to_owned())
        } else if has_pipeline(&row.command) {
            Some("canonical".to_owned())
        } else {
            None
        },
    }
}

fn command_audit(population: usize, rows: &[&CommandRecord], filters: Vec<String>) -> CommandAudit {
    let wrapper = rows
        .iter()
        .filter(|row| starts_with_wrapper(&row.command))
        .count();
    let pipelines = rows.iter().filter(|row| has_pipeline(&row.command)).count();
    let canonical = rows
        .iter()
        .filter(|row| canonical_pipeline(&row.command))
        .count();
    let mut labels = BTreeMap::new();
    for row in rows
        .iter()
        .filter(|row| noncanonical_pipeline(&row.command))
    {
        *labels.entry(command_label(&row.command)).or_insert(0_usize) += 1;
    }
    let mut labels = labels.into_iter().collect::<Vec<_>>();
    labels.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    CommandAudit {
        population,
        matched: rows.len(),
        filters,
        failed: rows
            .iter()
            .filter(|row| row.exit_code.is_some_and(|code| code != 0))
            .count(),
        wrapper,
        non_wrapper: rows.len().saturating_sub(wrapper),
        pipelines,
        canonical_pipelines: canonical,
        noncanonical_pipelines: pipelines.saturating_sub(canonical),
        top_noncanonical: labels
            .into_iter()
            .take(4)
            .map(|(label, count)| format!("{label}:{count}"))
            .collect(),
    }
}

fn split_pipeline(command: &str) -> Vec<String> {
    let mut rows = Vec::new();
    let mut start = 0;
    let mut quote = None;
    let mut escaped = false;
    let bytes = command.as_bytes();
    for (index, character) in command.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if let Some(open) = quote {
            if character == open {
                quote = None;
            }
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
            continue;
        }
        if character == '|'
            && index.checked_sub(1).and_then(|offset| bytes.get(offset)) != Some(&b'|')
            && bytes.get(index + 1) != Some(&b'|')
        {
            rows.push(command[start..index].trim().to_owned());
            start = index + 1;
        }
    }
    if rows.is_empty() {
        vec![command.trim().to_owned()]
    } else {
        rows.push(command[start..].trim().to_owned());
        rows
    }
}

fn has_pipeline(command: &str) -> bool {
    split_pipeline(command).len() > 1
}

fn canonical_pipeline(command: &str) -> bool {
    let segments = split_pipeline(command);
    segments.len() > 1 && segments.iter().all(|segment| starts_with_wrapper(segment))
}

fn noncanonical_pipeline(command: &str) -> bool {
    has_pipeline(command) && !canonical_pipeline(command)
}

fn starts_with_wrapper(command: &str) -> bool {
    let words = shell_words::split(command).unwrap_or_else(|_| {
        command
            .split_whitespace()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    });
    let words = words
        .iter()
        .map(String::as_str)
        .skip_while(|word| environment_assignment(word))
        .collect::<Vec<_>>();
    (words.starts_with(&["spice", "agent", "run"])
        && (words.len() == 3 || words.get(3) == Some(&"--")))
        || (words.starts_with(&["uv", "run", "spice", "agent", "run"])
            && (words.len() == 5 || words.get(5) == Some(&"--")))
}

fn environment_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name.bytes().enumerate().all(|(index, byte)| {
            byte == b'_' || byte.is_ascii_alphabetic() || index > 0 && byte.is_ascii_digit()
        })
}

fn command_label(command: &str) -> String {
    if starts_with_wrapper(command) {
        return "spice agent run".to_owned();
    }
    shell_words::split(command)
        .unwrap_or_else(|_| command.split_whitespace().map(str::to_owned).collect())
        .into_iter()
        .find(|word| !environment_assignment(word))
        .unwrap_or_else(|| "-".to_owned())
}

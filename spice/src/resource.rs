//! The `spice://` resources a lane's omp reads board and task state through.
//!
//! Every URL names one existing CLI read: [`Request::parse`] maps it onto
//! that read's exact arguments, and [`read`] runs the read in process over
//! the lane's own connection, so authority, scoping, refusals, and rendering
//! are the CLI's. A withheld page names the URL that resumes it, and a
//! collection ends with the child URLs omp has no way to enumerate. A
//! trailing line selector is honored or refused. Reading commits nothing, so
//! omp may read as often as its tools ask; a URL naming a mutation is refused
//! with the command that performs it.

use std::path::Path;

use artifact::ArtifactAnswer;
use spice::repository::Repository;
use wire::intent::CommandAnswer;
use wire::{CanonicalUuid, Outcome, TaskHandle};

use crate::render::{self, Form};

#[cfg(test)]
mod tests;

/// The scheme every resource URL carries.
pub(crate) const SCHEME: &str = "spice://";

/// The root index: every resource shape and the CLI read it answers with.
const INDEX: &str = "\
spice:// resources: read-only views of spice CLI reads; acts on the board stay spice commands
spice://board  task status
spice://task  task list [?all | ?project=PROJECT] [&status=STATUS] [&limit=N&max-bytes=N&max-text=N&after=HANDLE]
spice://task/HANDLE  task show HANDLE
spice://task/HANDLE/context  task show HANDLE --context [?limit=N&max-bytes=N&max-text=N&after=CURSOR]
spice://task/HANDLE/history  task show HANDLE --history [same paging]
spice://task/HANDLE/detail  task show HANDLE --context --history [same paging]
spice://task/HANDLE/artifact  task artifact list HANDLE [?limit=N&max-bytes=N&after=ARTIFACT]
spice://task/HANDLE/artifact/ARTIFACT  task artifact show HANDLE ARTIFACT [?offset=N&bytes=N]
spice://quote/v1/...  a quoted message and its recorded author
Query values percent-encode reserved bytes, ':' as %3A. A trailing :N, :N-M, :N+K, :-N, or comma list selects lines; :raw is the text itself.
resources:
  spice://board
  spice://task";

/// What one `spice://` URL names: the resource and the lines selected of it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Request {
    resource: Resource,
    selector: Option<Selector>,
}

#[derive(Debug, PartialEq, Eq)]
enum Resource {
    Index,
    /// A quote target, as the URL spelled it without its selector.
    Quote(String),
    Read(Query),
}

/// One CLI read: its resource shape and the options its query named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Query {
    shape: Shape,
    options: Vec<(String, Option<String>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Shape {
    Board,
    Tasks,
    Task { handle: TaskHandle, detail: Detail },
    Artifacts { handle: TaskHandle },
    Artifact { handle: TaskHandle, id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Detail {
    Summary,
    Context,
    History,
    Both,
}

/// One query key a shape admits: the CLI flag it spells, and whether it
/// carries a value.
struct Key {
    name: &'static str,
    valued: bool,
}

const fn valued(name: &'static str) -> Key {
    Key { name, valued: true }
}

const TASK_LIST: &[Key] = &[
    Key {
        name: "all",
        valued: false,
    },
    valued("project"),
    valued("status"),
    valued("limit"),
    valued("max-bytes"),
    valued("max-text"),
    valued("after"),
];
const TASK_DETAIL: &[Key] = &[
    valued("limit"),
    valued("max-bytes"),
    valued("max-text"),
    valued("after"),
];
const ARTIFACT_LIST: &[Key] = &[valued("limit"), valued("max-bytes"), valued("after")];
const ARTIFACT_SHOW: &[Key] = &[valued("offset"), valued("bytes")];

/// The CLI reads resources serve; every other `task` leaf acts on the board.
const SERVED: &[&[&str]] = &[
    &["task", "status"],
    &["task", "list"],
    &["task", "show"],
    &["task", "artifact", "list"],
    &["task", "artifact", "show"],
];

impl Shape {
    fn admitted(&self) -> &'static [Key] {
        match self {
            Self::Board => &[],
            Self::Tasks => TASK_LIST,
            Self::Task { .. } => TASK_DETAIL,
            Self::Artifacts { .. } => ARTIFACT_LIST,
            Self::Artifact { .. } => ARTIFACT_SHOW,
        }
    }

    fn path(&self) -> String {
        match self {
            Self::Board => "board".to_owned(),
            Self::Tasks => "task".to_owned(),
            Self::Task { handle, detail } => match detail {
                Detail::Summary => format!("task/{handle}"),
                Detail::Context => format!("task/{handle}/context"),
                Detail::History => format!("task/{handle}/history"),
                Detail::Both => format!("task/{handle}/detail"),
            },
            Self::Artifacts { handle } => format!("task/{handle}/artifact"),
            Self::Artifact { handle, id } => {
                format!("task/{handle}/artifact/{}", encode(id))
            }
        }
    }

    fn command(&self) -> Vec<String> {
        let words = |words: &[&str]| words.iter().map(|word| (*word).to_owned()).collect();
        match self {
            Self::Board => words(&["task", "status"]),
            Self::Tasks => words(&["task", "list"]),
            Self::Task { handle, detail } => {
                let mut command = words(&["task", "show", handle.as_str()]);
                if matches!(detail, Detail::Context | Detail::Both) {
                    command.push("--context".to_owned());
                }
                if matches!(detail, Detail::History | Detail::Both) {
                    command.push("--history".to_owned());
                }
                command
            }
            Self::Artifacts { handle } => words(&["task", "artifact", "list", handle.as_str()]),
            Self::Artifact { handle, id } => {
                words(&["task", "artifact", "show", handle.as_str(), id])
            }
        }
    }
}

impl Query {
    /// The exact arguments after `spice` that run this read.
    pub(crate) fn arguments(&self) -> Vec<String> {
        let mut arguments = self.shape.command();
        for (key, value) in &self.options {
            arguments.push(format!("--{key}"));
            arguments.extend(value.iter().cloned());
        }
        arguments
    }

    /// The URL naming this read.
    pub(crate) fn url(&self) -> String {
        let mut url = format!("{SCHEME}{}", self.shape.path());
        for (index, (key, value)) in self.options.iter().enumerate() {
            url.push(if index == 0 { '?' } else { '&' });
            url.push_str(key);
            if let Some(value) = value {
                url.push('=');
                url.push_str(&encode(value));
            }
        }
        url
    }

    /// The read a CLI command performs, when a resource serves it.
    pub(crate) fn from_arguments(arguments: &[String]) -> Option<Self> {
        let words = arguments.iter().map(String::as_str).collect::<Vec<_>>();
        let handle = |word: &str| word.parse::<TaskHandle>().ok();
        let (mut shape, rest) = match words.as_slice() {
            ["task", "status", rest @ ..] => (Shape::Board, rest),
            ["task", "list", rest @ ..] => (Shape::Tasks, rest),
            ["task", "show", task, rest @ ..] => (
                Shape::Task {
                    handle: handle(task)?,
                    detail: Detail::Summary,
                },
                rest,
            ),
            ["task", "artifact", "list", task, rest @ ..] => (
                Shape::Artifacts {
                    handle: handle(task)?,
                },
                rest,
            ),
            ["task", "artifact", "show", task, id, rest @ ..] => (
                Shape::Artifact {
                    handle: handle(task)?,
                    id: (*id).to_owned(),
                },
                rest,
            ),
            _ => return None,
        };
        let (mut context, mut history) = (false, false);
        let mut options = Vec::new();
        let mut words = rest.iter();
        while let Some(word) = words.next() {
            let key = word.strip_prefix("--")?;
            match (&shape, key) {
                (Shape::Task { .. }, "context") => context = true,
                (Shape::Task { .. }, "history") => history = true,
                _ => {
                    let admitted = shape
                        .admitted()
                        .iter()
                        .find(|admitted| admitted.name == key)?;
                    let value = if admitted.valued {
                        Some((*words.next()?).to_owned())
                    } else {
                        None
                    };
                    options.push((key.to_owned(), value));
                }
            }
        }
        if let Shape::Task { detail, .. } = &mut shape {
            *detail = match (context, history) {
                (false, false) => Detail::Summary,
                (true, false) => Detail::Context,
                (false, true) => Detail::History,
                (true, true) => Detail::Both,
            };
        }
        Some(Self { shape, options })
    }
}

impl Request {
    /// Map one URL onto the resource it names, or say why it names none.
    pub(crate) fn parse(url: &str) -> Result<Self, String> {
        let Some(rest) = url.strip_prefix(SCHEME) else {
            return Err(format!("{url} is not a {SCHEME} URL"));
        };
        let (rest, selector) = match rest.split_once(':') {
            Some((rest, selector)) => (rest, Some(Selector::parse(selector)?)),
            None => (rest, None),
        };
        let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
        let segments = path.strip_suffix('/').unwrap_or(path);
        let segments = if segments.is_empty() {
            Vec::new()
        } else {
            segments
                .split('/')
                .map(decode)
                .collect::<Result<Vec<_>, _>>()?
        };
        let words = segments.iter().map(String::as_str).collect::<Vec<_>>();
        let shape = match words.as_slice() {
            [] if query.is_empty() => {
                return Ok(Self {
                    resource: Resource::Index,
                    selector,
                });
            }
            ["quote", ..] => {
                return Ok(Self {
                    resource: Resource::Quote(format!("{SCHEME}{rest}")),
                    selector,
                });
            }
            ["board"] => Shape::Board,
            ["task"] => Shape::Tasks,
            ["task", task, rest @ ..] => {
                refuse_mutation(&["task", task], url)?;
                let handle = task
                    .parse::<TaskHandle>()
                    .map_err(|error| format!("{url} names no task handle {task}: {error}"))?;
                match rest {
                    [] => Shape::Task {
                        handle,
                        detail: Detail::Summary,
                    },
                    ["context"] => Shape::Task {
                        handle,
                        detail: Detail::Context,
                    },
                    ["history"] => Shape::Task {
                        handle,
                        detail: Detail::History,
                    },
                    ["detail"] => Shape::Task {
                        handle,
                        detail: Detail::Both,
                    },
                    ["artifact"] => Shape::Artifacts { handle },
                    ["artifact", id] => {
                        refuse_mutation(&["task", "artifact", id], url)?;
                        Shape::Artifact {
                            handle,
                            id: (*id).to_owned(),
                        }
                    }
                    [verb] => {
                        refuse_mutation(&["task", verb], url)?;
                        return Err(unknown(url));
                    }
                    _ => return Err(unknown(url)),
                }
            }
            _ => return Err(unknown(url)),
        };
        let options = options(&shape, query, url)?;
        Ok(Self {
            resource: Resource::Read(Query { shape, options }),
            selector,
        })
    }
}

fn unknown(url: &str) -> String {
    format!("{url} names no spice:// resource; read {SCHEME} for the index")
}

/// Refuse a URL whose path spells a CLI leaf that acts rather than reads.
fn refuse_mutation(path: &[&str], url: &str) -> Result<(), String> {
    if SERVED.contains(&path) || crate::cli::contract(path).is_none() {
        return Ok(());
    }
    Err(format!(
        "{url} names `spice {}`, which acts on the board; spice:// only reads, so run the command",
        path.join(" ")
    ))
}

fn options(shape: &Shape, query: &str, url: &str) -> Result<Vec<(String, Option<String>)>, String> {
    let mut options: Vec<(String, Option<String>)> = Vec::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = match pair.split_once('=') {
            Some((key, value)) => (decode(key)?, Some(decode(value)?)),
            None => (decode(pair)?, None),
        };
        let admitted = shape.admitted();
        let Some(admitted_key) = admitted.iter().find(|admitted| admitted.name == key) else {
            let names = admitted
                .iter()
                .map(|admitted| admitted.name)
                .collect::<Vec<_>>();
            return Err(if names.is_empty() {
                format!("{url} takes no query; drop ?{query}")
            } else {
                format!(
                    "{url} takes no query key {key}; it takes {}",
                    names.join(", ")
                )
            });
        };
        match (&value, admitted_key.valued) {
            (Some(value), true) if !value.is_empty() => {}
            (None, false) => {}
            (_, true) => {
                return Err(format!(
                    "{url} names {key} without a value; write {key}=VALUE"
                ));
            }
            (Some(_), false) => {
                return Err(format!("{url} gives {key} a value; write {key} alone"));
            }
        }
        if options.iter().any(|(named, _)| *named == key) {
            return Err(format!("{url} names {key} twice"));
        }
        options.push((key, value));
    }
    Ok(options)
}

/// Percent-encode everything but unreserved URL bytes.
fn encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn decode(text: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let byte = bytes
                .get(index + 1..index + 3)
                .and_then(|hex| std::str::from_utf8(hex).ok())
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                .ok_or_else(|| format!("{text} has a % not followed by two hex digits"))?;
            decoded.push(byte);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).map_err(|_| format!("{text} decodes to bytes that are not UTF-8"))
}

/// The lines a trailing `:` selector names.
#[derive(Debug, PartialEq, Eq)]
struct Selector {
    /// Inclusive one-based line spans, in the order named; empty for `:raw`.
    spans: Vec<Span>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Span {
    /// Lines `first..=last`; `last` absent runs to the end.
    Lines { first: usize, last: Option<usize> },
    /// The last `count` lines.
    Tail { count: usize },
}

impl Selector {
    fn parse(written: &str) -> Result<Self, String> {
        let mut spans = Vec::new();
        for part in written.split(':') {
            match part {
                "raw" => {}
                "img" => {
                    return Err(
                        "spice:// reads are text; :img selects nothing, read the file a binary artifact names".to_owned(),
                    );
                }
                _ => {
                    for span in part.split(',') {
                        spans.push(Span::parse(span).ok_or_else(|| {
                            format!(
                                ":{written} is no line selector; use :N, :N-M, :N+K, :-N, a comma list, or :raw"
                            )
                        })?);
                    }
                }
            }
        }
        Ok(Self { spans })
    }

    fn select(&self, text: &str) -> Result<String, String> {
        if self.spans.is_empty() {
            return Ok(text.to_owned());
        }
        let lines = text.lines().collect::<Vec<_>>();
        let mut selected = Vec::new();
        for span in &self.spans {
            let (first, last) = match *span {
                Span::Lines { first, last } => (first, last.unwrap_or(lines.len())),
                Span::Tail { count } => (lines.len().saturating_sub(count) + 1, lines.len()),
            };
            if first > lines.len() {
                return Err(format!(
                    "line {first} is past the end; the resource has {} lines",
                    lines.len()
                ));
            }
            selected.extend_from_slice(&lines[first - 1..last.min(lines.len())]);
        }
        Ok(selected.join("\n"))
    }
}

impl Span {
    fn parse(written: &str) -> Option<Self> {
        let number = |text: &str| text.parse::<usize>().ok().filter(|number| *number > 0);
        if let Some(count) = written.strip_prefix('-') {
            return Some(Self::Tail {
                count: number(count)?,
            });
        }
        if let Some((first, count)) = written.split_once('+') {
            let first = number(first)?;
            return Some(Self::Lines {
                first,
                last: Some(first.checked_add(number(count)? - 1)?),
            });
        }
        if let Some((first, last)) = written.split_once('-') {
            let first = number(first)?;
            let last = if last.is_empty() {
                None
            } else {
                Some(number(last)?)
            };
            return last
                .is_none_or(|last| last >= first)
                .then_some(Self::Lines { first, last });
        }
        Some(Self::Lines {
            first: number(written)?,
            last: None,
        })
    }
}

/// Answer one `spice://` read for `lane`, whose checkout is `worktree`:
/// the document the URL names, or why there is none.
///
/// `read_quote` resolves quote targets.
pub(crate) fn read(
    worktree: &Path,
    lane: CanonicalUuid,
    url: &str,
    read_quote: impl FnOnce(&str) -> Result<String, String>,
) -> Result<String, String> {
    let request = Request::parse(url)?;
    let document = match &request.resource {
        Resource::Index => INDEX.to_owned(),
        Resource::Quote(target) => read_quote(target)?,
        Resource::Read(query) => {
            let repository = Repository::discover(worktree)
                .map_err(|error| format!("spice cannot resolve the lane's checkout: {error}"))?;
            crate::command_client::as_lane(lane, || run(&repository, query))?
        }
    };
    match &request.selector {
        Some(selector) => selector.select(&document),
        None => Ok(document),
    }
}

/// Run one read through the CLI's own parser and dispatch.
fn run(repository: &Repository, query: &Query) -> Result<String, String> {
    let matches = crate::cli::command()
        .try_get_matches_from(std::iter::once("spice".to_owned()).chain(query.arguments()))
        .map_err(|error| error.to_string().trim_end().to_owned())?;
    let contract = crate::cli::selected_contract(&matches)
        .expect("the generated parser selects only registered contracts");
    let leaf = crate::cli::selected_matches(&matches);
    if let ["task", "artifact", ..] = contract.path {
        let outcome = crate::task_artifact_command::run_at(repository, leaf, contract);
        let mut document = rendered(&outcome, render::render(&outcome, Form::Text))?;
        if let Outcome::Reply(reply) = &outcome {
            artifact_trailer(&mut document, &reply.body);
        }
        return Ok(document);
    }
    let outcome = crate::board_command::run_at(repository, leaf, contract);
    let rendering =
        render::render_with_reply_text(&outcome, Form::Text, crate::command_answer_text);
    let mut document = rewrite_continuations(&rendered(&outcome, rendering)?);
    if let Outcome::Reply(reply) = &outcome
        && let CommandAnswer::Text(_) = &reply.body
    {
        board_trailer(&mut document, &query.shape);
    }
    Ok(document)
}

/// The rendered document of a reply, or of a refusal or fault as the error.
fn rendered<B>(
    outcome: &Outcome<B>,
    rendering: Result<render::Rendering, String>,
) -> Result<String, String> {
    let rendering =
        rendering.map_err(|error| format!("spice could not render the read: {error}"))?;
    let document = String::from_utf8_lossy(rendering.document())
        .trim_end()
        .to_owned();
    match outcome {
        Outcome::Reply(_) => Ok(document),
        Outcome::Refusal(_) | Outcome::Fault(_) => Err(document),
    }
}

/// Replace each page continuation a resource serves with its URL.
fn rewrite_continuations(document: &str) -> String {
    const CONTINUE: &str = "; continue: ";
    document
        .lines()
        .map(|line| {
            let Some((head, command)) = line.rsplit_once(CONTINUE) else {
                return line.to_owned();
            };
            let url = shell_words::split(command)
                .ok()
                .and_then(|words| match words.split_first() {
                    Some((spice, arguments)) if spice == "spice" => {
                        Query::from_arguments(arguments)
                    }
                    _ => None,
                })
                .map(|query| query.url());
            match url {
                Some(url) => format!("{head}{CONTINUE}{url}"),
                None => line.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// End a collection with the child URLs it names.
fn board_trailer(document: &mut String, shape: &Shape) {
    let children = match shape {
        Shape::Board => vec![format!("{SCHEME}task")],
        Shape::Tasks => document
            .lines()
            .filter_map(|line| line.split_whitespace().next()?.parse::<TaskHandle>().ok())
            .map(|handle| format!("{SCHEME}task/{handle}"))
            .collect(),
        Shape::Task {
            handle,
            detail: Detail::Summary,
        } => ["context", "history", "detail", "artifact"]
            .iter()
            .map(|child| format!("{SCHEME}task/{handle}/{child}"))
            .collect(),
        _ => Vec::new(),
    };
    push_resources(document, &children);
}

/// Name an artifact page's continuation and its children as URLs.
fn artifact_trailer(document: &mut String, answer: &ArtifactAnswer) {
    let (continuation, children) = match answer {
        ArtifactAnswer::Listed {
            task,
            artifacts,
            omission,
        } => (
            omission.as_ref().map(|omission| &omission.continuation),
            artifacts
                .iter()
                .map(|artifact| {
                    Query {
                        shape: Shape::Artifact {
                            handle: task.clone(),
                            id: artifact.id.clone(),
                        },
                        options: Vec::new(),
                    }
                    .url()
                })
                .collect(),
        ),
        ArtifactAnswer::Shown { next, .. } => (next.as_ref(), Vec::new()),
        _ => (None, Vec::new()),
    };
    if let Some(query) = continuation.and_then(|command| Query::from_arguments(command.arguments()))
    {
        document.push_str("\ncontinue: ");
        document.push_str(&query.url());
    }
    push_resources(document, &children);
}

fn push_resources(document: &mut String, children: &[String]) {
    if children.is_empty() {
        return;
    }
    document.push_str("\nresources:");
    for child in children {
        document.push_str("\n  ");
        document.push_str(child);
    }
}

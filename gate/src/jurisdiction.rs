//! Which paths a gate run is allowed to speak about.
//!
//! A gate that measures whatever it happens to find measures something
//! different on every machine, and a number nobody can reproduce cannot hold a
//! ratchet up. Jurisdiction is the answer to that: one ignore policy, one walk
//! whose result is a total function of the tree, and one recorded reason for
//! every path the walk reached. Refused paths are recorded exactly as carefully
//! as admitted ones, because a measurement's silence is only trustworthy when
//! the record can say what filled it.

use std::fmt;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use wire::{Digest, Domain};

/// What the walk found at one path.
///
/// Symlinks are [`Shape::Other`] rather than resolved. Following them would let
/// a jurisdiction reach outside its own root and would let a cycle end the
/// walk, and neither outcome is one a deterministic record can survive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Shape {
    /// A regular file, the only shape a metric can measure.
    File,
    /// A directory, walked when admitted and pruned when not.
    Directory,
    /// Anything else, recorded and never descended or measured.
    Other,
}

impl Shape {
    /// The one spelling this shape takes in an inventory.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Directory => "directory",
            Self::Other => "other",
        }
    }
}

impl fmt::Display for Shape {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Why one path is inside or outside a jurisdiction.
///
/// Each variant names the rule that decided, so an inventory line is enough to
/// replay the decision against the policy without re-walking anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Admission {
    /// No rule named this path, so the default admits it.
    Unnamed,
    /// The last rule to match re-admitted it.
    Readmitted(usize),
    /// The last rule to match excluded it.
    Ignored(usize),
}

impl Admission {
    /// Whether this path is inside the jurisdiction.
    #[must_use]
    pub const fn admits(self) -> bool {
        match self {
            Self::Unnamed | Self::Readmitted(_) => true,
            Self::Ignored(_) => false,
        }
    }

    /// The rule that decided, absent when the default did.
    #[must_use]
    pub const fn rule(self) -> Option<usize> {
        match self {
            Self::Unnamed => None,
            Self::Readmitted(rule) | Self::Ignored(rule) => Some(rule),
        }
    }
}

impl fmt::Display for Admission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unnamed => formatter.write_str("unnamed"),
            Self::Readmitted(rule) => write!(formatter, "readmitted:{rule}"),
            Self::Ignored(rule) => write!(formatter, "ignored:{rule}"),
        }
    }
}

/// One path the walk reached and what the policy said about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    path: String,
    shape: Shape,
    admission: Admission,
}

impl Entry {
    /// Path relative to the jurisdiction root, always `/`-separated.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// What the walk found here.
    #[must_use]
    pub const fn shape(&self) -> Shape {
        self.shape
    }

    /// Why this path is in or out.
    #[must_use]
    pub const fn admission(&self) -> Admission {
        self.admission
    }

    /// Whether a metric may measure this path.
    #[must_use]
    pub const fn measurable(&self) -> bool {
        self.admission.admits() && matches!(self.shape, Shape::File)
    }
}

/// One ignore-file line, holding the shape of the match it performs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pattern: String,
    negated: bool,
    anchored: bool,
    directory_only: bool,
}

impl Rule {
    /// Parse one line, yielding nothing for blanks and comments.
    ///
    /// # Errors
    ///
    /// Returns the reason this line is not one of the admitted spellings.
    pub fn parse(line: &str) -> Result<Option<Self>, IgnoreError> {
        let refuse = |reason| Err(IgnoreError::new(line, reason));
        if line.trim() != line {
            return refuse(Malady::SurroundingSpace);
        }
        if line.is_empty() || line.starts_with('#') {
            return Ok(None);
        }

        let (negated, rest) = match line.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, line),
        };
        let (directory_only, rest) = match rest.strip_suffix('/') {
            Some(rest) => (true, rest),
            None => (false, rest),
        };
        let (rooted, pattern) = match rest.strip_prefix('/') {
            Some(rest) => (true, rest),
            None => (false, rest),
        };

        if pattern.is_empty() {
            return refuse(Malady::EmptyPattern);
        }
        if pattern.contains("**") {
            return refuse(Malady::CrossingStar);
        }
        if pattern.contains('\\') {
            return refuse(Malady::Escape);
        }

        Ok(Some(Self {
            anchored: rooted || pattern.contains('/'),
            pattern: pattern.to_owned(),
            negated,
            directory_only,
        }))
    }

    /// Whether this rule re-admits rather than excludes.
    #[must_use]
    pub const fn negated(&self) -> bool {
        self.negated
    }

    /// Whether this rule matches a path of this shape.
    #[must_use]
    pub fn matches(&self, path: &str, shape: Shape) -> bool {
        if self.directory_only && !matches!(shape, Shape::Directory) {
            return false;
        }

        let target = if self.anchored {
            path
        } else {
            &path[path.rfind('/').map_or(0, |slash| slash + 1)..]
        };
        glob_matches(self.pattern.as_bytes(), target.as_bytes())
    }
}

impl fmt::Display for Rule {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bang = if self.negated { "!" } else { "" };
        let root = if self.anchored && !self.pattern.contains('/') {
            "/"
        } else {
            ""
        };
        let slash = if self.directory_only { "/" } else { "" };
        write!(formatter, "{bang}{root}{}{slash}", self.pattern)
    }
}

/// Whether a glob matches this text, with `*` and `?` never crossing `/`.
fn glob_matches(pattern: &[u8], text: &[u8]) -> bool {
    let (mut at_pattern, mut at_text) = (0, 0);
    let (mut star, mut consumed) = (None, 0);

    while at_text < text.len() {
        let literal = pattern
            .get(at_pattern)
            .is_some_and(|byte| *byte == text[at_text] || (*byte == b'?' && text[at_text] != b'/'));

        if literal {
            at_pattern += 1;
            at_text += 1;
        } else if pattern.get(at_pattern) == Some(&b'*') {
            star = Some(at_pattern);
            consumed = at_text;
            at_pattern += 1;
        } else if let Some(index) = star.filter(|_| text[consumed] != b'/') {
            consumed += 1;
            at_text = consumed;
            at_pattern = index + 1;
        } else {
            return false;
        }
    }

    pattern[at_pattern..].iter().all(|byte| *byte == b'*')
}

/// The ordered ignore policy one jurisdiction answers to.
///
/// The last rule to match decides, which is what lets a broad exclusion be
/// narrowed by a later `!` line without the two orders meaning the same thing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Ignore {
    rules: Vec<Rule>,
}

impl Ignore {
    /// Parse an ignore file.
    ///
    /// # Errors
    ///
    /// Returns the first line that is not an admitted spelling, with that line
    /// as evidence.
    pub fn parse(text: &str) -> Result<Self, IgnoreError> {
        let mut rules = Vec::new();
        for line in text.lines() {
            if let Some(rule) = Rule::parse(line)? {
                rules.push(rule);
            }
        }
        Ok(Self { rules })
    }

    /// The rules in the order they decide.
    #[must_use]
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// What this policy says about one path.
    #[must_use]
    pub fn decide(&self, path: &str, shape: Shape) -> Admission {
        let mut decision = Admission::Unnamed;
        for (index, rule) in self.rules.iter().enumerate() {
            if rule.matches(path, shape) {
                decision = if rule.negated() {
                    Admission::Readmitted(index)
                } else {
                    Admission::Ignored(index)
                };
            }
        }
        decision
    }

    /// The canonical rendering of this policy, one rule per line.
    #[must_use]
    pub fn render(&self) -> String {
        let mut rendered = String::new();
        for rule in &self.rules {
            let _ = writeln!(rendered, "{rule}");
        }
        rendered
    }
}

/// Every path one walk reached, with the reason each is in or out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Jurisdiction {
    root: PathBuf,
    entries: Vec<Entry>,
}

impl Jurisdiction {
    /// Walk a foreign repository under one ignore policy.
    ///
    /// The result is a total function of the tree: children are read in sorted
    /// order, excluded directories are pruned but recorded, and the entries are
    /// sorted by path before they are returned, so traversal order cannot leak
    /// into the inventory.
    ///
    /// # Errors
    ///
    /// Returns the failure that stopped the walk, naming the path it happened
    /// at.
    pub fn walk(root: &Path, ignore: &Ignore) -> Result<Self, WalkError> {
        let mut entries = Vec::new();
        let mut directories = vec![String::new()];

        while let Some(relative) = directories.pop() {
            for (name, shape) in read_children(root, &relative)? {
                let path = if relative.is_empty() {
                    name
                } else {
                    format!("{relative}/{name}")
                };
                let admission = ignore.decide(&path, shape);

                if matches!(shape, Shape::Directory) && admission.admits() {
                    directories.push(path.clone());
                }
                entries.push(Entry {
                    path,
                    shape,
                    admission,
                });
            }
        }

        entries.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(Self {
            root: root.to_path_buf(),
            entries,
        })
    }

    /// The root every entry path is relative to.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every path the walk reached, sorted, admitted and refused alike.
    #[must_use]
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The admitted regular files, in path order.
    pub fn measurable(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|entry| entry.measurable())
    }

    /// The canonical inventory: one line per reached path, reason first.
    ///
    /// The path is last because a path may contain spaces and the reason and
    /// shape may not, which makes every line readable by splitting twice.
    #[must_use]
    pub fn inventory(&self) -> String {
        let mut rendered = String::new();
        for entry in &self.entries {
            let _ = writeln!(
                rendered,
                "{} {} {}",
                entry.admission, entry.shape, entry.path
            );
        }
        rendered
    }

    /// Digest of the inventory, covering refusals as well as admissions.
    #[must_use]
    pub fn inventory_digest(&self) -> Digest {
        Digest::compute(Domain::GateInventory, self.inventory().as_bytes())
    }

    /// Digest of what the admitted files contain.
    ///
    /// Each file is digested under its own domain and the listing of those
    /// digests is digested under another, so a file whose bytes happen to spell
    /// a listing cannot be mistaken for one.
    ///
    /// # Errors
    ///
    /// Returns the read failure, naming the file it happened at.
    pub fn tree_digest(&self) -> Result<Digest, WalkError> {
        let mut listing = String::new();
        for entry in self.measurable() {
            let path = self.root.join(&entry.path);
            let bytes = fs::read(&path)
                .map_err(|source| WalkError::io("reading an admitted file", &path, source))?;
            let _ = writeln!(
                listing,
                "{} {}",
                Digest::compute(Domain::GateFile, &bytes),
                entry.path
            );
        }
        Ok(Digest::compute(Domain::GateTree, listing.as_bytes()))
    }
}

/// Read one directory's children, sorted, refusing names an inventory cannot
/// carry.
fn read_children(root: &Path, relative: &str) -> Result<Vec<(String, Shape)>, WalkError> {
    let directory = if relative.is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative)
    };
    let listing = fs::read_dir(&directory)
        .map_err(|source| WalkError::io("opening a directory", &directory, source))?;

    let mut children = Vec::new();
    for child in listing {
        let child =
            child.map_err(|source| WalkError::io("reading a directory", &directory, source))?;
        let name = child
            .file_name()
            .into_string()
            .map_err(|name| WalkError::Unnameable {
                path: directory.join(name),
            })?;
        if name.contains('\n') {
            return Err(WalkError::Unnameable {
                path: directory.join(&name),
            });
        }

        let path = child.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|source| WalkError::io("inspecting a path", &path, source))?;
        let shape = if metadata.is_dir() {
            Shape::Directory
        } else if metadata.is_file() {
            Shape::File
        } else {
            Shape::Other
        };

        children.push((name, shape));
    }

    children.sort();
    Ok(children)
}

/// What is wrong with one ignore-file line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Malady {
    /// The line carries leading or trailing space, so it has two spellings.
    SurroundingSpace,
    /// The line names no pattern at all.
    EmptyPattern,
    /// `**` is not part of the matcher this gate declares.
    CrossingStar,
    /// Backslash escapes are not part of the matcher this gate declares.
    Escape,
}

impl fmt::Display for Malady {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SurroundingSpace => {
                formatter.write_str("carries surrounding space, so it has more than one spelling")
            }
            Self::EmptyPattern => formatter.write_str("names no pattern"),
            Self::CrossingStar => {
                formatter.write_str("uses ** , which this matcher does not implement")
            }
            Self::Escape => formatter
                .write_str("uses a backslash escape, which this matcher does not implement"),
        }
    }
}

/// A line that is not one of the admitted ignore spellings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IgnoreError {
    line: String,
    malady: Malady,
}

impl IgnoreError {
    fn new(line: &str, malady: Malady) -> Self {
        Self {
            line: line.to_owned(),
            malady,
        }
    }

    /// The exact line that was refused.
    #[must_use]
    pub fn line(&self) -> &str {
        &self.line
    }

    /// Why it was refused.
    #[must_use]
    pub const fn malady(&self) -> Malady {
        self.malady
    }
}

impl fmt::Display for IgnoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "ignore line {:?} {}", self.line, self.malady)
    }
}

impl std::error::Error for IgnoreError {}

/// What stopped a walk.
#[derive(Debug)]
pub enum WalkError {
    /// The filesystem refused an operation the walk needed.
    Io {
        /// What the walk was doing.
        operation: &'static str,
        /// Where it was doing it.
        path: PathBuf,
        /// What the filesystem said.
        source: io::Error,
    },
    /// A name no canonical inventory line could carry.
    Unnameable {
        /// The path that cannot be written down.
        path: PathBuf,
    },
}

impl WalkError {
    fn io(operation: &'static str, path: &Path, source: io::Error) -> Self {
        Self::Io {
            operation,
            path: path.to_path_buf(),
            source,
        }
    }
}

impl fmt::Display for WalkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                operation,
                path,
                source,
            } => write!(
                formatter,
                "{operation} at {} failed: {source}",
                path.display()
            ),
            Self::Unnameable { path } => write!(
                formatter,
                "{} cannot be written into a canonical inventory",
                path.display()
            ),
        }
    }
}

impl std::error::Error for WalkError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Unnameable { .. } => None,
        }
    }
}

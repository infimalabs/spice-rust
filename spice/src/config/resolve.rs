//! Pure admission, precedence, and provenance for acquired configuration text.

use std::error::Error;
use std::fmt;

use serde_path_to_error::Segment;
use toml::Value;
use wire::{ConfigSource, Name};

use super::{
    Builtin, Descriptor, FlagMapping, PathSegment, Root, ValueKind, descriptor, descriptors,
};

/// One already-acquired configuration document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Document<'a> {
    location: &'a str,
    text: &'a str,
}

impl<'a> Document<'a> {
    /// Pair a diagnostic location with the exact bytes acquired there.
    #[must_use]
    pub const fn new(location: &'a str, text: &'a str) -> Self {
        Self { location, text }
    }

    /// File or other acquisition location named by diagnostics.
    #[must_use]
    pub const fn location(self) -> &'a str {
        self.location
    }

    /// Exact acquired TOML text.
    #[must_use]
    pub const fn text(self) -> &'a str {
        self.text
    }
}

/// Every already-acquired source admitted by one pure resolution.
#[derive(Clone, Copy, Debug)]
pub struct Inputs<'a> {
    tracked: Option<Document<'a>>,
    local: Option<Document<'a>>,
    flags: &'a [FlagOverride],
}

impl<'a> Inputs<'a> {
    /// Supply optional documents and explicit typed command flags.
    #[must_use]
    pub const fn new(
        tracked: Option<Document<'a>>,
        local: Option<Document<'a>>,
        flags: &'a [FlagOverride],
    ) -> Self {
        Self {
            tracked,
            local,
            flags,
        }
    }

    /// Resolve only compiled defaults.
    #[must_use]
    pub const fn empty() -> Self {
        Self::new(None, None, &[])
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum OwnedSegment {
    Key(String),
    Index(usize),
}

/// An owned, structurally segmented path produced by the resolver.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ConcretePath(Vec<OwnedSegment>);

impl ConcretePath {
    fn root() -> Self {
        Self(Vec::new())
    }

    /// Borrow every key and index without reparsing display text.
    pub fn segments(&self) -> impl ExactSizeIterator<Item = PathSegment<'_>> {
        self.0.iter().map(|segment| match segment {
            OwnedSegment::Key(key) => PathSegment::Key(key),
            OwnedSegment::Index(index) => PathSegment::Index(*index),
        })
    }

    fn borrowed(&self) -> Vec<PathSegment<'_>> {
        self.segments().collect()
    }

    fn starts_with(&self, prefix: &Self) -> bool {
        self.0.starts_with(&prefix.0)
    }

    fn related(&self, other: &Self) -> bool {
        self.starts_with(other) || other.starts_with(self)
    }

    fn push_key(&mut self, key: &str) {
        self.0.push(OwnedSegment::Key(key.to_owned()));
    }

    fn push_index(&mut self, index: usize) {
        self.0.push(OwnedSegment::Index(index));
    }

    fn pop(&mut self) {
        self.0.pop();
    }
}

impl fmt::Display for ConcretePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            return formatter.write_str("<root>");
        }
        let mut first = true;
        for segment in &self.0 {
            match segment {
                OwnedSegment::Key(key) => {
                    if !first {
                        formatter.write_str(".")?;
                    }
                    write_key(formatter, key)?;
                }
                OwnedSegment::Index(index) => write!(formatter, "[{index}]")?,
            }
            first = false;
        }
        Ok(())
    }
}

fn write_key(formatter: &mut fmt::Formatter<'_>, key: &str) -> fmt::Result {
    if !key.is_empty()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        formatter.write_str(key)
    } else {
        formatter.write_str("\"")?;
        for character in key.chars() {
            for escaped in character.escape_default() {
                write!(formatter, "{escaped}")?;
            }
        }
        formatter.write_str("\"")
    }
}

/// The closed set of typed values currently carried by command mappings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FlagValue {
    /// A lane lifecycle policy.
    Life(lane::life::Life),
    /// A validated one-word identity.
    Name(Name),
}

impl FlagValue {
    const fn kind(&self) -> ValueKind {
        match self {
            Self::Life(_) => ValueKind::Life,
            Self::Name(_) => ValueKind::String,
        }
    }

    fn to_toml(&self) -> Value {
        match self {
            Self::Life(life) => {
                Value::try_from(life).expect("lane life must serialize as a TOML string")
            }
            Self::Name(name) => {
                Value::try_from(name).expect("wire name must serialize as a TOML string")
            }
        }
    }
}

/// One explicit flag routed through a registry-owned command mapping.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlagOverride {
    mapping: FlagMapping,
    path: ConcretePath,
    value: FlagValue,
}

impl FlagOverride {
    /// Bind one typed value to an exported registry mapping.
    ///
    /// # Errors
    ///
    /// Refuses a value whose type does not match the mapped descriptor.
    pub fn new(mapping: FlagMapping, value: FlagValue) -> Result<Self, FlagError> {
        let entry = descriptors()
            .iter()
            .find(|entry| entry.flag() == Some(mapping))
            .ok_or(FlagError::UnknownMapping)?;
        if entry.kind() != value.kind() {
            return Err(FlagError::WrongValue {
                expected: entry.kind(),
                actual: value.kind(),
            });
        }
        let path = concrete_fixed(entry).ok_or(FlagError::DynamicMapping)?;
        Ok(Self {
            mapping,
            path,
            value,
        })
    }

    /// Registry mapping that admitted this override.
    #[must_use]
    pub const fn mapping(&self) -> FlagMapping {
        self.mapping
    }
}

/// Why a typed command override could not bind to its mapping.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlagError {
    /// The mapping is not present in this registry build.
    UnknownMapping,
    /// Command mappings must target one fixed leaf.
    DynamicMapping,
    /// The typed flag value does not match the descriptor's shape.
    WrongValue {
        /// Descriptor shape.
        expected: ValueKind,
        /// Supplied typed value shape.
        actual: ValueKind,
    },
}

impl fmt::Display for FlagError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownMapping => formatter.write_str("flag mapping is not in the registry"),
            Self::DynamicMapping => {
                formatter.write_str("flag mapping does not name one fixed leaf")
            }
            Self::WrongValue { expected, actual } => write!(
                formatter,
                "flag expects {} but received {}",
                expected.as_str(),
                actual.as_str()
            ),
        }
    }
}

impl Error for FlagError {}

/// Stable diagnostic category.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum IssueKind {
    /// The document is not TOML.
    Syntax,
    /// A parsed table or key is outside the release registry.
    Unknown,
    /// A known path is forbidden in this layer.
    Policy,
    /// A known value is not admitted by its domain decoder.
    Malformed,
    /// The same explicit command mapping appeared more than once.
    DuplicateFlag,
}

/// One deterministic refusal from one supplied source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Issue {
    kind: IssueKind,
    source: ConfigSource,
    document: String,
    path: Option<ConcretePath>,
    line: Option<usize>,
    column: Option<usize>,
    message: String,
}

impl Issue {
    /// Diagnostic category.
    #[must_use]
    pub const fn kind(&self) -> IssueKind {
        self.kind
    }

    /// Layer carrying the refused intent.
    #[must_use]
    pub const fn source(&self) -> ConfigSource {
        self.source
    }

    /// File or synthesized command location.
    #[must_use]
    pub fn document(&self) -> &str {
        &self.document
    }

    /// Structured offending path when parsing reached one.
    #[must_use]
    pub const fn path(&self) -> Option<&ConcretePath> {
        self.path.as_ref()
    }

    /// One-based syntax line, when applicable.
    #[must_use]
    pub const fn line(&self) -> Option<usize> {
        self.line
    }

    /// One-based syntax column, when applicable.
    #[must_use]
    pub const fn column(&self) -> Option<usize> {
        self.column
    }

    /// Domain or routing refusal.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    fn at(
        kind: IssueKind,
        source: ConfigSource,
        document: &str,
        path: ConcretePath,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            source,
            document: document.to_owned(),
            path: Some(path),
            line: None,
            column: None,
            message: message.into(),
        }
    }
}

impl fmt::Display for Issue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.document)?;
        if let (Some(line), Some(column)) = (self.line, self.column) {
            write!(formatter, ":{line}:{column}")?;
        }
        if let Some(path) = &self.path {
            write!(formatter, ": {path}")?;
        }
        write!(formatter, ": {}", self.message)
    }
}

/// Every refusal discovered in one resolution attempt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolveError {
    issues: Vec<Issue>,
}

impl ResolveError {
    /// Refusals in deterministic path and source order.
    #[must_use]
    pub fn issues(&self) -> &[Issue] {
        &self.issues
    }
}

impl fmt::Display for ResolveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "configuration refused with {} issue(s)",
            self.issues.len()
        )
    }
}

impl Error for ResolveError {}

/// Winning source for one concrete effective leaf.
#[derive(Clone, Debug, PartialEq)]
pub struct Provenance {
    path: ConcretePath,
    source: ConfigSource,
    builtin: Option<Builtin>,
}

impl Provenance {
    /// Effective concrete leaf.
    #[must_use]
    pub const fn path(&self) -> &ConcretePath {
        &self.path
    }

    /// Highest-precedence layer that supplied the leaf.
    #[must_use]
    pub const fn source(&self) -> ConfigSource {
        self.source
    }

    /// Compiled value shadowed by authored intent, when one exists.
    #[must_use]
    pub const fn builtin(&self) -> Option<Builtin> {
        self.builtin
    }
}

/// The only successful resolution product.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    root: Root,
    provenance: Vec<Provenance>,
}

impl Resolved {
    /// Fully admitted effective configuration.
    #[must_use]
    pub const fn root(&self) -> &Root {
        &self.root
    }

    /// One deterministic row for every concrete effective leaf.
    #[must_use]
    pub fn provenance(&self) -> &[Provenance] {
        &self.provenance
    }

    /// Registry metadata governing this typed result.
    #[must_use]
    pub const fn registry(&self) -> &'static [Descriptor] {
        descriptors()
    }

    /// Consume the result and return its typed root.
    #[must_use]
    pub fn into_root(self) -> Root {
        self.root
    }
}

#[derive(Clone, Debug)]
struct Layer {
    source: ConfigSource,
    document: String,
    value: Value,
    leaves: Vec<ConcretePath>,
}

/// Resolve compiled defaults and every supplied layer without acquiring state.
///
/// # Errors
///
/// Returns all independently observable syntax, routing, policy, and typed
/// admission failures in deterministic order. No partial root is exposed.
pub fn resolve(inputs: Inputs<'_>) -> Result<Resolved, ResolveError> {
    let mut issues = Vec::new();
    let mut layers = Vec::new();

    if let Some(document) = inputs.tracked
        && let Some(layer) = parse_layer(ConfigSource::Tracked, document, &mut issues)
    {
        layers.push(layer);
    }
    if let Some(document) = inputs.local
        && let Some(layer) = parse_layer(ConfigSource::Local, document, &mut issues)
    {
        layers.push(layer);
    }
    if !inputs.flags.is_empty() {
        layers.push(flag_layer(inputs.flags, &mut issues));
    }

    let mut effective = Value::try_from(Root::default())
        .expect("the compiled typed configuration must serialize to TOML");
    let mut admitted = Vec::with_capacity(layers.len());
    for layer in layers {
        let (value, leaves) = validate_layer(&effective, &layer, &mut issues);
        merge(&mut effective, value.clone());
        admitted.push(Layer {
            value,
            leaves,
            ..layer
        });
    }

    let root = decode_root(effective)
        .expect("invalid authored values are removed before the final typed decode");
    sort_issues(&mut issues);
    if !issues.is_empty() {
        return Err(ResolveError { issues });
    }

    Ok(Resolved {
        provenance: provenance(&root, &admitted),
        root,
    })
}

fn parse_layer(
    source: ConfigSource,
    document: Document<'_>,
    issues: &mut Vec<Issue>,
) -> Option<Layer> {
    let mut value = match toml::from_str::<Value>(document.text) {
        Ok(value) => value,
        Err(error) => {
            let (line, column) = error
                .span()
                .map(|span| line_column(document.text, span.start))
                .unwrap_or((1, 1));
            issues.push(Issue {
                kind: IssueKind::Syntax,
                source,
                document: document.location.to_owned(),
                path: None,
                line: Some(line),
                column: Some(column),
                message: error.message().to_owned(),
            });
            return None;
        }
    };
    let mut leaves = Vec::new();
    let mut path = ConcretePath::root();
    sanitize(
        &mut value,
        &mut path,
        source,
        document.location,
        issues,
        &mut leaves,
    );
    Some(Layer {
        source,
        document: document.location.to_owned(),
        value,
        leaves,
    })
}

fn sanitize(
    value: &mut Value,
    path: &mut ConcretePath,
    source: ConfigSource,
    document: &str,
    issues: &mut Vec<Issue>,
    leaves: &mut Vec<ConcretePath>,
) -> bool {
    let borrowed = path.borrowed();
    if let Some(entry) = descriptor(&borrowed) {
        if !entry.layers().contains(&source) {
            issues.push(Issue::at(
                IssueKind::Policy,
                source,
                document,
                path.clone(),
                format!(
                    "{} is tracked policy; set it in tracked spice.toml",
                    entry.template().as_str()
                ),
            ));
            return false;
        }
        if !compatible(entry.kind(), value) {
            issues.push(Issue::at(
                IssueKind::Malformed,
                source,
                document,
                path.clone(),
                format!("expected {}", entry.kind().as_str()),
            ));
            return false;
        }
        leaves.push(path.clone());
        return true;
    }

    if !descriptors()
        .iter()
        .any(|entry| entry.template().accepts_prefix(&borrowed))
    {
        issues.push(Issue::at(
            IssueKind::Unknown,
            source,
            document,
            path.clone(),
            "path is not present in the release configuration registry",
        ));
        return false;
    }

    match value {
        Value::Table(table) => {
            table.retain(|key, child| {
                path.push_key(key);
                let keep = sanitize(child, path, source, document, issues, leaves);
                path.pop();
                keep
            });
            true
        }
        Value::Array(values) => {
            let mut index = 0;
            values.retain_mut(|child| {
                path.push_index(index);
                let keep = sanitize(child, path, source, document, issues, leaves);
                path.pop();
                index += 1;
                keep
            });
            true
        }
        _ => {
            issues.push(Issue::at(
                IssueKind::Malformed,
                source,
                document,
                path.clone(),
                "expected a configuration table or array of tables",
            ));
            false
        }
    }
}

fn compatible(kind: ValueKind, value: &Value) -> bool {
    match kind {
        ValueKind::String | ValueKind::Duration | ValueKind::Answer | ValueKind::Life => {
            value.is_str()
        }
        ValueKind::StringList | ValueKind::DurationList => value
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_str)),
        ValueKind::UnsignedInteger => value
            .as_integer()
            .is_some_and(|number| u64::try_from(number).is_ok()),
        ValueKind::Float => value.is_float(),
    }
}

fn flag_layer(flags: &[FlagOverride], issues: &mut Vec<Issue>) -> Layer {
    let mut value = Value::Table(toml::Table::new());
    let mut leaves = Vec::new();
    let mut document = Vec::new();
    for flag in flags {
        let location = format!(
            "{} {}",
            flag.mapping.command().join(" "),
            flag.mapping.flag()
        );
        if leaves.contains(&flag.path) {
            issues.push(Issue::at(
                IssueKind::DuplicateFlag,
                ConfigSource::Flag,
                &location,
                flag.path.clone(),
                "explicit flag mapping was supplied more than once",
            ));
            continue;
        }
        insert_unit(&mut value, &flag.path, flag.value.to_toml());
        leaves.push(flag.path.clone());
        document.push(location);
    }
    Layer {
        source: ConfigSource::Flag,
        document: document.join(", "),
        value,
        leaves,
    }
}

fn validate_layer(
    base: &Value,
    layer: &Layer,
    issues: &mut Vec<Issue>,
) -> (Value, Vec<ConcretePath>) {
    let units = validation_units(&layer.leaves);
    let mut active = vec![true; units.len()];

    loop {
        let candidate = build_units(&layer.value, &units, &active);
        let mut merged = base.clone();
        merge(&mut merged, candidate.clone());
        let Err(full_error) = decode_root(merged) else {
            let leaves = layer
                .leaves
                .iter()
                .filter(|leaf| {
                    units
                        .iter()
                        .zip(&active)
                        .any(|(unit, active)| *active && leaf.starts_with(unit))
                })
                .cloned()
                .collect();
            return (candidate, leaves);
        };

        let mut invalid = Vec::new();
        for (index, unit) in units.iter().enumerate() {
            if !active[index] {
                continue;
            }
            let mut isolated = base.clone();
            let mask = units
                .iter()
                .enumerate()
                .map(|(candidate, _)| candidate == index)
                .collect::<Vec<_>>();
            merge(&mut isolated, build_units(&layer.value, &units, &mask));
            if let Err(error) = decode_root(isolated) {
                invalid.push((index, unit.clone(), error.1));
            }
        }

        if invalid.is_empty() {
            let index = units
                .iter()
                .enumerate()
                .find(|(index, unit)| active[*index] && unit.related(&full_error.0))
                .map(|(index, _)| index)
                .or_else(|| active.iter().position(|active| *active))
                .expect("a layer decode cannot fail without authored validation units");
            invalid.push((index, units[index].clone(), full_error.1));
        }

        for (index, path, message) in invalid {
            active[index] = false;
            issues.push(Issue::at(
                IssueKind::Malformed,
                layer.source,
                &layer.document,
                path,
                message,
            ));
        }
    }
}

fn validation_units(leaves: &[ConcretePath]) -> Vec<ConcretePath> {
    let mut units = Vec::new();
    for leaf in leaves {
        let entry = descriptor(&leaf.borrowed()).expect("sanitized leaves are registered");
        let template = entry.template().as_str().split('.');
        let mut unit_len = leaf.0.len();
        for (concrete, segment) in template.enumerate() {
            if segment == "<name>" {
                unit_len = concrete + 1;
                break;
            }
            if segment.ends_with("[]") {
                unit_len = concrete + 2;
                break;
            }
        }
        let unit = ConcretePath(leaf.0[..unit_len].to_vec());
        if !units.contains(&unit) {
            units.push(unit);
        }
    }
    units
}

fn build_units(source: &Value, units: &[ConcretePath], active: &[bool]) -> Value {
    let mut output = Value::Table(toml::Table::new());
    for (unit, active) in units.iter().zip(active) {
        if !active {
            continue;
        }
        let value = value_at(source, unit)
            .expect("validation units originate in the sanitized layer")
            .clone();
        insert_unit(&mut output, unit, value);
    }
    output
}

fn value_at<'a>(value: &'a Value, path: &ConcretePath) -> Option<&'a Value> {
    let mut current = value;
    for segment in &path.0 {
        current = match segment {
            OwnedSegment::Key(key) => current.as_table()?.get(key)?,
            OwnedSegment::Index(index) => current.as_array()?.get(*index)?,
        };
    }
    Some(current)
}

fn insert_unit(target: &mut Value, path: &ConcretePath, value: Value) {
    insert_segments(target, &path.0, value);
}

fn insert_segments(target: &mut Value, path: &[OwnedSegment], value: Value) {
    let Some((head, tail)) = path.split_first() else {
        *target = value;
        return;
    };
    match head {
        OwnedSegment::Key(key) => {
            if !target.is_table() {
                *target = Value::Table(toml::Table::new());
            }
            let child = target
                .as_table_mut()
                .expect("a table was just installed")
                .entry(key.clone())
                .or_insert_with(|| match tail.first() {
                    Some(OwnedSegment::Index(_)) => Value::Array(Vec::new()),
                    _ => Value::Table(toml::Table::new()),
                });
            insert_segments(child, tail, value);
        }
        OwnedSegment::Index(_) => {
            if !target.is_array() {
                *target = Value::Array(Vec::new());
            }
            let values = target.as_array_mut().expect("an array was just installed");
            if tail.is_empty() {
                values.push(value);
            } else {
                let mut child = match tail.first() {
                    Some(OwnedSegment::Index(_)) => Value::Array(Vec::new()),
                    _ => Value::Table(toml::Table::new()),
                };
                insert_segments(&mut child, tail, value);
                values.push(child);
            }
        }
    }
}

fn merge(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Table(base), Value::Table(overlay)) => {
            for (key, value) in overlay {
                if let Some(existing) = base.get_mut(&key) {
                    merge(existing, value);
                } else {
                    base.insert(key, value);
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

fn decode_root(value: Value) -> Result<Root, (ConcretePath, String)> {
    serde_path_to_error::deserialize(value).map_err(|error| {
        let path = ConcretePath(
            error
                .path()
                .iter()
                .filter_map(|segment| match segment {
                    Segment::Seq { index } => Some(OwnedSegment::Index(*index)),
                    Segment::Map { key } | Segment::Enum { variant: key } => {
                        Some(OwnedSegment::Key(key.clone()))
                    }
                    Segment::Unknown => None,
                })
                .collect(),
        );
        (path, error.inner().message().to_owned())
    })
}

fn provenance(root: &Root, layers: &[Layer]) -> Vec<Provenance> {
    let value = Value::try_from(root).expect("an admitted root must serialize to TOML");
    let mut leaves = Vec::new();
    collect_leaves(&value, &mut ConcretePath::root(), &mut leaves);
    leaves
        .into_iter()
        .map(|path| {
            let source = layers
                .iter()
                .rev()
                .find(|layer| layer.leaves.contains(&path))
                .map_or(ConfigSource::Builtin, |layer| layer.source);
            let builtin = if source == ConfigSource::Builtin {
                None
            } else {
                descriptor(&path.borrowed()).and_then(|entry| entry.builtin())
            };
            Provenance {
                path,
                source,
                builtin,
            }
        })
        .collect()
}

fn collect_leaves(value: &Value, path: &mut ConcretePath, leaves: &mut Vec<ConcretePath>) {
    match value {
        Value::Table(table) => {
            for (key, child) in table {
                path.push_key(key);
                collect_leaves(child, path, leaves);
                path.pop();
            }
        }
        Value::Array(values)
            if !values.is_empty() && values.iter().all(|value| value.is_table()) =>
        {
            for (index, child) in values.iter().enumerate() {
                path.push_index(index);
                collect_leaves(child, path, leaves);
                path.pop();
            }
        }
        Value::Array(values) if values.is_empty() && descriptor(&path.borrowed()).is_none() => {}
        _ => leaves.push(path.clone()),
    }
}

fn concrete_fixed(entry: &Descriptor) -> Option<ConcretePath> {
    let mut path = Vec::new();
    for segment in entry.template().as_str().split('.') {
        if segment == "<name>" || segment.ends_with("[]") {
            return None;
        }
        path.push(OwnedSegment::Key(segment.to_owned()));
    }
    Some(ConcretePath(path))
}

fn line_column(text: &str, byte: usize) -> (usize, usize) {
    let mut byte = byte.min(text.len());
    while !text.is_char_boundary(byte) {
        byte -= 1;
    }
    let prefix = &text[..byte];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = prefix
        .rsplit_once('\n')
        .map_or(prefix, |(_, suffix)| suffix)
        .chars()
        .count()
        + 1;
    (line, column)
}

fn sort_issues(issues: &mut Vec<Issue>) {
    issues.sort_by(|left, right| {
        left.path
            .cmp(&right.path)
            .then_with(|| left.source.cmp(&right.source))
            .then_with(|| left.kind.cmp(&right.kind))
            .then_with(|| left.document.cmp(&right.document))
            .then_with(|| left.message.cmp(&right.message))
    });
    issues.dedup();
}

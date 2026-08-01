//! The typed release-one configuration root and its complete leaf registry.
//!
//! Domain crates remain the only authorities for admitting values. This
//! module composes those values and publishes metadata used to route layers,
//! render help, and report provenance without carrying a second value parser.

use serde::{Deserialize, Serialize};
use wire::ConfigSource;

mod load;
mod resolve;

pub use load::{
    AcquisitionIssue, AcquisitionKind, LoadError, Locations, load, load_repository, locations,
};
pub use resolve::{
    ConcretePath, Document, FlagError, FlagOverride, FlagValue, Inputs, Issue, IssueKind,
    Provenance, ResolveError, Resolved, resolve,
};

/// Every release-one configuration domain.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Root {
    taste: taste::Config,
    consent: consent::Config,
    gate: gate::Config,
    board: board::Config,
    lane: lane::Config,
    steer: steer::Config,
    vendor: vendor::Config,
    listen: listen::Config,
    lock: sill::LockConfig,
    canvas: canvas::Config,
    proof: proof::Config,
}

impl Root {
    /// Active prose tastes.
    #[must_use]
    pub const fn taste(&self) -> &taste::Config {
        &self.taste
    }

    /// Ordered consent policy.
    #[must_use]
    pub const fn consent(&self) -> &consent::Config {
        &self.consent
    }

    /// Gate walk policy.
    #[must_use]
    pub const fn gate(&self) -> &gate::Config {
        &self.gate
    }

    /// Allocator tuning.
    #[must_use]
    pub const fn board(&self) -> &board::Config {
        &self.board
    }

    /// Lane supervision policy.
    #[must_use]
    pub const fn lane(&self) -> &lane::Config {
        &self.lane
    }

    /// Steering horizons.
    #[must_use]
    pub const fn steer(&self) -> &steer::Config {
        &self.steer
    }

    /// Vendor executable overrides.
    #[must_use]
    pub const fn vendor(&self) -> &vendor::Config {
        &self.vendor
    }

    /// Listening horizon.
    #[must_use]
    pub const fn listen(&self) -> &listen::Config {
        &self.listen
    }

    /// Lock exit and capacity policy.
    #[must_use]
    pub const fn lock(&self) -> &sill::LockConfig {
        &self.lock
    }

    /// Canvas packing limits.
    #[must_use]
    pub const fn canvas(&self) -> &canvas::Config {
        &self.canvas
    }

    /// Proof image policy and machine-local executable locations.
    #[must_use]
    pub const fn proof(&self) -> &proof::Config {
        &self.proof
    }
}

/// Serialized shape of one leaf value.
///
/// Semantic validation still belongs to the domain type named by [`Root`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueKind {
    /// One string.
    String,
    /// One ordered string list.
    StringList,
    /// One unsigned integer.
    UnsignedInteger,
    /// One finite floating-point number.
    Float,
    /// One duration word.
    Duration,
    /// One ordered duration-word list.
    DurationList,
    /// One consent answer.
    Answer,
    /// One lane life variant.
    Life,
}

impl ValueKind {
    /// Stable metadata spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::StringList => "string-list",
            Self::UnsignedInteger => "unsigned-integer",
            Self::Float => "float",
            Self::Duration => "duration",
            Self::DurationList => "duration-list",
            Self::Answer => "answer",
            Self::Life => "life",
        }
    }
}

/// A compiled default represented as explicit absence or a serde scalar.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Builtin {
    /// An optional fixed leaf has no compiled value.
    Absent,
    /// One string-shaped value.
    String(&'static str),
    /// One ordered string-shaped list.
    Strings(&'static [&'static str]),
    /// One unsigned integer.
    UnsignedInteger(u64),
    /// One finite floating-point number.
    Float(f64),
}

/// Whether a leaf is tracked policy or an admitted machine-local setting.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PolicyClass {
    /// Behavior that is owned by tracked policy except for an explicit flag.
    Policy,
    /// Machine-specific operation that admits a local value.
    MachineLocal,
}

impl PolicyClass {
    /// Stable metadata spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Policy => "policy",
            Self::MachineLocal => "machine-local",
        }
    }
}

/// One command flag that may supply a configuration value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlagMapping {
    command: &'static [&'static str],
    flag: &'static str,
}

impl FlagMapping {
    const fn new(command: &'static [&'static str], flag: &'static str) -> Self {
        Self { command, flag }
    }

    /// Command path carrying the flag.
    #[must_use]
    pub const fn command(self) -> &'static [&'static str] {
        self.command
    }

    /// Exact long flag spelling.
    #[must_use]
    pub const fn flag(self) -> &'static str {
        self.flag
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TemplateShape {
    Fixed,
    Named,
    Indexed,
}

/// One unambiguous segment of a concrete configuration leaf path.
///
/// Keys remain whole even when a domain admits punctuation in a dynamic name;
/// indexes are separate from keys rather than encoded into display text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathSegment<'a> {
    /// One table or field key.
    Key(&'a str),
    /// One ordered row index.
    Index(usize),
}

/// One fixed or dynamic leaf path template.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PathTemplate {
    rendered: &'static str,
    shape: TemplateShape,
}

impl PathTemplate {
    const fn fixed(rendered: &'static str) -> Self {
        Self {
            rendered,
            shape: TemplateShape::Fixed,
        }
    }

    const fn named(rendered: &'static str) -> Self {
        Self {
            rendered,
            shape: TemplateShape::Named,
        }
    }

    const fn indexed(rendered: &'static str) -> Self {
        Self {
            rendered,
            shape: TemplateShape::Indexed,
        }
    }

    /// Stable path spelling, using `<name>` and `[]` for dynamic segments.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.rendered
    }

    /// Whether this template covers one concrete leaf path.
    #[must_use]
    pub fn matches(self, path: &[PathSegment<'_>]) -> bool {
        self.covers(path, true)
    }

    pub(crate) fn accepts_prefix(self, path: &[PathSegment<'_>]) -> bool {
        self.covers(path, false)
    }

    fn covers(self, path: &[PathSegment<'_>], exact: bool) -> bool {
        let mut at = 0;
        for segment in self.rendered.split('.') {
            if at == path.len() {
                return !exact;
            }
            if segment == "<name>" {
                if !matches!(path.get(at), Some(PathSegment::Key(_))) {
                    return false;
                }
                at += 1;
            } else if let Some(key) = segment.strip_suffix("[]") {
                if path.get(at) != Some(&PathSegment::Key(key)) {
                    return false;
                }
                at += 1;
                if at == path.len() {
                    return !exact;
                }
                if !matches!(path.get(at), Some(PathSegment::Index(_))) {
                    return false;
                }
                at += 1;
            } else {
                if path.get(at) != Some(&PathSegment::Key(segment)) {
                    return false;
                }
                at += 1;
            }
        }
        at == path.len()
    }

    /// Whether the template contains a named or indexed row.
    #[must_use]
    pub const fn is_dynamic(self) -> bool {
        !matches!(self.shape, TemplateShape::Fixed)
    }
}

/// Metadata and routing policy for one leaf template.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Descriptor {
    template: PathTemplate,
    kind: ValueKind,
    builtin: Option<Builtin>,
    layers: &'static [ConfigSource],
    class: PolicyClass,
    flag: Option<FlagMapping>,
}

impl Descriptor {
    /// Fixed or dynamic path template.
    #[must_use]
    pub const fn template(self) -> PathTemplate {
        self.template
    }

    /// Serialized value shape.
    #[must_use]
    pub const fn kind(self) -> ValueKind {
        self.kind
    }

    /// Compiled default; `None` is reserved for operator-created dynamic rows.
    #[must_use]
    pub const fn builtin(self) -> Option<Builtin> {
        self.builtin
    }

    /// Admitted authored layers in increasing precedence order.
    #[must_use]
    pub const fn layers(self) -> &'static [ConfigSource] {
        self.layers
    }

    /// Tracked-policy or machine-local classification.
    #[must_use]
    pub const fn policy_class(self) -> PolicyClass {
        self.class
    }

    /// Command flag supplying this leaf, when one exists.
    #[must_use]
    pub const fn flag(self) -> Option<FlagMapping> {
        self.flag
    }
}

const TRACKED: &[ConfigSource] = &[ConfigSource::Tracked];
const TRACKED_LOCAL: &[ConfigSource] = &[ConfigSource::Tracked, ConfigSource::Local];
const TRACKED_FLAG: &[ConfigSource] = &[ConfigSource::Tracked, ConfigSource::Flag];
const LOCAL: &[ConfigSource] = &[ConfigSource::Local];

macro_rules! registry {
    ($(
        $shape:ident $path:literal {
            kind: $kind:ident,
            builtin: $builtin:expr,
            layers: $layers:ident,
            class: $class:ident,
            flag: $flag:expr,
        }
    )+) => {
        const DESCRIPTORS: &[Descriptor] = &[
            $(Descriptor {
                template: PathTemplate::$shape($path),
                kind: ValueKind::$kind,
                builtin: $builtin,
                layers: $layers,
                class: PolicyClass::$class,
                flag: $flag,
            }),+
        ];
    };
}

registry! {
    named "taste.<name>.words" {
        kind: StringList,
        builtin: None,
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    named "taste.<name>.message" {
        kind: String,
        builtin: None,
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "consent.posture" {
        kind: Answer,
        builtin: Some(Builtin::String("queue")),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    indexed "consent.rule[].name" {
        kind: String,
        builtin: None,
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    indexed "consent.rule[].tool" {
        kind: String,
        builtin: None,
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    indexed "consent.rule[].answer" {
        kind: Answer,
        builtin: None,
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    indexed "consent.rule[].budget" {
        kind: UnsignedInteger,
        builtin: None,
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "gate.walk.skip" {
        kind: StringList,
        builtin: Some(Builtin::Strings(&[])),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "board.next.review" {
        kind: Float,
        builtin: Some(Builtin::Float(-100.0)),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "board.next.band" {
        kind: Float,
        builtin: Some(Builtin::Float(2.0)),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "lane.life" {
        kind: Life,
        builtin: Some(Builtin::String("Drive")),
        layers: TRACKED_FLAG,
        class: Policy,
        flag: Some(FlagMapping::new(&["lane", "add"], "--life")),
    }
    fixed "lane.lease" {
        kind: Duration,
        builtin: Some(Builtin::String("15m")),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "lane.vendor" {
        kind: String,
        builtin: Some(Builtin::String("claude")),
        layers: TRACKED_FLAG,
        class: Policy,
        flag: Some(FlagMapping::new(&["lane", "add"], "--vendor")),
    }
    fixed "lane.deaths" {
        kind: UnsignedInteger,
        builtin: Some(Builtin::UnsignedInteger(3)),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "lane.young" {
        kind: Duration,
        builtin: Some(Builtin::String("30s")),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "lane.hold" {
        kind: Duration,
        builtin: Some(Builtin::String("30m")),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "steer.expiry" {
        kind: Duration,
        builtin: Some(Builtin::String("24h")),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "steer.resend" {
        kind: DurationList,
        builtin: Some(Builtin::Strings(&["5m", "15m", "1h"])),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    named "vendor.<name>.binary" {
        kind: String,
        builtin: None,
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "listen.window" {
        kind: Duration,
        builtin: Some(Builtin::String("90s")),
        layers: TRACKED_LOCAL,
        class: MachineLocal,
        flag: None,
    }
    fixed "lock.busy" {
        kind: UnsignedInteger,
        builtin: Some(Builtin::UnsignedInteger(75)),
        layers: TRACKED_LOCAL,
        class: MachineLocal,
        flag: None,
    }
    fixed "lock.taken" {
        kind: UnsignedInteger,
        builtin: Some(Builtin::UnsignedInteger(76)),
        layers: TRACKED_LOCAL,
        class: MachineLocal,
        flag: None,
    }
    fixed "lock.full" {
        kind: UnsignedInteger,
        builtin: Some(Builtin::UnsignedInteger(77)),
        layers: TRACKED_LOCAL,
        class: MachineLocal,
        flag: None,
    }
    named "lock.<name>.shards" {
        kind: UnsignedInteger,
        builtin: None,
        layers: TRACKED_LOCAL,
        class: MachineLocal,
        flag: None,
    }
    fixed "canvas.pack.rows" {
        kind: UnsignedInteger,
        builtin: Some(Builtin::UnsignedInteger(40)),
        layers: TRACKED_LOCAL,
        class: MachineLocal,
        flag: None,
    }
    fixed "proof.image" {
        kind: String,
        builtin: Some(Builtin::String(proof::DEFAULT_IMAGE)),
        layers: TRACKED,
        class: Policy,
        flag: None,
    }
    fixed "proof.container" {
        kind: String,
        builtin: Some(Builtin::Absent),
        layers: LOCAL,
        class: MachineLocal,
        flag: None,
    }
    fixed "proof.runner" {
        kind: String,
        builtin: Some(Builtin::Absent),
        layers: LOCAL,
        class: MachineLocal,
        flag: None,
    }
}

/// Complete descriptor registry in stable configuration order.
#[must_use]
pub const fn descriptors() -> &'static [Descriptor] {
    DESCRIPTORS
}

/// Find the sole descriptor covering a concrete leaf path.
#[must_use]
pub fn descriptor(path: &[PathSegment<'_>]) -> Option<&'static Descriptor> {
    DESCRIPTORS
        .iter()
        .find(|descriptor| descriptor.template.matches(path))
}

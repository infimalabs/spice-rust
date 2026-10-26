use super::*;

/// How one lower-is-better floor obtains its current measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloorKind {
    /// An ordinary parser-backed [`Metric`].
    Measurement,
    /// Confirmed survivor debt produced by the mutation battery.
    Mutation,
}

impl FloorKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Measurement => "measurement",
            Self::Mutation => "mutation",
        }
    }
}

/// One compiled lower-is-better name in the current gate set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FloorDeclaration {
    pub(super) name: String,
    pub(super) kind: FloorKind,
}

impl FloorDeclaration {
    fn new(name: &str, kind: FloorKind) -> Self {
        Self {
            name: name.to_owned(),
            kind,
        }
    }

    /// Stable floor name used by `baseline.toml`.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Which current execution path measures this floor.
    #[must_use]
    pub const fn kind(&self) -> FloorKind {
        self.kind
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ToolDeclaration {
    pub(super) name: String,
    pub(super) program: String,
    pub(super) probe_arguments: Vec<String>,
    pub(super) probe_output: String,
}

impl ToolDeclaration {
    fn awk() -> Self {
        Self {
            name: "awk".to_owned(),
            program: AWK.to_owned(),
            probe_arguments: vec![AWK_PROBE.to_owned()],
            probe_output: AWK_PROBE_OUTPUT.to_owned(),
        }
    }

    pub(super) fn render(&self, into: &mut String) {
        let _ = writeln!(into, "tool {} program {}", self.name, self.program);
        for argument in &self.probe_arguments {
            let _ = writeln!(into, "tool {} probe-argument {argument}", self.name);
        }
        let _ = writeln!(
            into,
            "tool {} probe-output {}",
            self.name, self.probe_output
        );
    }
}

/// One compiled Gate executable declaration without execution authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutableDeclaration {
    name: String,
    program: String,
}

impl ExecutableDeclaration {
    fn new(name: impl Into<String>, program: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            program: program.into(),
        }
    }

    /// Stable compiled tool name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Compiled program: an absolute path is exact, and a bare name resolves
    /// on the running host's executable search path.
    #[must_use]
    pub fn program(&self) -> &str {
        &self.program
    }
}

/// Return every compiled Gate policy executable without reading or starting it.
///
/// # Errors
///
/// Returns a specification error if this build's own declaration set is not
/// internally coherent.
pub fn declared_tools() -> Result<Vec<ExecutableDeclaration>, GateSetError> {
    GateSet::current().map(|gate_set| {
        gate_set
            .tools
            .into_iter()
            .map(|tool| ExecutableDeclaration::new(tool.name, tool.program))
            .collect()
    })
}

/// Return every Gate mutation program name without resolving, reading, or
/// starting it.
///
/// # Errors
///
/// Returns a specification error if this build's own declaration set is not
/// internally coherent.
pub fn declared_mutation_tools() -> Result<Vec<ExecutableDeclaration>, GateSetError> {
    GateSet::current().map(|_| {
        [CARGO_PROGRAM, MUTANTS_PROGRAM]
            .into_iter()
            .chain(NATIVE_BUILD_PROGRAMS)
            .map(|program| ExecutableDeclaration::new(program, program))
            .collect()
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MetricDeclaration {
    tool: String,
    pub(super) metric: Metric,
}

impl MetricDeclaration {
    fn new(tool: &str, metric: Metric) -> Self {
        Self {
            tool: tool.to_owned(),
            metric,
        }
    }

    fn render(&self, into: &mut String) {
        let _ = writeln!(into, "metric {} tool {}", self.metric.name(), self.tool);
        self.metric.render(into);
    }

    fn render_camp(&self, into: &mut String) {
        let _ = writeln!(
            into,
            "camp-metric {} tool {}",
            self.metric.name(),
            self.tool
        );
        self.metric.render(into);
    }
}

const MUTATION_TARGETS: [&str; 4] = [
    "gate/src/camp.rs",
    "gate/src/input.rs",
    "gate/src/kill.rs",
    "gate/src/verdict.rs",
];
const MUTATION_ATTEMPTS: usize = 3;
const MUTATION_SEED: u64 = 41;
const MUTATION_RUNTIME: Duration = Duration::from_secs(300);
const MUTATION_OUTPUT_DRAIN: Duration = Duration::from_secs(2);
const MUTATION_REAP: Duration = Duration::from_secs(2);
const MUTATION_GRACE: Duration = Duration::from_secs(1);
/// The pinned 27.1.0 inventory over the four current targets is 528,161 bytes
/// for 331 mutants. 640 KiB is the first 128 KiB step above that measurement:
/// it retains 127,199 bytes of growth margin while keeping each stream bounded.
const MUTATION_CAPTURE: usize = 640 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct KillBoundsDeclaration {
    deadlines: Deadlines,
    grace: Duration,
    pub(super) capture: usize,
}

impl KillBoundsDeclaration {
    fn current() -> Result<Self, RateError> {
        let deadlines = Deadlines::new(MUTATION_RUNTIME, MUTATION_OUTPUT_DRAIN, MUTATION_REAP)
            .map_err(RateError::Spec)?;
        Ok(Self {
            deadlines,
            grace: MUTATION_GRACE,
            capture: MUTATION_CAPTURE,
        })
    }

    fn build(self) -> Result<KillBounds, RateError> {
        KillBounds::new(
            "gate current mutation profile",
            self.deadlines,
            self.grace,
            self.capture,
        )
    }

    fn render(self, into: &mut String) {
        let _ = writeln!(
            into,
            "kill deadline-runtime-ms {}",
            self.deadlines.runtime().as_millis()
        );
        let _ = writeln!(
            into,
            "kill deadline-output-drain-ms {}",
            self.deadlines.output_drain().as_millis()
        );
        let _ = writeln!(
            into,
            "kill deadline-reap-ms {}",
            self.deadlines.reap().as_millis()
        );
        let _ = writeln!(into, "kill grace-ms {}", self.grace.as_millis());
        let _ = writeln!(into, "kill capture {}", self.capture);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct KillDeclaration {
    pub(super) identity: &'static str,
    pub(super) battery: MutationBattery,
    pub(super) bounds: KillBoundsDeclaration,
}

impl KillDeclaration {
    fn current() -> Result<Self, RateError> {
        check_identity(PINNED_CARGO_MUTANTS)?;
        let battery = MutationBattery::new(MUTATION_TARGETS, MUTATION_ATTEMPTS, MUTATION_SEED)?;
        Ok(Self {
            identity: PINNED_CARGO_MUTANTS,
            battery,
            bounds: KillBoundsDeclaration::current()?,
        })
    }

    /// Resolve the declared programs on the running host into one runner.
    #[cfg(not(target_arch = "wasm32"))]
    fn resolve(
        &self,
        rim: &crate::toolchain::RimCapture,
        inherited: &[(OsString, OsString)],
    ) -> Result<Kill, KillError> {
        let tool = MutationTool::resolve(self.identity, rim, inherited)?;
        let bounds = self
            .bounds
            .build()
            .expect("admitted mutation bounds remain valid");
        Ok(Kill::new(tool, self.battery.clone(), bounds))
    }

    fn render(&self, into: &mut String) {
        MutationTool::render_declaration(self.identity, into);
        let _ = writeln!(into, "kill attempts {}", self.battery.attempts());
        let _ = writeln!(into, "kill seed {}", self.battery.seed());
        for target in self.battery.targets() {
            let _ = writeln!(into, "kill target {target}");
        }
        self.bounds.render(into);
    }
}

const CAMP_BOUND: &str = "rust.gate-line-length";
const CAMP_LOCI: [(&str, &str); 4] = [
    ("rust.max-line.camp", "gate/src/camp.rs"),
    ("rust.max-line.input", "gate/src/input.rs"),
    ("rust.max-line.kill", "gate/src/kill.rs"),
    ("rust.max-line.verdict", "gate/src/verdict.rs"),
];

/// The compiled checks and floor names this build can execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateSet {
    pub(super) metrics: Vec<MetricDeclaration>,
    pub(super) camp_metrics: Vec<MetricDeclaration>,
    pub(super) camp: Camp,
    pub(super) kill: KillDeclaration,
    pub(super) floors: Vec<FloorDeclaration>,
    pub(super) tools: Vec<ToolDeclaration>,
}

impl GateSet {
    pub(super) fn current() -> Result<Self, GateSetError> {
        let camp_metrics = current_camp_metrics()?;
        let camp = Camp::new([SharedBound::new(
            CAMP_BOUND,
            LONG_LINE_LIMIT as u64,
            CAMP_LOCI.map(|(name, _)| name),
        )
        .map_err(GateSetError::Camp)?])
        .map_err(GateSetError::Camp)?;
        let kill = KillDeclaration::current().map_err(GateSetError::Kill)?;

        Self::admit(
            vec![],
            camp_metrics,
            camp,
            kill,
            vec![FloorDeclaration::new(
                "rust.mutation-survivors",
                FloorKind::Mutation,
            )],
            vec![ToolDeclaration::awk()],
        )
    }

    pub(super) fn admit(
        mut metrics: Vec<MetricDeclaration>,
        mut camp_metrics: Vec<MetricDeclaration>,
        camp: Camp,
        kill: KillDeclaration,
        mut floors: Vec<FloorDeclaration>,
        mut tools: Vec<ToolDeclaration>,
    ) -> Result<Self, GateSetError> {
        metrics.sort_by(|left, right| left.metric.name().cmp(right.metric.name()));
        camp_metrics.sort_by(|left, right| left.metric.name().cmp(right.metric.name()));
        floors.sort_by(|left, right| left.name.cmp(&right.name));
        tools.sort_by(|left, right| left.name.cmp(&right.name));

        let mut metric_names = BTreeSet::new();
        for declaration in metrics.iter().chain(&camp_metrics) {
            if !metric_names.insert(declaration.metric.name().to_owned()) {
                return Err(GateSetError::DuplicateMetric {
                    metric: declaration.metric.name().to_owned(),
                });
            }
        }

        validate_camp_declarations(&camp_metrics, &camp)?;
        kill.bounds.build().map_err(GateSetError::Kill)?;

        let mut names = BTreeSet::new();
        let mut programs = BTreeSet::new();
        for tool in &tools {
            if !names.insert(tool.name.clone()) {
                return Err(GateSetError::DuplicateToolName {
                    name: tool.name.clone(),
                });
            }
            if !programs.insert(tool.program.clone()) {
                return Err(GateSetError::DuplicateToolProgram {
                    program: tool.program.clone(),
                });
            }
        }

        let mut used = BTreeSet::new();
        for declaration in metrics.iter().chain(&camp_metrics) {
            let Some(tool) = tools.iter().find(|tool| tool.name == declaration.tool) else {
                return Err(GateSetError::MissingTool {
                    metric: declaration.metric.name().to_owned(),
                    tool: declaration.tool.clone(),
                    parser_program: declaration.metric.parser().program().to_owned(),
                });
            };
            let parser_program = declaration.metric.parser().program();
            if parser_program != tool.program {
                return Err(GateSetError::MismatchedTool {
                    metric: declaration.metric.name().to_owned(),
                    tool: tool.name.clone(),
                    parser_program: parser_program.to_owned(),
                    tool_program: tool.program.clone(),
                });
            }
            used.insert(tool.name.clone());
        }

        if let Some(tool) = tools.iter().find(|tool| !used.contains(&tool.name)) {
            return Err(GateSetError::UnusedTool {
                tool: tool.name.clone(),
                program: tool.program.clone(),
            });
        }

        Ok(Self {
            metrics,
            camp_metrics,
            camp,
            kill,
            floors,
            tools,
        })
    }

    /// Current floor declarations in canonical name order.
    #[must_use]
    pub fn floors(&self) -> &[FloorDeclaration] {
        &self.floors
    }

    /// Current compiled shared-bound profile.
    #[must_use]
    pub const fn camp(&self) -> &Camp {
        &self.camp
    }

    /// Resolve the mutation programs on the running host into one runner.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn resolve_kill(
        &self,
        rim: &crate::toolchain::RimCapture,
        inherited: &[(OsString, OsString)],
    ) -> Result<Kill, KillError> {
        self.kill.resolve(rim, inherited)
    }

    pub(crate) fn camp_metrics(&self) -> impl Iterator<Item = Metric> + '_ {
        self.camp_metrics
            .iter()
            .map(|declaration| declaration.metric.clone())
    }

    /// Canonical compiled declaration, including parser argv and tool probes.
    #[must_use]
    pub fn render(&self) -> String {
        let mut rendered = String::from("gate-set 1\n");
        let _ = writeln!(rendered, "parser-file-limit {}", Parser::FILE_LIMIT);
        for floor in &self.floors {
            let _ = writeln!(rendered, "floor {} {}", floor.name, floor.kind.as_str());
        }
        for tool in &self.tools {
            tool.render(&mut rendered);
        }
        for declaration in &self.metrics {
            declaration.render(&mut rendered);
        }
        for bound in self.camp.bounds() {
            let _ = writeln!(rendered, "camp {} limit {}", bound.name(), bound.limit());
            for locus in bound.loci() {
                let _ = writeln!(rendered, "camp {} locus {locus}", bound.name());
            }
        }
        for declaration in &self.camp_metrics {
            declaration.render_camp(&mut rendered);
        }
        self.kill.render(&mut rendered);
        rendered
    }

    /// Identity of the complete compiled set.
    #[must_use]
    pub fn digest(&self) -> Digest {
        Digest::compute(Domain::GateSet, self.render().as_bytes())
    }
}

fn current_camp_metrics() -> Result<Vec<MetricDeclaration>, GateSetError> {
    CAMP_LOCI
        .iter()
        .map(|(name, path)| {
            let parser = Parser::new(
                AWK,
                ["{ if (length($0) > maximum) maximum = length($0) } END { print maximum + 0 }"],
            )
            .map_err(GateSetError::Metric)?;
            let metric = Metric::new(
                *name,
                "rust",
                "maximum source-line length in one gate implementation locus",
                parser,
                path,
                "",
            )
            .map_err(GateSetError::Metric)?;
            Ok(MetricDeclaration::new("awk", metric))
        })
        .collect()
}

fn validate_camp_declarations(
    metrics: &[MetricDeclaration],
    camp: &Camp,
) -> Result<(), GateSetError> {
    if camp.bounds().is_empty() {
        return Err(GateSetError::EmptyCamp);
    }
    let declared: Vec<String> = camp
        .bounds()
        .iter()
        .flat_map(|bound| bound.loci().iter().cloned())
        .collect();
    let measured: Vec<String> = metrics
        .iter()
        .map(|declaration| declaration.metric.name().to_owned())
        .collect();
    if declared != measured {
        return Err(GateSetError::CampLoci { declared, measured });
    }
    Ok(())
}

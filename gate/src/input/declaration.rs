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
    name: String,
    kind: FloorKind,
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
struct ToolDeclaration {
    name: String,
    program: String,
    probe_arguments: Vec<String>,
    probe_output: String,
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

    fn render(&self, into: &mut String) {
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct MetricDeclaration {
    tool: String,
    metric: Metric,
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
}

/// The compiled checks and floor names this build can execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateSet {
    metrics: Vec<MetricDeclaration>,
    floors: Vec<FloorDeclaration>,
    tools: Vec<ToolDeclaration>,
}

impl GateSet {
    fn current() -> Result<Self, GateSetError> {
        let program =
            format!("length($0) > {LONG_LINE_LIMIT} {{ debt += 1 }} END {{ print debt + 0 }}");
        let parser = Parser::new(AWK, [program]).map_err(GateSetError::Metric)?;
        let metric = Metric::new(
            "rust.long-lines",
            "rust",
            format!("source lines longer than {LONG_LINE_LIMIT} bytes"),
            parser,
            "*.rs",
            "",
        )
        .map_err(GateSetError::Metric)?;

        Self::admit(
            vec![MetricDeclaration::new("awk", metric)],
            vec![
                FloorDeclaration::new("rust.long-lines", FloorKind::Measurement),
                FloorDeclaration::new("rust.mutation-survivors", FloorKind::Mutation),
            ],
            vec![ToolDeclaration::awk()],
        )
    }

    fn admit(
        metrics: Vec<MetricDeclaration>,
        floors: Vec<FloorDeclaration>,
        tools: Vec<ToolDeclaration>,
    ) -> Result<Self, GateSetError> {
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
        for declaration in &metrics {
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
            floors,
            tools,
        })
    }

    /// Current floor declarations in canonical name order.
    #[must_use]
    pub fn floors(&self) -> &[FloorDeclaration] {
        &self.floors
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
        rendered
    }

    /// Identity of the complete compiled set.
    #[must_use]
    pub fn digest(&self) -> Digest {
        Digest::compute(Domain::GateSet, self.render().as_bytes())
    }
}

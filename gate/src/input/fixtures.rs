#[cfg(test)]
use super::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_inputs_preserve_typed_absence_while_strict_inputs_require_both_files() {
        let root = fixture_root("generic-absent");

        let generic = Contract::load_generic(&root).expect("generic inputs admit absent files");
        assert_eq!(
            generic.rule_policy().document(),
            migrate_predecessor_policy(PACKAGED_GENERIC_POLICY_SOURCE, PACKAGED_GENERIC_POLICY)
                .expect("the packaged generic policy resolves")
                .document()
        );
        assert_eq!(
            generic.tracked_configuration(),
            &toml::Value::Table(toml::Table::new()),
            "an absent repository policy must not invent tracked values"
        );
        assert!(generic.baseline_digest().is_none());
        assert!(generic.tools().is_empty());
        assert!(matches!(
            generic.execute_kill(&root),
            Err(KillError::MissingFloor { ref path, name })
                if path == &root.join(BASELINE_PATH) && name == "rust.mutation-survivors"
        ));

        assert!(matches!(
            Contract::load(&root),
            Err(ContractError::Read { ref path, ref source })
                if path == &root.join(POLICY_PATH)
                    && source.kind() == io::ErrorKind::NotFound
        ));
        fs::remove_dir_all(root).expect("remove generic input fixture");
    }

    #[test]
    fn generic_inputs_refuse_each_malformed_present_document_at_its_exact_path() {
        let root = fixture_root("generic-malformed");
        let policy_path = root.join(POLICY_PATH);
        let baseline_path = root.join(BASELINE_PATH);

        fs::write(&policy_path, "[policy\n").expect("malformed present policy");
        let policy = Contract::load_generic(&root).expect_err("present malformed policy refuses");
        assert!(matches!(
            policy,
            ContractError::Policy(PolicyError::Toml { ref path, .. })
                if path == &policy_path
        ));

        fs::write(&policy_path, "").expect("present incomplete policy");
        let incomplete =
            Contract::load_generic(&root).expect_err("present incomplete policy refuses");
        assert!(matches!(
            incomplete,
            ContractError::Policy(PolicyError::Rules { ref path, ref source })
                if path == &policy_path
                    && source.to_string().contains("policy.file_shape.source_suffixes")
        ));

        fs::write(&policy_path, PACKAGED_GENERIC_POLICY).expect("valid present policy");
        fs::write(&baseline_path, "version = [\n").expect("malformed present baseline");
        let baseline =
            Contract::load_generic(&root).expect_err("present malformed baseline refuses");
        assert!(matches!(
            baseline,
            ContractError::Baseline(BaselineError::Toml { ref path, .. })
                if path == &baseline_path
        ));
        fs::remove_dir_all(root).expect("remove malformed input fixture");
    }

    fn fixture_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "spice-gate-{label}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir(&root).expect("create isolated input fixture");
        root
    }

    #[test]
    fn doctor_declarations_are_exact_views_of_both_current_tool_profiles() {
        let current = GateSet::current().expect("compiled gate set");
        assert_eq!(
            declared_tools()
                .expect("policy declarations")
                .iter()
                .map(|declaration| (declaration.name(), declaration.program()))
                .collect::<Vec<_>>(),
            current
                .tools
                .iter()
                .map(|declaration| (declaration.name.as_str(), Path::new(&declaration.program)))
                .collect::<Vec<_>>()
        );

        let mutation = declared_mutation_tools().expect("mutation declarations");
        assert_eq!(
            mutation
                .iter()
                .map(|declaration| (declaration.name(), declaration.program()))
                .collect::<Vec<_>>(),
            [
                ("cargo", current.kill.tool.cargo_program()),
                ("cargo-mutants", current.kill.tool.mutants_program()),
            ]
        );
        assert!(
            declared_tools()
                .expect("policy declarations")
                .into_iter()
                .chain(mutation)
                .all(|declaration| declaration.program().is_absolute())
        );
    }

    #[test]
    fn missing_and_non_operational_compiled_tools_are_named_before_execution() {
        let root = workspace_root();
        let bounds = current_bounds().expect("compiled bounds are valid");
        let missing = ToolDeclaration {
            name: "missing".to_owned(),
            program: root.join("does-not-exist").display().to_string(),
            probe_arguments: Vec::new(),
            probe_output: "unused".to_owned(),
        };
        let refused = identify_tool(&missing, &root, &bounds).expect_err("missing tool refuses");
        assert!(matches!(
            refused,
            ToolError::Read { ref name, ref source, .. }
                if name == "missing" && source.kind() == io::ErrorKind::NotFound
        ));

        let non_operational = ToolDeclaration {
            name: "wrong-answer".to_owned(),
            program: "/bin/sh".to_owned(),
            probe_arguments: vec!["-c".to_owned(), "printf wrong".to_owned()],
            probe_output: "right".to_owned(),
        };
        let refused = identify_tool(&non_operational, &root, &bounds)
            .expect_err("a tool that answers another probe is not operational");
        assert!(matches!(
            refused,
            ToolError::NonOperational { ref name, ref evidence }
                if name == "wrong-answer" && evidence.contains("wrong")
        ));
    }

    #[test]
    fn gate_set_admission_closes_every_metric_over_exactly_one_used_tool() {
        let current = GateSet::current().expect("compiled gate set");

        let mut incomplete_camp = current.clone();
        incomplete_camp.camp_metrics.pop();
        assert!(matches!(
            readmit(incomplete_camp),
            Err(GateSetError::CampLoci { ref declared, ref measured })
                if declared.len() == 4 && measured.len() == 3
        ));

        let mut empty_camp = current.clone();
        empty_camp.camp = Camp::new([]).expect("the Camp type permits an empty collection");
        assert!(matches!(readmit(empty_camp), Err(GateSetError::EmptyCamp)));

        let mut duplicate_metric = current.clone();
        duplicate_metric.camp_metrics[1].metric = duplicate_metric.camp_metrics[0].metric.clone();
        assert!(matches!(
            readmit(duplicate_metric),
            Err(GateSetError::DuplicateMetric { ref metric }) if metric == "rust.max-line.camp"
        ));

        let mut missing = current.clone();
        missing.tools.clear();
        assert!(matches!(
            readmit(missing),
            Err(GateSetError::MissingTool {
                ref metric,
                ref tool,
                ref parser_program,
            }) if metric == "rust.max-line.camp" && tool == "awk" && parser_program == AWK
        ));

        let mut duplicate_name = current.clone();
        let mut second = duplicate_name.tools[0].clone();
        second.program = "/bin/sh".to_owned();
        duplicate_name.tools.push(second);
        assert!(matches!(
            readmit(duplicate_name),
            Err(GateSetError::DuplicateToolName { ref name }) if name == "awk"
        ));

        let mut duplicate_program = current.clone();
        let mut second = duplicate_program.tools[0].clone();
        second.name = "awk-copy".to_owned();
        duplicate_program.tools.push(second);
        assert!(matches!(
            readmit(duplicate_program),
            Err(GateSetError::DuplicateToolProgram { ref program }) if program == AWK
        ));

        let mut mismatched = current.clone();
        mismatched.tools[0].program = "/bin/sh".to_owned();
        assert!(matches!(
            readmit(mismatched),
            Err(GateSetError::MismatchedTool {
                ref metric,
                ref tool,
                ref parser_program,
                ref tool_program,
            }) if metric == "rust.max-line.camp" && tool == "awk"
                && parser_program == AWK && tool_program == "/bin/sh"
        ));

        let mut unused = current;
        unused.tools.push(ToolDeclaration {
            name: "sh".to_owned(),
            program: "/bin/sh".to_owned(),
            probe_arguments: vec!["-c".to_owned(), "printf unused".to_owned()],
            probe_output: "unused".to_owned(),
        });
        assert!(matches!(
            readmit(unused),
            Err(GateSetError::UnusedTool {
                ref tool,
                ref program,
            }) if tool == "sh" && program == "/bin/sh"
        ));
    }

    #[test]
    fn executable_and_bound_declarations_move_their_own_identities() {
        let current = GateSet::current().expect("compiled gate set");
        let mut changed = current.clone();
        changed.tools[0].program = "/bin/awk".to_owned();
        assert_ne!(
            current.digest(),
            changed.digest(),
            "an executable declaration did not move the gate-set identity"
        );

        let bounds = current_bounds().expect("compiled bounds");
        let changed_bounds = Bounds::new(
            "gate current tools",
            bounds.deadlines(),
            Duration::from_millis(251),
            bounds.capture(),
        )
        .expect("changed bounds remain valid");
        assert_ne!(bounds.digest(), changed_bounds.digest());

        let mut changed_mutation_tool = current.clone();
        changed_mutation_tool.kill.tool = MutationTool::new(
            changed_mutation_tool.kill.tool.cargo_program(),
            "/different/mutants",
            changed_mutation_tool.kill.tool.identity(),
            changed_mutation_tool.kill.tool.base_environment().clone(),
        )
        .expect("changed absolute declaration remains valid");
        assert_ne!(
            current.digest(),
            changed_mutation_tool.digest(),
            "the mutation executable declaration did not move the gate set"
        );

        let mut changed_mutation_bounds = current.clone();
        changed_mutation_bounds.kill.bounds.capture += 1;
        assert_ne!(
            current.digest(),
            changed_mutation_bounds.digest(),
            "the mutation capture bound did not move the gate set"
        );

        let mut changed_battery = current.clone();
        changed_battery.kill.battery = MutationBattery::new(
            changed_battery.kill.battery.targets().iter().cloned(),
            changed_battery.kill.battery.attempts(),
            changed_battery.kill.battery.seed() + 1,
        )
        .expect("changed seed remains a valid battery");
        assert_ne!(
            current.digest(),
            changed_battery.digest(),
            "the mutation battery did not move the gate set"
        );

        let first = identify_tool(&current.tools[0], &workspace_root(), &bounds)
            .expect("current tool answers its probe");
        let second = identify_tool(&current.tools[0], &workspace_root(), &bounds)
            .expect("same current tool answers again");
        assert_eq!(first, second, "unchanged executable bytes moved identity");
    }

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("gate is a direct workspace member")
            .to_path_buf()
    }

    fn readmit(candidate: GateSet) -> Result<GateSet, GateSetError> {
        GateSet::admit(
            candidate.metrics,
            candidate.camp_metrics,
            candidate.camp,
            candidate.kill,
            candidate.floors,
            candidate.tools,
        )
    }
}

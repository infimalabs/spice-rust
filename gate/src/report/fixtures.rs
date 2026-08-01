#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use serde_json::json;
    use wire::{Outcome, Reply, ReplyPosition};

    use super::*;
    use crate::Contract;

    #[test]
    fn one_current_run_constructs_one_coherent_gate_report_and_direct_body() {
        let report = current_gate_report();

        assert_eq!(report.version(), GATE_REPORT_VERSION);
        assert_eq!(report.standing(), report.verdict().standing());
        assert_eq!(report.findings(), report.verdict().findings());
        assert_eq!(report.measurements().len(), 1);
        assert!(report.tightening().is_some());

        let encoded = report.encode().expect("encode gate report");
        let decoded = GateReport::decode(&encoded).expect("decode report");
        assert_eq!(decoded, report);
        assert_eq!(report.encode().expect("encode again"), encoded);
        assert_eq!(decoded.render(), report.render());

        let outcome = Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body: report,
        });
        assert_eq!(
            wire::encode(&outcome).expect("encode direct body"),
            wire::encode(&outcome).expect("encode direct body again")
        );
    }

    #[test]
    fn gate_report_refuses_foreign_unknown_alternate_and_inconsistent_fields() {
        let report = current_gate_report();
        let encoded = report.encode().expect("encode gate report");
        let value: serde_json::Value = serde_json::from_slice(&encoded).expect("gate report JSON");

        let mut foreign = value.clone();
        foreign["version"] = json!(77);
        let refused =
            GateReport::decode(&json_bytes(&foreign)).expect_err("foreign gate report refuses");
        assert_eq!(refused.unsupported_version(), Some(77));

        let mut unknown = value.clone();
        unknown
            .as_object_mut()
            .expect("report object")
            .insert("argv".to_owned(), json!(["untrusted"]));
        assert!(matches!(
            GateReport::decode(&json_bytes(&unknown)),
            Err(ReportDecodeError::Json(_))
        ));

        let nested_unknown = mutate(&value, |value| {
            value["findings"][0]["movement"]
                .as_object_mut()
                .expect("movement object")
                .insert("unknown".to_owned(), json!(true));
        });
        assert!(matches!(
            GateReport::decode(&json_bytes(&nested_unknown)),
            Err(ReportDecodeError::Json(_))
        ));

        let null_tightening = mutate(&value, |value| value["tightening"] = json!(null));
        assert!(matches!(
            GateReport::decode(&json_bytes(&null_tightening)),
            Err(ReportDecodeError::Json(_))
        ));

        let mut alternate = vec![b' '];
        alternate.extend_from_slice(&encoded);
        let refused = GateReport::decode(&alternate).expect_err("alternate spelling refuses");
        assert_eq!(refused.canonical(), Some(encoded.as_slice()));

        for (field, changed) in [
            (
                "standing",
                mutate(&value, |value| value["standing"] = json!("refused")),
            ),
            (
                "findings",
                mutate(&value, |value| value["findings"] = json!([])),
            ),
            (
                "measurements",
                mutate(&value, |value| {
                    value["measurements"][0]["value"] = json!(999)
                }),
            ),
            (
                "tightening",
                mutate(&value, |value| {
                    value
                        .as_object_mut()
                        .expect("report object")
                        .remove("tightening");
                }),
            ),
            (
                "verdict",
                mutate(&value, |value| {
                    value["verdict"]["standing"] = json!("refused")
                }),
            ),
        ] {
            assert!(
                matches!(
                    GateReport::decode(&json_bytes(&changed)),
                    Err(ReportDecodeError::Inconsistent { .. })
                ),
                "gate report field {field} escaped coherence validation"
            );
        }
    }

    #[test]
    fn every_gate_report_field_moves_its_canonical_document() {
        let report = current_gate_report();
        let encoded = report.encode().expect("encode original report");
        let assert_moved = |changed: GateReport, field: &str| {
            assert_ne!(
                changed.encode().expect("encode changed report"),
                encoded,
                "gate report field {field} is not retained"
            );
        };

        let mut changed = report.clone();
        changed.version += 1;
        assert_moved(changed, "version");
        let mut changed = report.clone();
        changed.standing = Standing::Refused;
        assert_moved(changed, "standing");
        let mut changed = report.clone();
        changed.findings[0].metric.push('!');
        assert_moved(changed, "finding.metric");
        let mut changed = report.clone();
        changed.findings[0].movement = Movement::Held { baseline: u64::MAX };
        assert_moved(changed, "finding.movement");
        let mut changed = report.clone();
        changed.measurements[0].metric.push('!');
        assert_moved(changed, "measurement.metric");
        let mut changed = report.clone();
        changed.measurements[0].value += 1;
        assert_moved(changed, "measurement.value");
        let mut changed = report.clone();
        changed.measurements[0].files.push("other.rs".to_owned());
        assert_moved(changed, "measurement.files");
        let mut changed = report.clone();
        changed.tightening.as_mut().expect("clean proposal")[0]
            .metric
            .push('!');
        assert_moved(changed, "floor.metric");
        let mut changed = report.clone();
        changed.tightening.as_mut().expect("clean proposal")[0].value += 1;
        assert_moved(changed, "floor.value");
        let mut changed = report;
        let mut verdict = serde_json::to_value(&changed.verdict).expect("verdict JSON value");
        verdict["standing"] = json!("refused");
        changed.verdict = serde_json::from_value(verdict).expect("changed typed verdict");
        assert_moved(changed, "verdict");
    }

    #[test]
    fn kill_report_round_trips_every_complete_and_incomplete_evidence_shape() {
        let report = sample_kill_report();
        let encoded = report.encode().expect("encode kill report");
        let decoded = KillReport::decode(&encoded).expect("decode report");
        assert_eq!(decoded, report);
        assert_eq!(report.encode().expect("encode again"), encoded);
        assert_eq!(decoded.render(), report.render());
        assert_eq!(report.rate(), Some(rate(1, 2)));
        assert_eq!(report.proposed_floor(), Some(rate(1, 2)));
        let outcome = Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body: report.clone(),
        });
        assert_eq!(
            wire::encode(&outcome).expect("encode direct kill body"),
            wire::encode(&outcome).expect("encode direct kill body again")
        );

        let incomplete = sample_unconfirmed_report();
        let encoded = incomplete.encode().expect("encode incomplete report");
        let value: serde_json::Value =
            serde_json::from_slice(&encoded).expect("incomplete report JSON");
        assert!(value.get("rate").is_none());
        assert!(value.get("proposed_floor").is_none());
        assert_eq!(
            KillReport::decode(&encoded).expect("decode incomplete report"),
            incomplete
        );
        assert_eq!(incomplete.rate(), None);
        assert_eq!(incomplete.proposed_floor(), None);
        let text = incomplete.render();
        assert!(text.contains("stdout captured truncated true complete true bytes 0102"));
        assert!(text.contains("stderr unavailable"));
        assert!(text.contains("outcome unreadable \"unreadable artifact\""));

        let mut truncated_stderr = incomplete.clone();
        truncated_stderr.mutants[0].attempts[0].process.stdout = captured(Vec::new());
        truncated_stderr.mutants[0].attempts[0].process.stderr = ReportStream::Captured {
            bytes: vec![3, 4],
            truncated: true,
            complete: false,
        };
        truncated_stderr.mutants[0].attempts[0].outcome = ReportOutcome::Absent;
        let encoded = truncated_stderr
            .encode()
            .expect("encode truncated stderr report");
        assert_eq!(
            KillReport::decode(&encoded).expect("decode truncated stderr report"),
            truncated_stderr
        );
        let text = truncated_stderr.render();
        assert!(text.contains("stderr captured truncated true complete false bytes 0304"));
        assert!(text.contains("outcome absent"));

        let mut no_exit_code = incomplete.clone();
        no_exit_code.mutants[0].attempts[0].kind = ReportAttemptKind::TimedOut;
        no_exit_code.mutants[0].attempts[0].process.completion =
            ReportCompletion::TimedOut { code: None };
        no_exit_code.mutants[0].attempts[0].outcome = ReportOutcome::Absent;
        let encoded = no_exit_code.encode().expect("encode absent exit code");
        let value: serde_json::Value =
            serde_json::from_slice(&encoded).expect("absent exit code JSON");
        assert!(
            value["mutants"][0]["attempts"][0]["process"]["completion"]
                .get("code")
                .is_none()
        );
        assert_eq!(
            KillReport::decode(&encoded).expect("decode absent exit code"),
            no_exit_code
        );

        let mut regressed = sample_kill_report();
        regressed.movement = ReportMutationMovement::Regressed {
            floor: rate(0, 1),
            measured: rate(1, 2),
        };
        regressed.proposed_floor = None;
        let encoded = regressed.encode().expect("encode regression report");
        assert_eq!(
            KillReport::decode(&encoded).expect("decode regression report"),
            regressed
        );
        assert_eq!(regressed.proposed_floor(), None);
    }

    #[test]
    fn every_kill_report_field_moves_its_canonical_document() {
        let report = sample_kill_report();
        assert_kill_inventory_fields_move(&report);
        assert_kill_attempt_fields_move(&report);
        assert_kill_evidence_fields_move(&report);
        assert_kill_rate_fields_move(&report);
        assert_unconfirmed_movement_fields_move();
    }

    fn assert_kill_inventory_fields_move(report: &KillReport) {
        let mut changed = report.clone();
        changed.version += 1;
        assert_kill_moved(report, changed, "version");
        let mut changed = report.clone();
        changed.inventory.tool.push('!');
        assert_kill_moved(report, changed, "inventory.tool");
        let mut changed = report.clone();
        changed.inventory.targets[0].push('!');
        assert_kill_moved(report, changed, "inventory.targets");
        let mut changed = report.clone();
        changed.inventory.mutants[0].push('!');
        assert_kill_moved(report, changed, "inventory.mutants");
        let mut changed = report.clone();
        changed.inventory.attempts += 1;
        assert_kill_moved(report, changed, "inventory.attempts");
        let mut changed = report.clone();
        changed.inventory.seed += 1;
        assert_kill_moved(report, changed, "inventory.seed");
        let mut changed = report.clone();
        changed.inventory.timeout_seconds += 1;
        assert_kill_moved(report, changed, "inventory.timeout_seconds");
        let mut changed = report.clone();
        changed.baseline.completion = ReportCompletion::Exited { code: Some(1) };
        assert_kill_moved(report, changed, "baseline.completion.code");
        let mut changed = report.clone();
        changed.baseline.stdout = ReportStream::Unavailable;
        assert_kill_moved(report, changed, "baseline.stdout");
        let mut changed = report.clone();
        changed.baseline.stderr = ReportStream::Unavailable;
        assert_kill_moved(report, changed, "baseline.stderr");
        let mut changed = report.clone();
        changed.mutants[0].identity.push('!');
        assert_kill_moved(report, changed, "mutants.identity");
        let mut changed = report.clone();
        changed.mutants[0].disposition = ReportMutantDisposition::Unconfirmed;
        assert_kill_moved(report, changed, "mutants.disposition");
    }

    fn assert_kill_attempt_fields_move(report: &KillReport) {
        let mut changed = report.clone();
        changed.mutants[0].attempts[0].number += 1;
        assert_kill_moved(report, changed, "attempt.number");
        let mut changed = report.clone();
        changed.mutants[0].attempts[0].seed += 1;
        assert_kill_moved(report, changed, "attempt.seed");
        let mut changed = report.clone();
        changed.mutants[0].attempts[0].kind = ReportAttemptKind::Undecodable;
        assert_kill_moved(report, changed, "attempt.kind");
        let mut changed = report.clone();
        changed.mutants[0].attempts[0].process.completion =
            ReportCompletion::Exited { code: Some(1) };
        assert_kill_moved(report, changed, "attempt.process.completion");
        let mut changed = report.clone();
        if let ReportStream::Captured { bytes, .. } =
            &mut changed.mutants[0].attempts[0].process.stdout
        {
            bytes.push(b'!');
        }
        assert_kill_moved(report, changed, "attempt.process.stdout.bytes");
        let mut changed = report.clone();
        if let ReportStream::Captured { truncated, .. } =
            &mut changed.mutants[0].attempts[0].process.stdout
        {
            *truncated = true;
        }
        assert_kill_moved(report, changed, "attempt.process.stdout.truncated");
        let mut changed = report.clone();
        if let ReportStream::Captured { complete, .. } =
            &mut changed.mutants[0].attempts[0].process.stdout
        {
            *complete = false;
        }
        assert_kill_moved(report, changed, "attempt.process.stdout.complete");
        let mut changed = report.clone();
        changed.mutants[0].attempts[0].process.stderr = ReportStream::Unavailable;
        assert_kill_moved(report, changed, "attempt.process.stderr");
    }

    fn assert_kill_evidence_fields_move(report: &KillReport) {
        let mut launch = report.clone();
        launch.mutants[0].attempts[0].process.completion = ReportCompletion::LaunchError {
            evidence: "launch evidence".to_owned(),
        };
        let encoded = launch.encode().expect("encode launch evidence");
        if let ReportCompletion::LaunchError { evidence } =
            &mut launch.mutants[0].attempts[0].process.completion
        {
            evidence.push('!');
        }
        assert_ne!(
            launch.encode().expect("encode changed launch evidence"),
            encoded,
            "attempt.process.completion.evidence is not retained"
        );

        let mut changed = report.clone();
        if let ReportOutcome::Present { bytes } = &mut changed.mutants[0].attempts[0].outcome {
            bytes.push(b'!');
        }
        assert_kill_moved(report, changed, "attempt.outcome.bytes");
        let mut unreadable = report.clone();
        unreadable.mutants[0].attempts[0].outcome = ReportOutcome::Unreadable {
            evidence: "outcome evidence".to_owned(),
        };
        let encoded = unreadable.encode().expect("encode unreadable evidence");
        if let ReportOutcome::Unreadable { evidence } =
            &mut unreadable.mutants[0].attempts[0].outcome
        {
            evidence.push('!');
        }
        assert_ne!(
            unreadable
                .encode()
                .expect("encode changed outcome evidence"),
            encoded,
            "attempt.outcome.evidence is not retained"
        );
    }

    fn assert_kill_rate_fields_move(report: &KillReport) {
        let mut changed = report.clone();
        changed.rate = Some(rate(0, 1));
        assert_kill_moved(report, changed, "rate.numerator");
        let mut changed = report.clone();
        changed.rate.as_mut().expect("confirmed rate").denominator += 1;
        assert_kill_moved(report, changed, "rate.denominator");
        let mut changed = report.clone();
        if let ReportMutationMovement::Shrank { floor, .. } = &mut changed.movement {
            floor.numerator += 1;
        }
        assert_kill_moved(report, changed, "movement.floor.numerator");
        let mut changed = report.clone();
        if let ReportMutationMovement::Shrank { floor, .. } = &mut changed.movement {
            floor.denominator += 1;
        }
        assert_kill_moved(report, changed, "movement.floor.denominator");
        let mut changed = report.clone();
        if let ReportMutationMovement::Shrank { measured, .. } = &mut changed.movement {
            measured.numerator += 1;
        }
        assert_kill_moved(report, changed, "movement.measured.numerator");
        let mut changed = report.clone();
        if let ReportMutationMovement::Shrank { measured, .. } = &mut changed.movement {
            measured.denominator += 1;
        }
        assert_kill_moved(report, changed, "movement.measured.denominator");
        let mut changed = report.clone();
        changed.proposed_floor = Some(rate(0, 1));
        assert_kill_moved(report, changed, "proposed_floor.numerator");
        let mut changed = report.clone();
        changed
            .proposed_floor
            .as_mut()
            .expect("current proposal")
            .denominator += 1;
        assert_kill_moved(report, changed, "proposed_floor.denominator");
    }

    fn assert_unconfirmed_movement_fields_move() {
        let unconfirmed = sample_unconfirmed_report();
        let encoded = unconfirmed.encode().expect("encode unconfirmed report");
        let mut changed = unconfirmed.clone();
        if let ReportMutationMovement::Unconfirmed { confirmed, .. } = &mut changed.movement {
            *confirmed += 1;
        }
        assert_ne!(
            changed.encode().expect("encode changed unconfirmed report"),
            encoded,
            "movement.confirmed is not retained"
        );
        let mut changed = unconfirmed;
        if let ReportMutationMovement::Unconfirmed { total, .. } = &mut changed.movement {
            *total += 1;
        }
        assert_ne!(
            changed.encode().expect("encode changed unconfirmed report"),
            encoded,
            "movement.total is not retained"
        );
    }

    fn assert_kill_moved(report: &KillReport, changed: KillReport, field: &str) {
        assert_ne!(
            changed.encode().expect("encode changed report"),
            report.encode().expect("encode original report"),
            "kill report field {field} is not retained"
        );
    }

    #[test]
    fn kill_report_refuses_foreign_unknown_alternate_and_inconsistent_fields() {
        let report = sample_kill_report();
        let encoded = report.encode().expect("encode kill report");
        let value: serde_json::Value = serde_json::from_slice(&encoded).expect("kill report JSON");

        let mut foreign = value.clone();
        foreign["version"] = json!(77);
        let refused =
            KillReport::decode(&json_bytes(&foreign)).expect_err("foreign kill report refuses");
        assert_eq!(refused.unsupported_version(), Some(77));

        let mut unknown = value.clone();
        unknown
            .as_object_mut()
            .expect("report object")
            .insert("program".to_owned(), json!("/usr/bin/cargo"));
        assert!(matches!(
            KillReport::decode(&json_bytes(&unknown)),
            Err(ReportDecodeError::Json(_))
        ));

        assert_kill_nulls_refuse(&value);
        assert_kill_nested_unknowns_refuse(&value);

        let mut alternate = vec![b'\n'];
        alternate.extend_from_slice(&encoded);
        let refused = KillReport::decode(&alternate).expect_err("alternate spelling refuses");
        assert_eq!(refused.canonical(), Some(encoded.as_slice()));

        assert_kill_inconsistencies_refuse(&value);
    }

    fn assert_kill_nulls_refuse(value: &serde_json::Value) {
        for (field, null) in [
            (
                "completion.code",
                mutate(value, |value| {
                    value["baseline"]["completion"]["code"] = json!(null);
                }),
            ),
            ("rate", mutate(value, |value| value["rate"] = json!(null))),
            (
                "proposed_floor",
                mutate(value, |value| value["proposed_floor"] = json!(null)),
            ),
        ] {
            assert!(
                matches!(
                    KillReport::decode(&json_bytes(&null)),
                    Err(ReportDecodeError::Json(_))
                ),
                "null {field} admitted a second absence spelling"
            );
        }
    }

    fn assert_kill_nested_unknowns_refuse(value: &serde_json::Value) {
        for (field, nested_unknown) in [
            (
                "stream",
                mutate(value, |value| {
                    value["baseline"]["stdout"]
                        .as_object_mut()
                        .expect("stream object")
                        .insert("unknown".to_owned(), json!(true));
                }),
            ),
            (
                "completion",
                mutate(value, |value| {
                    value["baseline"]["completion"]
                        .as_object_mut()
                        .expect("completion object")
                        .insert("unknown".to_owned(), json!(true));
                }),
            ),
            (
                "outcome",
                mutate(value, |value| {
                    value["mutants"][0]["attempts"][0]["outcome"]
                        .as_object_mut()
                        .expect("outcome object")
                        .insert("unknown".to_owned(), json!(true));
                }),
            ),
            (
                "movement",
                mutate(value, |value| {
                    value["movement"]
                        .as_object_mut()
                        .expect("movement object")
                        .insert("unknown".to_owned(), json!(true));
                }),
            ),
        ] {
            assert!(
                matches!(
                    KillReport::decode(&json_bytes(&nested_unknown)),
                    Err(ReportDecodeError::Json(_))
                ),
                "unknown {field} field was ignored"
            );
        }
    }

    fn assert_kill_inconsistencies_refuse(value: &serde_json::Value) {
        for (field, changed) in [
            (
                "inventory",
                mutate(value, |value| value["inventory"]["attempts"] = json!(2)),
            ),
            (
                "baseline",
                mutate(value, |value| {
                    value["baseline"]["completion"]["code"] = json!(1);
                }),
            ),
            (
                "mutants",
                mutate(value, |value| {
                    value["mutants"][0]["disposition"] = json!("unconfirmed");
                }),
            ),
            (
                "rate",
                mutate(value, |value| {
                    value.as_object_mut().expect("report object").remove("rate");
                }),
            ),
            (
                "movement",
                mutate(value, |value| {
                    value["movement"] = json!({
                        "movement": "unconfirmed",
                        "confirmed": 0,
                        "total": 2
                    });
                }),
            ),
            (
                "proposed_floor",
                mutate(value, |value| {
                    value
                        .as_object_mut()
                        .expect("report object")
                        .remove("proposed_floor");
                }),
            ),
        ] {
            assert!(
                matches!(
                    KillReport::decode(&json_bytes(&changed)),
                    Err(ReportDecodeError::Inconsistent { .. })
                ),
                "kill report field {field} escaped coherence validation"
            );
        }
    }

    #[test]
    fn gate_report_has_no_journal_dependency() {
        let manifest = include_str!("../../Cargo.toml");
        assert!(
            !manifest
                .lines()
                .any(|line| { line.trim_start().starts_with("journal") })
        );
    }

    fn current_gate_report() -> GateReport {
        let root = workspace_root();
        let contract = Contract::load(&root).expect("load current contract");
        contract
            .adjudicate(&root)
            .expect("complete current run")
            .report()
    }

    fn workspace_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("gate is a direct workspace member")
            .to_path_buf()
    }

    fn sample_kill_report() -> KillReport {
        let mutants = vec![
            sample_mutant("Killed", ReportAttemptKind::Killed, 0, caught()),
            sample_mutant("Survived", ReportAttemptKind::Survived, 2, survived()),
        ];
        KillReport {
            version: KILL_REPORT_VERSION,
            inventory: ReportMutationInventory {
                tool: PINNED_CARGO_MUTANTS.to_owned(),
                targets: vec!["src/lib.rs".to_owned()],
                mutants: vec!["Killed".to_owned(), "Survived".to_owned()],
                attempts: 1,
                seed: 41,
                timeout_seconds: 10,
            },
            baseline: green_process(0),
            mutants,
            rate: Some(rate(1, 2)),
            movement: ReportMutationMovement::Shrank {
                floor: rate(3, 4),
                measured: rate(1, 2),
            },
            proposed_floor: Some(rate(1, 2)),
        }
    }

    fn sample_unconfirmed_report() -> KillReport {
        let attempt = ReportAttempt {
            number: 1,
            seed: 41,
            kind: ReportAttemptKind::Undecodable,
            process: ReportProcess {
                completion: ReportCompletion::Exited { code: Some(0) },
                stdout: ReportStream::Captured {
                    bytes: vec![1, 2],
                    truncated: true,
                    complete: true,
                },
                stderr: ReportStream::Unavailable,
            },
            outcome: ReportOutcome::Unreadable {
                evidence: "unreadable artifact".to_owned(),
            },
        };
        KillReport {
            version: KILL_REPORT_VERSION,
            inventory: ReportMutationInventory {
                tool: PINNED_CARGO_MUTANTS.to_owned(),
                targets: vec!["src/lib.rs".to_owned()],
                mutants: vec!["Odd".to_owned()],
                attempts: 1,
                seed: 41,
                timeout_seconds: 10,
            },
            baseline: green_process(0),
            mutants: vec![ReportMutant {
                identity: "Odd".to_owned(),
                attempts: vec![attempt],
                disposition: ReportMutantDisposition::Unconfirmed,
            }],
            rate: None,
            movement: ReportMutationMovement::Unconfirmed {
                confirmed: 0,
                total: 1,
            },
            proposed_floor: None,
        }
    }

    fn sample_mutant(
        identity: &str,
        kind: ReportAttemptKind,
        exit: i32,
        outcome: Vec<u8>,
    ) -> ReportMutant {
        ReportMutant {
            identity: identity.to_owned(),
            attempts: vec![ReportAttempt {
                number: 1,
                seed: 41,
                kind,
                process: green_process(exit),
                outcome: ReportOutcome::Present { bytes: outcome },
            }],
            disposition: match kind {
                ReportAttemptKind::Killed => ReportMutantDisposition::ConfirmedKilled,
                ReportAttemptKind::Survived => ReportMutantDisposition::ConfirmedSurvived,
                ReportAttemptKind::TimedOut
                | ReportAttemptKind::LaunchError
                | ReportAttemptKind::WaitError
                | ReportAttemptKind::Undecodable => ReportMutantDisposition::Unconfirmed,
            },
        }
    }

    fn green_process(code: i32) -> ReportProcess {
        ReportProcess {
            completion: ReportCompletion::Exited { code: Some(code) },
            stdout: captured(Vec::new()),
            stderr: captured(Vec::new()),
        }
    }

    fn captured(bytes: Vec<u8>) -> ReportStream {
        ReportStream::Captured {
            bytes,
            truncated: false,
            complete: true,
        }
    }

    fn caught() -> Vec<u8> {
        outcome("CaughtMutant", 0, 1)
    }

    fn survived() -> Vec<u8> {
        outcome("MissedMutant", 1, 0)
    }

    fn outcome(summary: &str, missed: u64, caught: u64) -> Vec<u8> {
        format!(
            concat!(
                "{{\"cargo_mutants_version\":\"27.1.0\",",
                "\"total_mutants\":1,\"missed\":{},",
                "\"caught\":{},\"timeout\":0,\"unviable\":0,",
                "\"outcomes\":[{{\"summary\":\"{}\"}}]}}"
            ),
            missed, caught, summary
        )
        .into_bytes()
    }

    const fn rate(numerator: u64, denominator: u64) -> ReportRate {
        ReportRate {
            numerator,
            denominator,
        }
    }

    fn mutate(
        value: &serde_json::Value,
        mutation: impl FnOnce(&mut serde_json::Value),
    ) -> serde_json::Value {
        let mut changed = value.clone();
        mutation(&mut changed);
        changed
    }

    fn json_bytes(value: &serde_json::Value) -> Vec<u8> {
        serde_json::to_vec(value).expect("encode JSON mutation")
    }
}

use serde_json::{Value, json};

use super::{MigrationErrorKind, PredecessorTaskMigration, migrate_predecessor_tasks};

const SOURCE: &str = "/exports/release-0.31.0-tasks.json";

fn migrate(value: Value) -> Result<super::TaskMigrationReport, super::MigrationError> {
    let bytes = serde_json::to_vec(&value).expect("fixture JSON encodes");
    migrate_predecessor_tasks(PredecessorTaskMigration::new(SOURCE, &bytes))
}

#[test]
fn accepted_export_has_byte_stable_text_and_json() {
    let report = migrate(json!({
        "version": 1,
        "release": "0.31.0",
        "tasks": {
            "hidden_stems": ["sandbox", "oops", "audit", "sandbox"]
        },
        "teams": [
            {
                "teamId": "alpha",
                "shellSettings": {"rc": "rc.report.next.filter=bad", "taskRc": ""}
            },
            {"teamId": "beta", "shellSettings": {"rc": []}},
            {"teamId": "gamma", "shellSettings": {"taskRc": [""]}},
            {"teamId": "delta"}
        ]
    }))
    .expect("empty selected overrides and hidden roots migrate");

    assert_eq!(
        report.tasks().hidden_stems(),
        ["oops", "maxim_proposal", "sandbox", "audit"]
    );
    assert_eq!(report.provenance().len(), 9);

    let text =
        String::from_utf8(report.text().expect("report renders as TOML")).expect("TOML is UTF-8");
    assert_eq!(text, include_str!("fixtures/accepted.toml"));
    let encoded = serde_json::to_string_pretty(&report).expect("report renders as JSON");
    assert_eq!(encoded, include_str!("fixtures/accepted.json").trim_end());
}

#[test]
fn absent_and_empty_override_forms_follow_exact_precedence() {
    for shell_settings in [
        json!({}),
        json!({"rc": ""}),
        json!({"rc": "   "}),
        json!({"rc": []}),
        json!({"taskRc": ""}),
        json!({"taskRc": ["", "  "]}),
        json!({"taskRc": []}),
        json!({"rc": "rc.unsupported=yes", "taskRc": ""}),
        json!({"rc": ["rc.unsupported=yes"], "taskRc": []}),
    ] {
        migrate(json!({
            "version": 1,
            "release": "0.31.0",
            "teams": [{"teamId": "lineage-a", "shellSettings": shell_settings}]
        }))
        .unwrap_or_else(|error| panic!("selected empty override must migrate: {error}"));
    }
}

#[test]
fn override_prefix_classification_uses_predecessor_trimmed_text() {
    let unsupported = migrate(json!({
        "version": 1,
        "release": "0.31.0",
        "teams": [{
            "teamId": "red",
            "shellSettings": {"rc": "  rc.gc=off  "}
        }]
    }))
    .expect_err("a padded raw override remains an unsupported rc token");
    assert_eq!(unsupported.kind(), MigrationErrorKind::UnsupportedOverride);
    assert_eq!(unsupported.token(), Some("  rc.gc=off  "));

    let malformed = migrate(json!({
        "version": 1,
        "release": "0.31.0",
        "teams": [{
            "teamId": "red",
            "shellSettings": {"taskRc": "  report.next=yes  "}
        }]
    }))
    .expect_err("a padded token without an rc prefix remains malformed");
    assert_eq!(malformed.kind(), MigrationErrorKind::MalformedOverride);
    assert_eq!(malformed.token(), Some("  report.next=yes  "));
}

#[test]
fn every_nonempty_token_refuses_in_source_order_with_complete_identity() {
    let first = migrate(json!({
        "version": 1,
        "release": "0.31.0",
        "teams": [{
            "teamId": "red",
            "shellSettings": {"rc": ["rc.report.next.sort=urgency-", "rc.gc=off"]}
        }]
    }))
    .expect_err("first raw override refuses");
    assert_eq!(first.kind(), MigrationErrorKind::UnsupportedOverride);
    assert_eq!(first.lineage(), Some("red"));
    assert_eq!(first.field(), "teams[0].shellSettings.rc[0]");
    assert_eq!(first.list_index(), Some(0));
    assert_eq!(first.token(), Some("rc.report.next.sort=urgency-"));
    assert_eq!(first.source_file(), SOURCE);
    assert!(first.to_string().contains("release=0.31.0"));
    assert!(first.to_string().contains("lineage=red"));
    assert!(first.recovery().contains("future named Rust task field"));

    let second = migrate(json!({
        "version": 1,
        "release": "0.31.0",
        "teams": [{
            "teamId": "red",
            "shellSettings": {"taskRc": ["", "rc.gc=off"]}
        }]
    }))
    .expect_err("second raw override refuses after the empty first token");
    assert_eq!(second.field(), "teams[0].shellSettings.taskRc[1]");
    assert_eq!(second.list_index(), Some(1));
    assert_eq!(second.token(), Some("rc.gc=off"));
}

#[test]
fn malformed_prefix_and_every_wrong_shape_are_typed_refusals() {
    let malformed = migrate(json!({
        "version": 1,
        "release": "0.31.0",
        "teams": [{"teamId": "red", "shellSettings": {"rc": "report.next=yes"}}]
    }))
    .expect_err("non-rc prefix refuses distinctly");
    assert_eq!(malformed.kind(), MigrationErrorKind::MalformedOverride);
    assert_eq!(malformed.token(), Some("report.next=yes"));

    for (shell_settings, field, index) in [
        (json!({"rc": false}), "teams[0].shellSettings.rc", None),
        (json!({"taskRc": {}}), "teams[0].shellSettings.taskRc", None),
        (
            json!({"rc": ["", 7]}),
            "teams[0].shellSettings.rc[1]",
            Some(1),
        ),
    ] {
        let error = migrate(json!({
            "version": 1,
            "release": "0.31.0",
            "teams": [{"teamId": "red", "shellSettings": shell_settings}]
        }))
        .expect_err("wrong override shape refuses");
        assert_eq!(error.kind(), MigrationErrorKind::Malformed);
        assert_eq!(error.field(), field);
        assert_eq!(error.list_index(), index);
        assert!(error.token().is_none());
    }
}

#[test]
fn lineage_scoped_schema_and_duplicate_refusals_retain_known_identity() {
    let unknown = migrate(json!({
        "version": 1,
        "release": "0.31.0",
        "teams": [{"teamId": "red", "extra": true}]
    }))
    .expect_err("unknown lineage field refuses");
    assert_eq!(unknown.kind(), MigrationErrorKind::Schema);
    assert_eq!(unknown.lineage(), Some("red"));
    assert_eq!(unknown.field(), "teams[0].extra");

    let duplicate = migrate(json!({
        "version": 1,
        "release": "0.31.0",
        "teams": [{"teamId": "red"}, {"teamId": "red"}]
    }))
    .expect_err("duplicate lineage identity refuses");
    assert_eq!(duplicate.kind(), MigrationErrorKind::Malformed);
    assert_eq!(duplicate.lineage(), Some("red"));
    assert_eq!(duplicate.field(), "teams[1].teamId");
}

#[test]
fn hidden_stems_use_current_indexed_validation_and_public_conflicts() {
    for (stems, index, expected) in [
        (json!(["sandbox", "bad-stem"]), 1, "must match"),
        (json!(["task"]), 0, "conflicts with public stem"),
        (json!(["serve"]), 0, "conflicts with public stem"),
        (json!(["sandbox", 4]), 1, "expected a string"),
    ] {
        let error = migrate(json!({
            "version": 1,
            "release": "0.31.0",
            "tasks": {"hidden_stems": stems}
        }))
        .expect_err("invalid current hidden root refuses");
        assert_eq!(error.kind(), MigrationErrorKind::Malformed);
        assert_eq!(error.field(), format!("tasks.hidden_stems[{index}]"));
        assert!(error.to_string().contains(expected));
    }
}

#[test]
fn schema_release_and_unknown_fields_refuse_before_translation() {
    for (fixture, kind, field) in [
        (json!([]), MigrationErrorKind::Malformed, "<document>"),
        (
            json!({"release": "0.31.0"}),
            MigrationErrorKind::Schema,
            "version",
        ),
        (
            json!({"version": 2, "release": "0.31.0"}),
            MigrationErrorKind::Schema,
            "version",
        ),
        (
            json!({"version": 1, "release": "0.30.0"}),
            MigrationErrorKind::Release,
            "release",
        ),
        (
            json!({"version": 1, "release": "0.31.0", "extra": true}),
            MigrationErrorKind::Schema,
            "extra",
        ),
    ] {
        let error = migrate(fixture).expect_err("invalid export schema refuses");
        assert_eq!(error.kind(), kind);
        assert_eq!(error.field(), field);
    }

    let malformed = migrate_predecessor_tasks(PredecessorTaskMigration::new(SOURCE, b"{"))
        .expect_err("malformed JSON refuses");
    assert_eq!(malformed.kind(), MigrationErrorKind::Syntax);
    assert_eq!(malformed.field(), "<document>");
}

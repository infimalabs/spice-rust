//! Executable enforcement for the refusal sum type.

use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::Visit as _;
use wire::{Refusal, SpiceCommand};

#[test]
fn each_form_requires_its_own_fields_and_rejects_every_other_shape() {
    let cases = [
        (
            serde_json::json!({
                "form": "repairable",
                "command": ["task", "next"],
                "diagnostic": {"reason": "claim::not-held"}
            }),
            &["command", "diagnostic"][..],
            "contract",
        ),
        (
            serde_json::json!({
                "form": "bare",
                "diagnostic": {"reason": "claim::not-held"}
            }),
            &["diagnostic"][..],
            "command",
        ),
        (
            serde_json::json!({
                "form": "invocation",
                "reason": "gate::instrument-unregistered",
                "contract": "spice gate findings INSTRUMENT requires one registered instrument"
            }),
            &["reason", "contract"][..],
            "diagnostic",
        ),
        (
            serde_json::json!({
                "form": "evidence",
                "reason": "gate::regressed",
                "evidence": "file-shape: regressed",
                "summary": "repair the reported regression"
            }),
            &["reason", "evidence", "summary"][..],
            "command",
        ),
    ];

    for (document, required, foreign) in cases {
        let bytes = serde_json::to_vec(&document).expect("fixture encodes");
        wire::decode::<Refusal>(&bytes).expect("complete form decodes");

        for field in required {
            let mut missing = document.clone();
            missing
                .as_object_mut()
                .expect("form is an object")
                .remove(*field);
            let bytes = serde_json::to_vec(&missing).expect("missing fixture encodes");
            assert!(
                wire::decode::<Refusal>(&bytes).is_err(),
                "{} decoded without required field {field}",
                document["form"]
            );
        }

        let mut mixed = document.clone();
        mixed.as_object_mut().expect("form is an object").insert(
            foreign.to_owned(),
            serde_json::Value::String("foreign".to_owned()),
        );
        let bytes = serde_json::to_vec(&mixed).expect("mixed fixture encodes");
        assert!(
            wire::decode::<Refusal>(&bytes).is_err(),
            "{} admitted foreign arm field {foreign}",
            document["form"]
        );
    }

    for invalid in [
        serde_json::json!({"form": "future", "reason": "unknown"}),
        serde_json::json!({"reason": "legacy", "remedy": "retry"}),
    ] {
        let bytes = serde_json::to_vec(&invalid).expect("invalid fixture encodes");
        assert!(
            wire::decode::<Refusal>(&bytes).is_err(),
            "an untagged, unknown, or legacy shape decoded: {invalid}"
        );
    }
}

#[test]
fn repair_commands_cannot_be_empty_or_option_shaped() {
    assert!(SpiceCommand::new("task", ["next"]).is_ok());
    assert!(SpiceCommand::new("", std::iter::empty::<&str>()).is_err());
    assert!(SpiceCommand::new("--json", ["task", "next"]).is_err());
    assert!(SpiceCommand::new("task next", std::iter::empty::<&str>()).is_err());
    assert!(SpiceCommand::new("task", ["show", "bad\0handle"]).is_err());

    for document in [
        "[]",
        "[\"\"]",
        "[\"--json\"]",
        "[\"task next\"]",
        "[\"task\",\"bad\\u0000handle\"]",
    ] {
        assert!(
            wire::decode::<SpiceCommand>(document.as_bytes()).is_err(),
            "dummy recovery command decoded: {document}"
        );
    }
}

#[test]
fn production_refusals_use_typed_constructors_and_the_shared_renderer() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("wire crate has a workspace parent");
    let mut sources = Vec::new();
    collect_sources(workspace, &mut sources);
    sources.sort();

    let mut bypasses = Vec::new();
    let mut local_renderers = Vec::new();
    for path in sources {
        let source = fs::read_to_string(&path).expect("Rust source is readable");
        let syntax = syn::parse_file(&source).expect("production Rust parses");
        let mut audit = RefusalGrammarAudit {
            path: path.clone(),
            findings: Vec::new(),
        };
        audit.visit_file(&syntax);
        bypasses.extend(audit.findings);

        let is_renderer = path.ends_with("spice/src/render.rs");
        let is_test_fixture = path.ends_with("spice/src/gate_command/fixtures.rs");
        if !is_renderer && !is_test_fixture && source.contains("\"spice: ") {
            local_renderers.push(path.display().to_string());
        }
    }

    assert!(
        bypasses.is_empty(),
        "refusal construction or arm-specific reads bypass the shared grammar:\n{}",
        bypasses.join("\n")
    );
    assert!(
        local_renderers.is_empty(),
        "production modules format refusal text outside spice/src/render.rs:\n{}",
        local_renderers.join("\n")
    );
}

struct RefusalGrammarAudit {
    path: PathBuf,
    findings: Vec<String>,
}

impl<'ast> syn::visit::Visit<'ast> for RefusalGrammarAudit {
    fn visit_expr_struct(&mut self, node: &'ast syn::ExprStruct) {
        let segments = node
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>();
        let direct_arm = is_refusal_arm(&segments);
        let constructor_module = self.path.ends_with("wire/src/reply.rs");
        if direct_arm && !constructor_module {
            self.findings
                .push(format!("{}: {}", self.path.display(), segments.join("::")));
        }
        syn::visit::visit_expr_struct(self, node);
    }

    fn visit_pat_struct(&mut self, node: &'ast syn::PatStruct) {
        let segments = node
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>();
        let arm_specific = is_refusal_arm(&segments);
        let shared_grammar = self.path.ends_with("wire/src/reply.rs")
            || self.path.ends_with("spice/src/render.rs")
            || self.path.ends_with("spice/src/gate_command/fixtures.rs");
        if arm_specific && !shared_grammar {
            self.findings.push(format!(
                "{}: arm-specific read {}",
                self.path.display(),
                segments.join("::")
            ));
        }
        syn::visit::visit_pat_struct(self, node);
    }
}

fn is_refusal_arm(segments: &[String]) -> bool {
    segments.len() >= 2
        && segments[segments.len() - 2] == "Refusal"
        && matches!(
            segments.last().map(String::as_str),
            Some("Repairable" | "Bare" | "Invocation" | "Evidence")
        )
}

fn collect_sources(root: &Path, sources: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(root).expect("workspace directory is readable");
    for entry in entries {
        let path = entry.expect("workspace entry is readable").path();
        if path.is_dir() {
            if matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some(".git" | ".spice" | "target" | "tests")
            ) {
                continue;
            }
            collect_sources(&path, sources);
        } else if path.extension().is_some_and(|extension| extension == "rs")
            && path
                .components()
                .any(|component| component.as_os_str() == "src")
        {
            sources.push(path);
        }
    }
}

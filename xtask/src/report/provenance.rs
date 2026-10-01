//! Pinned applicability rules, not an invented per-row migration execution audit.

use super::publication::derived_json;
use super::{Result, hash_file};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize)]
pub(super) struct Rule {
    pub fields: String,
    pub source_module: String,
    pub schema_step: String,
    pub transformation: String,
    pub evidence_class: String,
}

pub(super) fn rules(directory: &Path) -> Result<Vec<Rule>> {
    let specifications = [
        (
            "Task birth, dependency identities and task origins",
            "journal/src/migrate/identity_stamp.rs",
            "5 → 6",
            "Rendered task handles contract onto birth stamps; task UUID copies disappear; task-origin realms and dependency arrays are rewritten.",
            "Potentially migration-normalized identity",
        ),
        (
            "Birth/vendor vocabulary in retained envelopes",
            "journal/src/migrate/vocabulary.rs",
            "8 → 9",
            "Retired incepted and driver keys are renamed born and vendor wherever retained authority can carry them.",
            "Potentially migration-normalized spelling",
        ),
        (
            "Claim-time cell and queue/claim provenance",
            "journal/src/migrate/task_allocation.rs",
            "3 → 4",
            "Queue and claim origins are backfilled on heads; legacy claim cells can be reconstructed from then-current task state. A retained claim cell is not universally original event-time evidence.",
            "Potentially migration-reconstructed allocation fields",
        ),
        (
            "Creation context and transcript provenance",
            "journal/src/migrate/creation_context.rs",
            "12 → 13",
            "Historical creation context may be reconstructed from retained authority. The report does not consume whole transcripts or equate birth stamp with observed admission time.",
            "Potentially migration-reconstructed provenance",
        ),
        (
            "Publication scope and creator lineage",
            "journal/src/migrate/task_creator_scope.rs",
            "18 → 19",
            "Retired project-backed lifetime-scope fields are removed from residency and lineage authority. Publication uses admitted scope; actual phase author and reviewer are independently observed in their facts.",
            "Migration-normalized authority scope",
        ),
    ];
    let root = spice::repository::Repository::discover(std::env::current_dir()?)?;
    let mut hashes = Vec::new();
    let mut rules = Vec::new();
    for (fields, module, step, transformation, class) in specifications {
        hashes.push(serde_json::json!({"module": module, "sha256": hash_file(&root.worktree_root().join(module))?}));
        rules.push(Rule {
            fields: fields.to_owned(),
            source_module: module.to_owned(),
            schema_step: step.to_owned(),
            transformation: transformation.to_owned(),
            evidence_class: class.to_owned(),
        });
    }
    rules.push(Rule { fields: "Published quantities and identity aliases".to_owned(), source_module: "Pinned report consumer".to_owned(),
        schema_step: "Publication schema 1".to_owned(), transformation: "Explicit allowlist, typed population reduction, day/hour disclosure granularity and stable report-local identities.".to_owned(), evidence_class: "Report-derived from frozen normalized evidence".to_owned() });
    rules.push(Rule { fields: "Requested correction flow in the curated case".to_owned(), source_module: "Retained non-journal command record".to_owned(),
        schema_step: "Private record contract 1".to_owned(), transformation: "Authored then operand is projected by the native receipt reader; exact retained request digest is private. Resulting flow and requested flow remain separate evidence fields.".to_owned(), evidence_class: "Retained command evidence; GC-censored, not a general request ledger".to_owned() });
    derived_json(
        &directory.join("provenance-rules-private.json"),
        &serde_json::json!({
            "version": 1, "rules": rules, "source_digests": hashes,
            "limits": "No general per-row migration audit or original-envelope archive. Current normalized bytes, decoder/schema identity, consumer and rule digests are the reproducibility asset. Git implementation dates do not date database migration execution."
        }),
    )?;
    Ok(rules)
}

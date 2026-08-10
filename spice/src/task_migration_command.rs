//! Daemonless rim for one explicit predecessor task export.

use std::fs;
use std::path::PathBuf;

use clap::ArgMatches;
use spice::config::{
    PredecessorTaskMigration, TaskMigrationError, TaskMigrationReport, migrate_predecessor_tasks,
};
use wire::{Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition};

pub(crate) fn run(matches: &ArgMatches) -> Outcome<TaskMigrationReport> {
    let path = PathBuf::from(
        matches
            .get_one::<String>("EXPORT")
            .expect("generated syntax requires the export path"),
    );
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return Outcome::Fault(Fault {
                class: FaultClass::Environment,
                reason: "task-migration::export-unavailable".to_owned(),
                detail: format!("cannot read {}: {error}", path.display()),
            });
        }
    };
    let source = path.display().to_string();
    match migrate_predecessor_tasks(PredecessorTaskMigration::new(&source, &bytes)) {
        Ok(report) => Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body: report,
        }),
        Err(error) => Outcome::Refusal(refusal(&error)),
    }
}

fn refusal(error: &TaskMigrationError) -> Refusal {
    let mut witness = format!("release=0.31.0 source={}", error.source_file());
    if let Some(lineage) = error.lineage() {
        witness.push_str(&format!(" lineage={lineage}"));
    }
    witness.push_str(&format!(" field={}", error.field()));
    if let Some(index) = error.list_index() {
        witness.push_str(&format!(" index={index}"));
    }
    if let Some(token) = error.token() {
        witness.push_str(&format!(" token={token:?}"));
    }
    Refusal::bare(wire::RefusalDiagnostic::new(
        format!("task-migration::{}", error.kind()),
        Some(witness),
        Some("a complete release-0.31.0 version-1 export with no raw task overrides".to_owned()),
        Some(error.to_string()),
    ))
}

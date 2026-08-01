use std::path::Path;

use wire::{Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition, RestoreVerification};

pub(crate) fn verify(matches: &clap::ArgMatches) -> Outcome<RestoreVerification> {
    let bundle = matches
        .get_one::<String>("BUNDLE")
        .expect("restore verify requires its generated BUNDLE operand");
    match journal::verify_restore_bundle(Path::new(bundle)) {
        Ok(body) => Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body,
        }),
        Err(error) => failure(error),
    }
}

fn failure(error: journal::BackupVerifyError) -> Outcome<RestoreVerification> {
    use journal::BackupVerifyError;

    match error {
        missing @ (BackupVerifyError::Missing { .. } | BackupVerifyError::Incomplete { .. }) => {
            Outcome::Refusal(Refusal {
                reason: "restore::bundle-incomplete".to_owned(),
                witness: Some("backup-inventory".to_owned()),
                expected: Some("one complete manifest-bound bundle".to_owned()),
                observed: Some(missing.to_string()),
                remedy: "restore every member named by the canonical backup manifest".to_owned(),
            })
        }
        version @ BackupVerifyError::UnsupportedVersion { .. } => Outcome::Refusal(Refusal {
            reason: "restore::bundle-version".to_owned(),
            witness: Some("backup-schema".to_owned()),
            expected: Some(wire::manifest::JOURNAL_BACKUP_VERSION.to_string()),
            observed: Some(version.to_string()),
            remedy: "verify the bundle with the Spice release that owns its backup schema"
                .to_owned(),
        }),
        schema @ (BackupVerifyError::ForwardSchema { .. }
        | BackupVerifyError::Authority(journal::OpenError::Migration(
            journal::MigrationError::ForwardSchema { .. },
        ))) => Outcome::Refusal(Refusal {
            reason: "restore::schema-forward".to_owned(),
            witness: Some("journal-schema".to_owned()),
            expected: Some(journal::SCHEMA_VERSION.to_string()),
            observed: Some(schema.to_string()),
            remedy: "verify the bundle with the Spice release that owns its journal schema"
                .to_owned(),
        }),
        unreadable @ BackupVerifyError::Io { .. } => Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "restore::bundle-unreadable".to_owned(),
            detail: unreadable.to_string(),
        }),
        invalid => Outcome::Fault(Fault {
            class: FaultClass::Store,
            reason: "restore::bundle-invalid".to_owned(),
            detail: invalid.to_string(),
        }),
    }
}

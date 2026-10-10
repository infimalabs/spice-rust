use std::path::PathBuf;

use wire::WireVersion;
use wire::manifest::{JOURNAL_BACKUP_VERSION, JournalBackupManifest};

use super::{BackupVerifyError, MANIFEST};
use crate::SCHEMA_VERSION;

const AUTHORITY_ONLY_BACKUP_VERSION: u16 = 1;

pub(super) fn admit_manifest_generation(
    mut manifest: JournalBackupManifest,
) -> Result<JournalBackupManifest, BackupVerifyError> {
    if manifest.version == AUTHORITY_ONLY_BACKUP_VERSION {
        let authority_only =
            manifest.members.len() == 1 && manifest.members[0].path == manifest.authority;
        if !authority_only {
            return Err(BackupVerifyError::Invalid {
                subject: PathBuf::from(MANIFEST),
                detail: concat!(
                    "backup manifest v1 migration admits exactly one authority member; ",
                    "artifact or sidecar members require canonical v2"
                )
                .to_owned(),
            });
        }
        manifest.version = JOURNAL_BACKUP_VERSION;
    } else if manifest.version != JOURNAL_BACKUP_VERSION {
        return Err(BackupVerifyError::UnsupportedVersion {
            found: manifest.version,
            current: JOURNAL_BACKUP_VERSION,
        });
    }
    if !manifest.wire.is_current() {
        Err(BackupVerifyError::UnsupportedWire {
            found: manifest.wire,
            current: WireVersion::CURRENT,
        })
    } else if manifest.journal_schema > SCHEMA_VERSION {
        Err(BackupVerifyError::ForwardSchema {
            found: manifest.journal_schema,
            current: SCHEMA_VERSION,
        })
    } else if manifest.journal_schema != SCHEMA_VERSION {
        Err(BackupVerifyError::SchemaMismatch {
            manifest: manifest.journal_schema,
            authority: SCHEMA_VERSION,
        })
    } else {
        Ok(manifest)
    }
}

#[cfg(test)]
mod tests {
    use wire::manifest::JournalBackupMember;
    use wire::{Digest, Domain};

    use super::*;

    fn member(path: &str) -> JournalBackupMember {
        JournalBackupMember {
            path: path.to_owned(),
            mode: 0o600,
            bytes: 0,
            digest: Digest::compute(Domain::JournalBackupMember, &[]),
        }
    }

    fn manifest(version: u16, members: Vec<JournalBackupMember>) -> JournalBackupManifest {
        JournalBackupManifest {
            version,
            wire: WireVersion::CURRENT,
            journal_schema: SCHEMA_VERSION,
            authority: "spicejournal.sqlite3".to_owned(),
            members,
        }
    }

    #[test]
    fn authority_only_v1_normalizes_once_and_every_other_old_shape_refuses() {
        let admitted = admit_manifest_generation(manifest(
            AUTHORITY_ONLY_BACKUP_VERSION,
            vec![member("spicejournal.sqlite3")],
        ))
        .expect("authority-only v1 migrates");
        assert_eq!(admitted.version, JOURNAL_BACKUP_VERSION);

        let extra = admit_manifest_generation(manifest(
            AUTHORITY_ONLY_BACKUP_VERSION,
            vec![member("artifact"), member("spicejournal.sqlite3")],
        ));
        assert!(matches!(extra, Err(BackupVerifyError::Invalid { .. })));
        assert!(matches!(
            admit_manifest_generation(manifest(0, vec![member("spicejournal.sqlite3")])),
            Err(BackupVerifyError::UnsupportedVersion { found: 0, current })
                if current == JOURNAL_BACKUP_VERSION
        ));
    }
}

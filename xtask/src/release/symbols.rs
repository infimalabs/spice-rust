use std::fs;
use std::path::Path;

use super::BuildError;

const MAC_SYMBOL_BYTES_LIMIT: u64 = 512 * 1024 * 1024;

pub(super) fn read_symbol_bundle(
    bundle: &Path,
    executable: &Path,
) -> Result<(Vec<u8>, Vec<u8>), BuildError> {
    let name = executable
        .file_name()
        .ok_or_else(|| BuildError::new("linked executable has no filename"))?;
    let contents = bundle.join("Contents");
    let resources = contents.join("Resources");
    let dwarf_root = resources.join("DWARF");
    require_entries(bundle, &["Contents"])?;
    require_entries(&contents, &["Info.plist", "Resources"])?;
    require_entries(&resources, &["DWARF"])?;
    require_entries(&dwarf_root, &[name.to_string_lossy().as_ref()])?;
    let info = read_file(&contents.join("Info.plist"))?;
    let dwarf = read_file(&dwarf_root.join(name))?;
    if (info.len() as u64).saturating_add(dwarf.len() as u64) > MAC_SYMBOL_BYTES_LIMIT {
        return Err(BuildError::new("dSYM bundle exceeds its fixed byte limit"));
    }
    Ok((info, dwarf))
}

fn read_file(path: &Path) -> Result<Vec<u8>, BuildError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| BuildError::new(format!("cannot inspect {}: {error}", path.display())))?;
    if !metadata.file_type().is_file() || metadata.len() > MAC_SYMBOL_BYTES_LIMIT {
        return Err(BuildError::new(format!(
            "dSYM member {} is not a bounded regular file",
            path.display()
        )));
    }
    let bytes = fs::read(path)
        .map_err(|error| BuildError::new(format!("cannot read {}: {error}", path.display())))?;
    let after = fs::symlink_metadata(path)
        .map_err(|error| BuildError::new(format!("cannot inspect {}: {error}", path.display())))?;
    if bytes.len() as u64 != metadata.len()
        || after.len() != metadata.len()
        || after.modified().ok() != metadata.modified().ok()
    {
        return Err(BuildError::new("dSYM member moved while captured"));
    }
    Ok(bytes)
}

fn require_entries(path: &Path, expected: &[&str]) -> Result<(), BuildError> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| BuildError::new(format!("cannot read {}: {error}", path.display())))?
        .map(|entry| {
            entry
                .map_err(|error| BuildError::new(error.to_string()))
                .and_then(|entry| {
                    entry
                        .file_name()
                        .into_string()
                        .map_err(|_| BuildError::new("dSYM path is not UTF-8"))
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort();
    let mut expected = expected
        .iter()
        .map(|item| (*item).to_owned())
        .collect::<Vec<_>>();
    expected.sort();
    if entries == expected {
        Ok(())
    } else {
        Err(BuildError::new(format!(
            "dSYM directory {} contains {entries:?}, expected {expected:?}",
            path.display()
        )))
    }
}

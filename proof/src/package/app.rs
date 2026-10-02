//! Bind ordinary app files to the shell declaration and target payload.

use super::{
    DATA_MODE, Digest, Domain, PACKAGE_DOCUMENT_LIMIT, PackageAppInput, PackageError,
    require_member,
};
pub(super) fn admit_app(
    app: &[PackageAppInput],
    payload: &wire::manifest::TargetPayload,
    index: usize,
) -> Result<(), PackageError> {
    let shell = app
        .iter()
        .find(|asset| asset.file == wire::manifest::CANVAS_APP_HTML)
        .ok_or(PackageError::Binding {
            index,
            field: "app shell",
        })?;
    let html = shell.member.read_bounded(PACKAGE_DOCUMENT_LIMIT)?;
    let renderers =
        wire::manifest::renderer::from_html(&html).map_err(|_| PackageError::Binding {
            index,
            field: "app renderer declaration",
        })?;
    let mut expected = wire::manifest::renderer::filenames(&renderers).collect::<Vec<_>>();
    expected.sort_unstable();
    if app
        .iter()
        .map(|asset| asset.file.as_str())
        .collect::<Vec<_>>()
        != expected
    {
        return Err(PackageError::Binding {
            index,
            field: "app package inventory",
        });
    }
    let mut renderer_bytes = 0_u64;
    for asset in app {
        let path = format!("app/{}", asset.file);
        require_member(&asset.member, &path, DATA_MODE)?;
        asset.member.revalidate()?;
        if let Some(file) = wire::manifest::renderer::File::parse_filename(&asset.file) {
            let member = &renderers[renderers
                .binary_search_by_key(&file, |member| member.file)
                .map_err(|_| PackageError::Binding {
                    index,
                    field: "app renderer declaration",
                })?];
            renderer_bytes = renderer_bytes
                .checked_add(asset.member.length)
                .filter(|total| *total <= wire::manifest::renderer::MAX_BYTES)
                .ok_or(PackageError::Binding {
                    index,
                    field: "app renderer byte bound",
                })?;
            let bytes = asset
                .member
                .read_bounded(wire::manifest::renderer::MAX_BYTES)?;
            if Digest::compute(Domain::InstallArtifact, &bytes) != member.digest {
                return Err(PackageError::Binding {
                    index,
                    field: "app renderer bytes",
                });
            }
        }
        let Some(entry) = payload.inventory.iter().find(|entry| entry.path == path) else {
            return Err(PackageError::Binding {
                index,
                field: "app payload inventory",
            });
        };
        if entry.mode != DATA_MODE
            || entry.bytes != asset.member.length
            || entry.digest != asset.member.digest
        {
            return Err(PackageError::Binding {
                index,
                field: "app asset",
            });
        }
    }
    Ok(())
}

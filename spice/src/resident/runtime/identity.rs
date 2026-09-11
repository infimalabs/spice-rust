//! Product and build identity captured before the Canvas surface binds.

use super::*;

pub(super) fn canvas_version_from_journal(
    app: &canvas::AppSource,
    journal: &journal::Journal,
) -> CommandResult<wire::Version> {
    let schema = journal
        .schema_version()
        .map_err(|error| store_fault("journal::schema-unreadable", &error.to_string()))?;
    canvas_version(app, schema)
}

pub(crate) fn canvas_version(
    app: &canvas::AppSource,
    journal: u32,
) -> CommandResult<wire::Version> {
    let executable = std::env::current_exe().map_err(|error| {
        environment_fault("resident::executable-unavailable", &error.to_string())
    })?;
    let bytes = fs::read(&executable).map_err(|error| {
        environment_fault(
            "resident::executable-unreadable",
            &format!("{}: {error}", executable.display()),
        )
    })?;
    let generation = app.current_generation().map_err(|error| {
        environment_fault("canvas::app-generation-unavailable", &error.to_string())
    })?;
    Ok(wire::Version {
        product: dist::current_product_version()
            .expect("the compiled product version is repository-validated"),
        binary: wire::Digest::compute(wire::Domain::DistArtifact, &bytes),
        wire: wire::WireVersion::CURRENT,
        journal,
        canvas: wire::CanvasVersion::CURRENT,
        app: generation.identity().clone(),
    })
}

use super::*;

#[cfg(debug_assertions)]
pub(in super::super) fn configure_mutation_rendezvous(
    control: Option<OsString>,
    action: Option<OsString>,
    path: Option<OsString>,
) {
    let rendezvous = match (control, action, path) {
        (Some(control), Some(action), Some(path)) => Some(MutationRendezvous {
            control: PathBuf::from(control),
            action,
            path: PathBuf::from(path),
        }),
        _ => None,
    };
    MUTATION_RENDEZVOUS
        .set(rendezvous)
        .expect("install mutation rendezvous is configured once at startup");
}

pub(super) fn temporary_name() -> OsString {
    OsString::from(format!(
        ".spice-install-{}-{}.tmp",
        std::process::id(),
        TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}

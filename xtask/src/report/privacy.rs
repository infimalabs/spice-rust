//! Fail closed before publication if an allowlisted payload leaks private authority.

use super::Result;
use super::publication::Publication;
use super::reduce::Study;

pub(super) fn validate(publication: &Publication, study: &Study) -> Result<()> {
    let encoded = serde_json::to_string(publication)?;
    for birth in study.tasks.keys() {
        if encoded.contains(birth) {
            return Err("publication contains an original task birth".into());
        }
    }
    for task in study.tasks.values() {
        for observation in std::iter::once(&task.admission)
            .chain(&task.phase_ends)
            .chain(task.reviews.iter().map(|review| &review.observation))
        {
            if let Some(actor) = observation
                .actor
                .as_str()
                .and_then(|actor| actor.strip_prefix("lane:"))
                && encoded.contains(actor)
            {
                return Err("publication contains an original lane identity".into());
            }
            if let Some(git) = &observation.git {
                for name in ["head", "integrated", "upstream_head"] {
                    if let Some(id) = git.get(name).and_then(serde_json::Value::as_str)
                        && encoded.contains(id)
                    {
                        return Err("publication contains an original Git identity".into());
                    }
                }
            }
        }
    }
    scan(&encoded)
}

pub(super) fn scan(text: &str) -> Result<()> {
    for forbidden in [
        // Spelled in parts so this detector is not itself a local-path finding.
        concat!("/", "Users/"),
        "/home/",
        "ssh://",
        "serve://",
        "Bearer ",
        "capability://",
        "access_token",
        "secret_key",
        "attachment:",
    ] {
        if text.contains(forbidden) {
            return Err(
                format!("publication contains forbidden private material: {forbidden}").into(),
            );
        }
    }
    for token in text.split(|character: char| {
        !(character.is_ascii_alphanumeric()
            || character == '-'
            || character == ':'
            || character == '.')
    }) {
        if is_uuid(token) || is_object_id(token) || is_precise_instant(token) {
            return Err("publication contains a raw identity or overprecise instant".into());
        }
    }
    Ok(())
}

fn is_uuid(value: &str) -> bool {
    const LENGTH: usize = 36;
    const SEPARATORS: [usize; 4] = [8, 13, 18, 23];
    value.len() == LENGTH
        && value.bytes().enumerate().all(|(index, byte)| {
            if SEPARATORS.contains(&index) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn is_object_id(value: &str) -> bool {
    const SHA1_LENGTH: usize = 40;
    const SHA256_LENGTH: usize = 64;
    matches!(value.len(), SHA1_LENGTH | SHA256_LENGTH)
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_precise_instant(value: &str) -> bool {
    value.contains('T') && value.matches(':').count() >= 2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disclosure_boundary_rejects_identities_credentials_and_exact_instants() {
        for private in [
            "56e9d867-710a-42bc-bb30-a8f627054a79",
            "d1b23313e6ab33d68c681abcb98495ec67d55fb3",
            "2026-09-30T22:43:49.465Z",
            "/home/private/snapshot",
            "Bearer credential",
            "attachment:original",
        ] {
            assert!(
                scan(private).is_err(),
                "private material was admitted: {private}"
            );
        }
        assert!(
            scan("T0001 / L001 / S001 / 2026-09-30 / 2026-09-30 22:00 UTC / 43.25 seconds").is_ok()
        );
    }
}

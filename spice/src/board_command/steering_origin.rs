use super::*;

pub(super) fn task_steering_origin_token(origin: &wire::Origin) -> Option<&str> {
    origin.as_str().strip_prefix("ack:")
}

/// Every steering key that caused non-deleted work, with that work's handles.
///
/// Open reads select only unsettled rows, yet a direction's Work outlives its
/// completion, so the index spans the whole task snapshot.
pub(super) fn steering_origins(
    observed: &bus::TaskSnapshot,
) -> std::collections::BTreeMap<String, Vec<wire::TaskHandle>> {
    let mut origins = std::collections::BTreeMap::<String, Vec<wire::TaskHandle>>::new();
    for heads in observed.tasks() {
        if heads.workflow().body().status == "deleted" {
            continue;
        }
        let statement = heads.statement().body();
        if let Some(key) = task_steering_origin_token(&statement.provenance) {
            origins
                .entry(key.to_owned())
                .or_default()
                .push(TaskHandle::for_project(&statement.project, &statement.born));
        }
    }
    for handles in origins.values_mut() {
        handles.sort();
    }
    origins
}

pub(super) fn task_steering_origin_output(
    bus: &Bus,
    origin: &wire::Origin,
) -> Result<Option<Box<str>>, Outcome<CommandAnswer>> {
    let Some(key) = task_steering_origin_token(origin) else {
        return Ok(None);
    };
    let lookup = key.to_owned();
    let head = bus
        .read(move |read| read.steering_key(&lookup))
        .map_err(|error| {
            store_fault(
                "task::steering-index-unreadable",
                &format!("cannot inspect steering origin {key}: {error}"),
            )
        })?;
    Ok(Some(render_task_steering_origin(
        key,
        head.as_ref().map(journal::JournalHead::body),
    )))
}

pub(super) fn render_task_steering_origin(
    key: &str,
    head: Option<&wire::SteeringHead>,
) -> Box<str> {
    let Some(head) = head else {
        return format!("origin_steering key {key}\norigin_steering unavailable unresolved-key")
            .into_boxed_str();
    };
    let attachments = if head.attachments.is_empty() {
        "-".to_owned()
    } else {
        head.attachments
            .iter()
            .enumerate()
            .map(|(index, attachment)| task_steering_attachment_name(key, index, attachment))
            .collect::<Vec<_>>()
            .join(" ")
    };
    format!(
        "origin_steering key {key}\norigin_steering_text:\n{}\norigin_steering_attachments {attachments}",
        head.text
    )
    .into_boxed_str()
}

pub(super) fn task_steering_attachment_name(
    key: &str,
    index: usize,
    attachment: &wire::body::DirectiveAttachment,
) -> String {
    if attachment.name.is_empty() {
        format!(
            "steering-{key}-attachment-{}.{}",
            index + 1,
            match attachment.content_type.as_str() {
                artifact::MediaType::IMAGE_JPEG => "jpg",
                artifact::MediaType::IMAGE_GIF => "gif",
                artifact::MediaType::IMAGE_WEBP => "webp",
                _ => "png",
            }
        )
    } else {
        attachment.name.clone()
    }
}

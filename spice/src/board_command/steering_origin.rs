use super::*;

pub(super) fn task_steering_origin_token(origin: &wire::Origin) -> Option<&str> {
    origin.as_str().strip_prefix("ack:")
}

/// Every distinct steering key on one task record: the task's own origin
/// first, then each other key in the order its first carrying fact landed.
pub(super) fn task_acknowledgment_keys<'a>(
    origin: &'a wire::Origin,
    acknowledgments: &'a [wire::intent::TaskAcknowledgment],
) -> Vec<&'a str> {
    let mut keys = Vec::new();
    for key in task_steering_origin_token(origin)
        .into_iter()
        .chain(acknowledgments.iter().map(|row| row.key.as_str()))
    {
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
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
    let provenance = bus
        .read(move |read| read_steering_provenance(read, &lookup))
        .map_err(|error| {
            store_fault(
                "task::steering-index-unreadable",
                &format!("cannot inspect steering origin {key}: {error}"),
            )
        })?;
    Ok(Some(render_task_steering_origin(key, provenance.as_ref())))
}

/// One steering key's source directive and the transcript locus that answered it.
pub(super) struct SteeringProvenance {
    presentation: bus::SteeringPresentation,
    answer: SteeringAnswer,
}

enum SteeringAnswer {
    /// A lane acknowledged or declined the directive at a transcript locus.
    Answered {
        disposition: &'static str,
        lane: wire::CanonicalUuid,
        at: wire::Instant,
        /// Executable session context at the answering record, or why none exists.
        context: Result<String, &'static str>,
    },
    /// No transcript answer retired the directive.
    Unanswered(&'static str),
}

/// Read one key's directive, its author, and the session context where a lane
/// answered it, all inside one immutable journal snapshot.
pub(super) fn read_steering_provenance(
    read: &journal::JournalRead<'_>,
    key: &str,
) -> Result<Option<SteeringProvenance>, journal::GuardError> {
    let Some(presentation) = bus::SteeringPresentation::read(read, key)? else {
        return Ok(None);
    };
    let current = presentation.current();
    let answered = current
        .fact
        .read::<wire::body::SteerAcked>()
        .map(|body| ("acked", body.lane, body.locus))
        .or_else(|| {
            current
                .fact
                .read::<wire::body::SteerDeclined>()
                .map(|body| ("declined", body.lane, body.locus))
        });
    let answer = match answered {
        Some((disposition, lane, locus)) => SteeringAnswer::Answered {
            disposition,
            lane,
            at: current.at,
            context: answer_context(read, current.seq, lane, &locus)?,
        },
        None => {
            SteeringAnswer::Unanswered(disposition_word(presentation.head().body().disposition))
        }
    };
    Ok(Some(SteeringProvenance {
        presentation,
        answer,
    }))
}

/// `spice session context` at the answering record, naming the resumable
/// session the lane declared on that exact tape at or before the record.
fn answer_context(
    read: &journal::JournalRead<'_>,
    answered: wire::Seq,
    lane: wire::CanonicalUuid,
    locus: &wire::body::TranscriptLocus,
) -> Result<Result<String, &'static str>, journal::GuardError> {
    let Some(source) = locus.source else {
        return Ok(Err("legacy-locus"));
    };
    let mut before = answered;
    loop {
        let page = read.recent_fact_page(
            wire::FactKind::LaneSessionDeclared,
            Some(before),
            journal::FactReadBudget::RETAINED,
        )?;
        let declared = page.rows.iter().find_map(|row| {
            row.fact
                .read::<wire::body::LaneSessionDeclared>()
                .filter(|declared| {
                    declared.lane == lane
                        && declared.source == Some(source)
                        && (declared.member, declared.offset) <= (locus.member, locus.offset)
                })
        });
        if let Some(declared) = declared {
            return Ok(Ok(shell_words::join([
                "spice".to_owned(),
                "session".to_owned(),
                "context".to_owned(),
                declared.identity,
                "--lane".to_owned(),
                lane.to_string(),
                "--vendor".to_owned(),
                declared.vendor.to_string(),
                "--stream".to_owned(),
                body::TASK_CREATION_CONTEXT_STREAM.to_owned(),
                "--source".to_owned(),
                source.to_string(),
                "--member".to_owned(),
                locus.member.to_string(),
                "--offset".to_owned(),
                locus.offset.to_string(),
            ])));
        }
        let Some(next) = page.continuation else {
            return Ok(Err("session-undeclared"));
        };
        before = next;
    }
}

const fn disposition_word(disposition: wire::Disposition) -> &'static str {
    match disposition {
        wire::Disposition::Live => "live",
        wire::Disposition::Acked => "acked",
        wire::Disposition::Declined => "declined",
        wire::Disposition::Lapsed => "lapsed",
        wire::Disposition::DeadLettered => "deadlettered",
        wire::Disposition::Canceled => "canceled",
    }
}

pub(super) fn render_task_steering_origin(
    key: &str,
    provenance: Option<&SteeringProvenance>,
) -> Box<str> {
    let Some(provenance) = provenance else {
        return format!("origin_steering key {key}\norigin_steering unavailable unresolved-key")
            .into_boxed_str();
    };
    let head = provenance.presentation.head().body();
    let created = provenance.presentation.created();
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
    let answer = match &provenance.answer {
        SteeringAnswer::Answered {
            disposition,
            lane,
            at,
            context,
        } => format!(
            "origin_steering_answer {disposition} lane={lane} at={at}\n{}",
            context.as_ref().map_or_else(
                |reason| format!("origin_steering_context unavailable {reason}"),
                Clone::clone,
            )
        ),
        SteeringAnswer::Unanswered(disposition) => {
            format!("origin_steering_answer unavailable {disposition}")
        }
    };
    format!(
        "origin_steering key {key}\n\
         origin_steering_directive {} lane={} author={} published={}\n\
         origin_steering_text:\n{}\n\
         origin_steering_attachments {attachments}\n\
         {answer}",
        head.directive, head.lane, created.actor, created.at, head.text
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

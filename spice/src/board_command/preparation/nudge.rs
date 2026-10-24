use super::*;
use spice::rooms::nudge::Recipient;
use wire::CanonicalUuid;
use wire::rooms::{
    DesignNudge, DesignNudged, RoomDestination, RoomPosted, RoomPublicationContent, RoomPublished,
};

/// Most bytes of unheard operator posts one nudge message carries.
const CARRIED_POSTS_BYTES: usize = 8 * 1024;

/// Guard reason when the recipient took other work between choice and commit.
const RECIPIENT_BUSY: &str = "preparation::nudge-recipient-busy";

/// Push one open Design to proposal at its witnessed heads.
///
/// The nudge is counted input: it readies a Design's first pass before its
/// maturity steeps, or a later pass once the previous one ended. The same write
/// sends `choose`'s recipient one operator MSG in the Design's room that
/// carries the room's unheard operator posts. `choose` receives the Design and
/// its active holder, which is then the one lane able to take it. Returns the
/// lane to notify.
pub(crate) fn design_nudge(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &DesignNudge,
    config: &steer::Config,
    choose: impl FnOnce(
        &TaskStamp,
        Option<CanonicalUuid>,
    ) -> Result<Option<Recipient>, Outcome<CommandAnswer>>,
) -> (Outcome<CommandAnswer>, Option<CanonicalUuid>) {
    match staged(bus, command, request_body, config, choose) {
        Ok(nudged) => nudged,
        Err(outcome) => (outcome, None),
    }
}

fn staged(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &DesignNudge,
    config: &steer::Config,
    choose: impl FnOnce(
        &TaskStamp,
        Option<CanonicalUuid>,
    ) -> Result<Option<Recipient>, Outcome<CommandAnswer>>,
) -> Result<(Outcome<CommandAnswer>, Option<CanonicalUuid>), Outcome<CommandAnswer>> {
    let design = request_body.design.born().map_err(|error| {
        refusal(
            "preparation::design-invalid",
            Some(request_body.design.to_string()),
            &format!("name a Design handle with a birth stamp: {error}"),
        )
    })?;
    let snapshot = bus
        .task_snapshot()
        .map_err(|error| store_fault("preparation::heads-unreadable", &error.to_string()))?;
    let heads = task_heads(&snapshot, &design)?;
    for witness in [
        Witness::Task {
            task: design.clone(),
            head: heads.statement().position(),
        },
        Witness::Workflow {
            task: design.clone(),
            head: heads.workflow().position(),
            phase: heads.workflow().body().phase.clone(),
        },
    ] {
        require_witness(command, &witness)?;
    }
    let statement = heads.statement().body();
    if !wire::intent::is_open_design(statement.intent, heads.workflow().body()) {
        return Err(refusal(
            "preparation::nudge-not-open-design",
            Some(request_body.design.to_string()),
            "nudge only an open Design; other work is scheduled by its own workflow",
        ));
    }
    let holder =
        heads.claim().and_then(
            |claim| match (&claim.body().disposition, &claim.body().actor) {
                (ClaimDisposition::Active, Actor::Lane(lane)) => Some(*lane),
                _ => None,
            },
        );
    let handle = &request_body.design;
    let Some(recipient) = choose(&design, holder)? else {
        let text = format!(
            "nudged {handle}: no lane could take it now, so it waits ready for allocation: every \
             lane holds other work, none can be woken, or its working group is passive"
        );
        let nudged = DesignNudged {
            design: design.clone(),
            message: None,
            recipient: None,
        };
        return Ok((
            record(
                bus,
                command,
                request_body,
                &design,
                nudged,
                Vec::new(),
                text,
            ),
            None,
        ));
    };
    let carried = unheard_posts(bus, &design)?;
    let message = message(
        handle,
        &statement.title,
        &statement.acceptance,
        &carried,
        recipient,
        holder == Some(recipient.lane),
    );
    let text = messaged_text(&snapshot, handle, recipient);
    let outcome = deliver(
        bus,
        command,
        request_body,
        config,
        &design,
        Delivery {
            recipient,
            message,
            text,
        },
    )?;
    Ok((outcome, Some(recipient.lane)))
}

/// The reply naming the recipient, its match score, whether it wakes, and
/// whether every free lane had already worked the Design.
fn messaged_text(
    snapshot: &bus::TaskSnapshot,
    handle: &wire::TaskHandle,
    recipient: Recipient,
) -> String {
    let name = snapshot
        .residency()
        .and_then(|residency| {
            residency
                .body()
                .lanes
                .iter()
                .find(|row| row.lane == recipient.lane)
                .and_then(|row| row.name.clone())
        })
        .map_or_else(String::new, |name| format!("\"{name}\" "));
    format!(
        "nudged {handle}: messaged {name}(lane {}) at match score {:.3}{}{}",
        recipient.lane,
        recipient.score,
        if recipient.seated {
            ""
        } else {
            "; no seated lane was free, so the message wakes this vacant lane"
        },
        if recipient.last_resort {
            "; every free lane has worked this Design, so this one is a last resort"
        } else {
            ""
        },
    )
}

/// One nudge's addressed message: who receives it, what it says, and the
/// reply text the operator reads once it lands.
struct Delivery {
    recipient: Recipient,
    message: String,
    text: String,
}

/// Commit the nudge with its one operator MSG to the recipient, refusing when
/// the recipient took other work before the write landed.
fn deliver(
    bus: &Bus,
    command: &wire::AdmittedCommand,
    request_body: &DesignNudge,
    config: &steer::Config,
    design: &TaskStamp,
    delivery: Delivery,
) -> Result<Outcome<CommandAnswer>, Outcome<CommandAnswer>> {
    let Delivery {
        recipient,
        message,
        text,
    } = delivery;
    let at = current_instant()?;
    let origin = task_origin(design)?;
    let mut write = JournalWrite::new(
        command.actor().clone(),
        origin.clone(),
        at,
        Producer::CommandDoor,
    );
    guard(&mut write, command.expected().to_vec());
    guard_free(&mut write, recipient.lane, design);
    let (key, _) = crate::resident::append_operator_directive(
        bus,
        origin,
        config,
        &mut write,
        crate::resident::OperatorDirective {
            directive: command.id(),
            fanout: None,
            lane: recipient.lane,
            text: &message,
            control: None,
            room: Some(design.clone()),
        },
        at,
        FactCause::Root,
    )?;
    let minted = key
        .parse::<steer::Key>()
        .ok()
        .and_then(|key| key.minted_micros())
        .ok_or_else(|| store_fault("preparation::nudge-key-invalid", &key))?;
    let micros = jiff::Timestamp::now()
        .as_microsecond()
        .max(minted.saturating_add(1));
    let publication = super::super::room_control::mint_stamp(bus, Some(design), micros)?;
    write
        .fact(
            RoomPosted {
                publication: publication.clone(),
                text: message,
                operation: wire::rooms::RoomPublicationKind::Msg {},
                routes: vec![wire::rooms::RoomRoute {
                    destination: RoomDestination::Lane {
                        lane: recipient.lane,
                    },
                    context: Some(design.clone()),
                    direction: None,
                }],
                directed: vec![command.id()],
                expires: super::super::room_control::expiry(at, config)?,
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("preparation::nudge-build", &error.to_string()))?;
    write
        .fact(
            DesignNudged {
                design: design.clone(),
                message: Some(publication),
                recipient: Some(recipient.lane),
            },
            FactCause::Root,
        )
        .map_err(|error| store_fault("preparation::fact-invalid", &error.to_string()))?;
    crate::resident::steering_envelope_admits(&write)?;
    let answer = TextAnswer { text };
    write
        .command_result(
            request(command, request_body.clone()),
            command.epoch(),
            answer.clone(),
        )
        .map_err(|error| store_fault("preparation::receipt-invalid", &error.to_string()))?;
    match committed_preserving_guard_refusal(
        bus,
        write,
        CommandAnswer::Text(answer),
        RECIPIENT_BUSY,
    ) {
        Outcome::Refusal(refused) if refused.reason() == RECIPIENT_BUSY => Err(refusal(
            RECIPIENT_BUSY,
            Some(recipient.lane.to_string()),
            "the chosen lane took other work before the nudge landed; nudge again to choose from \
             the lanes free now",
        )),
        outcome => Ok(outcome),
    }
}

/// Recheck in the nudge's own transaction that the recipient holds no active
/// claim but the Design's.
fn guard_free(write: &mut JournalWrite, lane: CanonicalUuid, design: &TaskStamp) {
    let actor = Actor::Lane(lane);
    let design = design.clone();
    write.guard(JournalGuard::new(move |read| {
        match read.active_claim_for_actor(&actor)? {
            Some(active) if active.statement().body().born != design => {
                Err(GuardError::refused(RECIPIENT_BUSY))
            }
            _ => Ok(()),
        }
    }));
}

/// The operator's authored room posts no lane has heard: those after the
/// room's latest lane-authored publication, oldest first, within
/// [`CARRIED_POSTS_BYTES`] keeping the newest, and whether older ones did not
/// fit.
fn unheard_posts(
    bus: &Bus,
    room: &TaskStamp,
) -> Result<(Vec<String>, bool), Outcome<CommandAnswer>> {
    let room = room.clone();
    bus.read(move |read| {
        let mut posts = Vec::new();
        let mut used = 0_usize;
        let mut before = None;
        let mut dropped = false;
        'pages: loop {
            let page =
                read.room_publications_newest(&room, before, journal::FactReadBudget::RETAINED)?;
            for envelope in &page.rows {
                if matches!(envelope.actor, Actor::Lane(_)) {
                    break 'pages;
                }
                if !matches!(envelope.actor, Actor::Operator | Actor::Human(_)) {
                    continue;
                }
                let Some(published) = envelope.fact.read::<RoomPublished>() else {
                    continue;
                };
                let RoomPublicationContent::Authored { text } = published.content else {
                    continue;
                };
                used = used.saturating_add(text.len());
                if used > CARRIED_POSTS_BYTES {
                    dropped = true;
                    break 'pages;
                }
                posts.push(text);
            }
            match page.continuation {
                Some(seq) => before = Some(seq),
                None => break,
            }
        }
        posts.reverse();
        Ok((posts, dropped))
    })
    .map_err(|error| store_fault("preparation::room-unreadable", &error.to_string()))
}

/// The one message a nudge sends: complete without the room or the board.
fn message(
    handle: &wire::TaskHandle,
    title: &str,
    acceptance: &[String],
    carried: &(Vec<String>, bool),
    recipient: Recipient,
    held: bool,
) -> String {
    let mut text = format!(
        "The operator pushed Design {handle} \"{title}\" to proposal before its room steeped, and \
         chose {} to take its next pass: this lane's retained work matched it at {:.3}, {}.\n\n\
         Open questions:",
        if recipient.seated { "you" } else { "this lane" },
        recipient.score,
        if recipient.last_resort {
            "and every lane free to take it had already worked a phase of it, so this lane is \
             its last resort"
        } else {
            "the best of the lanes free to take it"
        },
    );
    let questions = spice::rooms::awareness::request_questions(acceptance);
    for (item, question) in questions.iter().enumerate() {
        text.push_str(&format!("\n{}. {question}", item + 1));
    }
    if questions.is_empty() {
        text.push_str("\n- none recorded");
    }
    let (posts, dropped) = carried;
    if !posts.is_empty() {
        text.push_str("\n\nOperator posts in its room no lane had heard, oldest first:");
        if *dropped {
            text.push_str(" (older unheard posts did not fit; read them in its room history)");
        }
        for post in posts {
            text.push_str(&format!("\n- {post}"));
        }
    }
    let start = if held {
        "You already hold it: continue its pass".to_owned()
    } else {
        format!("Run `spice task capture {handle}` to claim it, then take its pass")
    };
    text.push_str(&format!(
        "\n\n{start}: read its room with `spice room history {handle}`, build the \
         acceptance-bearing outputs its open questions call for under the claim, and end the pass \
         with `spice preparation propose JUSTIFICATION`."
    ));
    text
}

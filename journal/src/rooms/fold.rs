use super::*;

/// Canonical key of one proposal subject.
pub(crate) fn subject(proposal: &ProposalRef) -> Result<String, Refusal> {
    String::from_utf8(wire::encode(proposal)?)
        .map_err(|_| Refusal::Unsound("proposal subject is not text".to_owned()))
}

/// Admit one fact `actor` wrote at commit instant `at` against `authority` and
/// apply its projection consequence.
pub(crate) fn fold(
    authority: &mut impl Authority,
    seq: Seq,
    at: Instant,
    actor: &Actor,
    fact: &AnyFact,
) -> Result<(), Refusal> {
    if let Some(body) = fact.read::<wire::body::OopsGenerationAdvanced>() {
        return advance_oops_generation(authority, seq, &body);
    }
    track_task_dependencies(authority, fact);
    for task in fact.revised_tasks() {
        authority.set_task_statement(task, seq);
    }
    if let Some(task) = fact.task_subject() {
        match fact.kind() {
            wire::FactKind::ClaimWon
            | wire::FactKind::ClaimCarried
            | wire::FactKind::ClaimRebound
            | wire::FactKind::ClaimRenewed => {
                authority.set_active_claim(task, Some(seq));
                return Ok(());
            }
            wire::FactKind::ClaimFreed | wire::FactKind::ClaimLapsed => {
                authority.set_active_claim(task, None);
                return Ok(());
            }
            _ => {}
        }
    }
    if let Some(body) = fact.read::<wire::rooms::OutputControlled>() {
        return control(authority, &body.output, body.preparation);
    }
    if let Some((task, preparation)) = fact.task_preparation() {
        authority.set_task(task, preparation.cloned());
        if let Some((_, intent)) = fact.declared_intent() {
            authority.set_task_intent(task, intent);
        }
        return Ok(());
    }
    if let Some((task, intent)) = fact.declared_intent() {
        // Intent is what separates an associated preparation from a gated
        // output, so an associated task keeps the intent it was created with.
        if let Some(Some(preparation)) = authority.task(task)? {
            return unsound(format!(
                "task {task} under controller {} keeps the intent it was created with",
                preparation.controller
            ));
        }
        authority.set_task_intent(task, intent);
        return Ok(());
    }
    if let Some(body) = fact.read::<wire::body::JournalIngested>() {
        authority.account(&TranscriptSourceHead {
            lane: body.lane,
            vendor: body.vendor,
            stream: body.stream,
            source: body.source,
            next: body.to,
        });
        return Ok(());
    }
    if let Some(body) = fact.read::<RoomPublished>() {
        if let RoomPublicationContent::Linked { link } = &body.content {
            require_source(authority, seq, link)?;
        }
        return publish(authority, seq, &body.room, &body.publication);
    }
    if let Some(body) = fact.read::<RoomMessaged>() {
        if *actor != Actor::Lane(body.hunk.head.lane) {
            return unsound(format!(
                "room packet in {} is written by {actor}, not its hunk's lane {}",
                body.room, body.hunk.head.lane
            ));
        }
        require_hunk(authority, &body.hunk)?;
        return publish(authority, seq, &body.room, &body.publication);
    }
    if let Some(result) = retained_note::fold(authority, seq, at, fact) {
        return result;
    }
    if let Some(result) = fold_traffic(authority, seq, at, actor, fact) {
        return result;
    }
    if let Some(moved) = fact.read::<WorkingGroupMoved>() {
        let current = current_partition(authority)?;
        if current.holder(&moved.design).map(|group| &group.group) != moved.from.as_ref()
            || moved
                .partition
                .holder(&moved.design)
                .map(|group| &group.group)
                != moved.to.as_ref()
        {
            return unsound(format!(
                "design {} does not move from {:?} to {:?}",
                moved.design, moved.from, moved.to
            ));
        }
        return store_partition(authority, seq, &current, &moved.partition);
    }
    if let Some(arranged) = fact.read::<WorkingGroupsArranged>() {
        let current = current_partition(authority)?;
        if arranged.from != current {
            return unsound("arrangement does not restate the current partition".to_owned());
        }
        return store_partition(authority, seq, &current, &arranged.partition);
    }
    fold_preparation(authority, seq, actor, fact).unwrap_or(Ok(()))
}

fn advance_oops_generation(
    authority: &mut impl Authority,
    seq: Seq,
    advanced: &wire::body::OopsGenerationAdvanced,
) -> Result<(), Refusal> {
    let current = authority
        .oops_generation()?
        .map(|body| wire::decode::<wire::OopsGenerationHead>(&body))
        .transpose()?
        .unwrap_or_default();
    if current.revision.checked_add(1) != Some(advanced.revision)
        || advanced.from != current.design
        || advanced.from.is_some() == advanced.to.is_some()
    {
        return unsound(format!(
            "OOPS generation transition at {seq} does not advance its exact preceding head"
        ));
    }
    if let Some(design) = &advanced.to {
        match authority.task_intent(design)? {
            Some(Some(wire::TaskIntent::OpportunisticDesign)) => {}
            Some(_) => {
                return unsound(format!(
                    "OOPS generation at {seq} names task {design} without opportunistic Design intent"
                ));
            }
            None => {
                return unsound(format!(
                    "OOPS generation at {seq} names absent task {design}"
                ));
            }
        }
    }
    let head = wire::OopsGenerationHead {
        revision: advanced.revision,
        design: advanced.to.clone(),
    };
    authority.change(Change::OopsGeneration {
        seq: seq.get(),
        body: wire::encode(&head)?,
    })
}

/// Admit one candidate, critique, coverage, decision, withdrawal, or
/// authorization fact, or `None` when `fact` is none of them.
fn fold_preparation(
    authority: &mut impl Authority,
    seq: Seq,
    actor: &Actor,
    fact: &AnyFact,
) -> Option<Result<(), Refusal>> {
    if let Some(DesignProposed { candidate }) = fact.read() {
        return Some(preparation::retain(authority, seq, candidate));
    }
    if let Some(CandidateCritiqued { critique }) = fact.read() {
        return Some(critique_candidate(authority, seq, &critique));
    }
    if let Some(CandidateCovered { coverage }) = fact.read() {
        return Some(cover(authority, seq, actor, coverage));
    }
    if let Some(PreparationDecided { decision }) = fact.read() {
        return Some(
            require_operator(actor, "a preparation decision")
                .and_then(|()| preparation::decide(authority, seq, decision)),
        );
    }
    if let Some(CandidateWithdrawn { withdrawal }) = fact.read() {
        return Some(preparation::withdraw(authority, seq, actor, withdrawal));
    }
    if let Some(ExecutionAuthorized { authorization }) = fact.read() {
        return Some(
            require_operator(actor, "execution authorization")
                .and_then(|()| preparation::authorize(authority, seq, authorization)),
        );
    }
    if fact.kind() == wire::FactKind::DesignNudged {
        return Some(require_operator(actor, "a Design nudge"));
    }
    None
}

/// Admit one critique of an existing candidate citing an admissible source.
fn critique_candidate(
    authority: &mut impl Authority,
    seq: Seq,
    critique: &wire::rooms::CandidateCritique,
) -> Result<(), Refusal> {
    let assessed = ProposalRef::Candidate {
        candidate: critique.critique.candidate.clone(),
    };
    require_proposal(authority, &assessed)?;
    require_source(authority, seq, &critique.source)?;
    let controller = critique.critique.candidate.controller.to_string();
    let proposal = ProposalRef::Critique {
        critique: critique.critique.clone(),
    };
    record(authority, seq, &proposal, controller, critique)
}

/// Admit one coordination, catch-up, conversation, or leave fact, or `None`
/// when `fact` is none of them.
fn fold_traffic(
    authority: &mut impl Authority,
    seq: Seq,
    at: Instant,
    actor: &Actor,
    fact: &AnyFact,
) -> Option<Result<(), Refusal>> {
    if let Some(result) = relevance::fold(authority, seq, actor, fact) {
        return Some(result);
    }
    if let Some(result) = awareness::fold(authority, seq, actor, fact) {
        return Some(result);
    }
    if let Some(result) = coordination::fold(authority, seq, actor, fact) {
        return Some(result);
    }
    if let Some(result) = conversation::fold(authority, seq, at, actor, fact) {
        return Some(result);
    }
    if let Some(body) = fact.read::<RoomDelivered>() {
        return Some(deliver(authority, seq, &body));
    }
    if let Some(body) = fact.read::<RoomLeft>() {
        if *actor != Actor::Lane(body.lane) {
            return Some(unsound(format!(
                "room leave in {} is written by {actor}, not its lane {}",
                body.room, body.lane
            )));
        }
        return Some(
            publish(authority, seq, &body.room, &body.publication)
                .and_then(|()| authority.change(Change::MaturityLeave(body))),
        );
    }
    if let Some(body) = fact.read::<RoomContributed>() {
        return Some(contribute(authority, seq, actor, &body));
    }
    fact.read::<RoomHorizon>()
        .map(|body| begin(authority, seq, actor, &body))
}

struct OpenedPacketRoute {
    directive: CanonicalUuid,
    room: Option<RoomAnchor>,
    from: CanonicalUuid,
    sender: CanonicalUuid,
    locus: wire::body::TranscriptLocus,
}

impl OpenedPacketRoute {
    fn read(fact: &AnyFact) -> Option<Self> {
        if let Some(message) = fact.read::<wire::body::SteerMessaged>() {
            return Some(Self {
                directive: message.directive,
                room: message.room,
                from: message.from,
                sender: message.sender,
                locus: message.locus,
            });
        }
        fact.read::<wire::body::SteerQueued>().map(|message| Self {
            directive: message.directive,
            room: message.room,
            from: message.from,
            sender: message.sender,
            locus: message.locus,
        })
    }
}

/// Each room record of a packet names exactly the directives the same write's
/// peer messages opened into that room, in write order, and each of those
/// messages was sent from the packet's lane by its author at its hunk's tape
/// record. A write publishes at most one packet, so these are its routes.
pub(crate) fn require_packet_routes<'a>(
    facts: impl IntoIterator<Item = &'a AnyFact>,
) -> Result<(), Refusal> {
    let mut messages = Vec::new();
    let mut packets = Vec::new();
    for fact in facts {
        if let Some(message) = OpenedPacketRoute::read(fact) {
            messages.push(message);
        } else if let Some(packet) = fact.read::<RoomMessaged>() {
            packets.push(packet);
        }
    }
    for packet in &packets {
        let opened = messages
            .iter()
            .filter(|message| message.room.as_ref() == Some(&packet.room))
            .collect::<Vec<_>>();
        if !opened
            .iter()
            .map(|message| message.directive)
            .eq(packet.directed.iter().copied())
        {
            return unsound(format!(
                "room packet in {} names directed routes other than those its write opened there",
                packet.room
            ));
        }
        let hunk = &packet.hunk;
        if let Some(message) = opened.into_iter().find(|message| {
            let locus = &message.locus;
            message.from != hunk.head.lane
                || message.sender != packet.author
                || locus.source != Some(hunk.head.source)
                || (locus.member, locus.offset, locus.length, locus.event)
                    != (
                        hunk.locus.member,
                        hunk.locus.offset,
                        hunk.locus.length,
                        hunk.locus.event,
                    )
        }) {
            return unsound(format!(
                "directive {} in room {} was not sent from this packet's lane by its author at its record",
                message.directive, packet.room
            ));
        }
    }
    Ok(())
}

/// Whether `actor` acts as a born principal. The fold replays immutable
/// history, so authorship asks only the actor's principal resolution, never a
/// role a later fact could change; the command door decides current roles.
pub(super) fn is_principal(actor: &Actor) -> bool {
    ActingPrincipal::of(actor).is_some()
}

/// [`is_principal`] for a stored actor spelling.
pub(super) fn acts_as_principal(spelling: &str) -> bool {
    spelling
        .parse::<Actor>()
        .is_ok_and(|actor| is_principal(&actor))
}

pub(super) fn require_operator(actor: &Actor, what: &str) -> Result<(), Refusal> {
    if is_principal(actor) {
        return Ok(());
    }
    unsound(format!("{what} is operator-owned, not written by {actor}"))
}

/// A delivery lands in its own lane's accounted transcript and strictly
/// advances that lane's cursor to an earlier indexed room publication or
/// Design request, the two kinds of item room catch-up carries.
fn deliver(
    authority: &mut impl Authority,
    seq: Seq,
    delivered: &RoomDelivered,
) -> Result<(), Refusal> {
    require_hunk(authority, &delivered.hunk)?;
    let through = delivered.through;
    let indexed =
        authority.publication_at(through.get())? || authority.request_at(through.get())?;
    if through.get() >= seq.get() || !indexed {
        return unsound(format!(
            "delivery cursor {through} names no room publication or Design request earlier than {seq}"
        ));
    }
    let lane = delivered.hunk.head.lane.to_string();
    if let Some(previous) = authority.cursor(&lane)? {
        let previous = wire::decode::<RoomDelivered>(&previous)?.through;
        if through <= previous {
            return unsound(format!(
                "lane {lane} delivery cursor {through} does not advance past {previous}"
            ));
        }
    }
    authority.change(Change::Cursor {
        lane,
        seq: seq.get(),
        body: wire::encode(delivered)?,
    })
}

/// The resident records one horizon for a lane that no delivery has ever
/// advanced; a lane with a cursor or an earlier horizon already has its start.
fn begin(
    authority: &mut impl Authority,
    seq: Seq,
    actor: &Actor,
    horizon: &RoomHorizon,
) -> Result<(), Refusal> {
    if *actor != Actor::Spice {
        return unsound(format!(
            "a room catch-up horizon is resident-owned, not written by {actor}"
        ));
    }
    let lane = horizon.lane.to_string();
    if authority.cursor(&lane)?.is_some() || authority.horizon(&lane)? {
        return unsound(format!("lane {lane} already has a room catch-up start"));
    }
    authority.change(Change::Horizon {
        lane,
        seq: seq.get(),
        body: wire::encode(horizon)?,
    })
}

/// A contribution is its own lane's bounded text, and each source it cites is
/// authority that exists before it. Its Design's openness is checked against
/// stored heads at commit, by [`require_open_controllers`].
fn contribute(
    authority: &mut impl Authority,
    seq: Seq,
    actor: &Actor,
    body: &RoomContributed,
) -> Result<(), Refusal> {
    if *actor != Actor::Lane(body.lane) {
        return unsound(format!(
            "contribution in {} is written by {actor}, not its lane {}",
            body.room, body.lane
        ));
    }
    if let Err(error) = RoomContributed::admit(&body.text, &body.sources) {
        return unsound(format!("contribution in {}: {error}", body.room));
    }
    for source in &body.sources {
        require_source(authority, seq, source)?;
    }
    publish(authority, seq, &body.room, &body.publication)?;
    if let Some(statement) = body.statement {
        let current = authority.task_statement(&body.room)?.ok_or_else(|| {
            Refusal::Unsound(format!(
                "contribution Design {} has no statement",
                body.room
            ))
        })?;
        if current != statement {
            return unsound(format!(
                "contribution in {} names statement {statement} while its current head is {current}",
                body.room
            ));
        }
    }
    authority.change(Change::MaturityContribution(body.clone()))
}

/// Coverage authority is the principal that wrote it: the operator, an
/// independent lane, or the candidate's own authoring lane labeled as
/// self-review.
fn cover(
    authority: &mut impl Authority,
    seq: Seq,
    actor: &Actor,
    coverage: CandidateCoverage,
) -> Result<(), Refusal> {
    let assessed = ProposalRef::Candidate {
        candidate: coverage.coverage.candidate.clone(),
    };
    require_proposal(authority, &assessed)?;
    let author = authority.proposal_author(&subject(&assessed)?)?;
    let admitted = match &coverage.authority {
        CoverageAuthority::Operator {} => is_principal(actor),
        CoverageAuthority::Independent { lane, .. } => {
            *actor == Actor::Lane(*lane) && author.as_ref() != Some(actor)
        }
        CoverageAuthority::SelfReview { lane, .. } => {
            *actor == Actor::Lane(*lane) && author.as_ref() == Some(actor)
        }
    };
    if !admitted {
        return unsound(format!(
            "coverage authority disagrees with its writer {actor} and the candidate's author"
        ));
    }
    require_source(authority, seq, &coverage.source)?;
    let controller = coverage.coverage.candidate.controller.to_string();
    let proposal = ProposalRef::Coverage {
        coverage: coverage.coverage.clone(),
    };
    record(authority, seq, &proposal, controller, &coverage)
}

fn track_task_dependencies(authority: &mut impl Authority, fact: &AnyFact) {
    if let Some(added) = fact.read::<wire::body::TaskAdded>() {
        authority.set_task_birth_dependencies(&added);
        return;
    }
    if let Some(edited) = fact.read::<wire::body::TaskEdited>()
        && let Some(dependencies) = edited.dependencies
    {
        authority.set_task_dependencies(&edited.task, dependencies);
    }
}

fn control(
    authority: &mut impl Authority,
    output: &TaskStamp,
    preparation: TaskPreparation,
) -> Result<(), Refusal> {
    match authority.task(output)? {
        None => return unsound(format!("controlled output {output} does not exist")),
        Some(Some(current)) => {
            return unsound(format!(
                "output {output} is already controlled by {}",
                current.controller
            ));
        }
        Some(None) => {}
    }
    if preparation.parent == *output || preparation.controller == *output {
        return unsound(format!("output {output} cannot prepare or control itself"));
    }
    for task in [
        &preparation.parent,
        &preparation.controller,
        &preparation.construction,
    ] {
        require_task(authority, task, "preparation")?;
    }
    let mut dependencies = authority.task_dependencies(output)?.unwrap_or_default();
    if !dependencies.contains(&preparation.parent) {
        dependencies.push(preparation.parent.clone());
    }
    authority.set_task_dependencies(output, dependencies);
    authority.set_task(output, Some(preparation));
    Ok(())
}

pub(super) fn publish(
    authority: &mut impl Authority,
    seq: Seq,
    room: &TaskStamp,
    publication: &RoomAuthorityStamp,
) -> Result<(), Refusal> {
    require_task(authority, room, "room anchor")?;
    let room = room.to_string();
    let publication = publication.to_string();
    if authority.published(&room, &publication)? {
        return unsound(format!(
            "room {room} publication {publication} already exists"
        ));
    }
    authority.change(Change::Publication {
        room,
        publication,
        seq: seq.get(),
    })
}

pub(super) fn record<T: serde::Serialize>(
    authority: &mut impl Authority,
    seq: Seq,
    proposal: &ProposalRef,
    controller: String,
    body: &T,
) -> Result<(), Refusal> {
    let key = subject(proposal)?;
    if authority.proposal(&key)?.is_some() {
        return unsound(format!("proposal subject {key} is already recorded"));
    }
    authority.change(Change::Proposal {
        subject: key,
        row: ProposalRow {
            controller,
            seq: seq.get(),
            body: wire::encode(body)?,
        },
    })
}

fn current_partition(authority: &impl Authority) -> Result<WorkingGroupPartition, Refusal> {
    Ok(match authority.group_partition()? {
        Some(body) => wire::decode(&body)?,
        None => WorkingGroupPartition::default(),
    })
}

/// Fold one partition change: the next revision, standing groups of existing
/// tasks, none past the design cap.
fn store_partition(
    authority: &mut impl Authority,
    seq: Seq,
    current: &WorkingGroupPartition,
    partition: &WorkingGroupPartition,
) -> Result<(), Refusal> {
    if Some(partition.revision) != current.revision.checked_add(1) {
        return unsound(format!(
            "working-group revision {} does not follow {}",
            partition.revision, current.revision
        ));
    }
    for group in &partition.groups {
        if group.anchors.len() > wire::rooms::WORKING_GROUP_DESIGN_LIMIT {
            return unsound(format!(
                "working group {} holds {} designs, more than {}",
                group.group,
                group.anchors.len(),
                wire::rooms::WORKING_GROUP_DESIGN_LIMIT
            ));
        }
        for anchor in &group.anchors {
            require_task(authority, anchor, "working-group anchor")?;
        }
    }
    authority.change(Change::GroupPartition {
        seq: seq.get(),
        body: wire::encode(partition)?,
    })
}

pub(super) fn require_task(
    authority: &impl Authority,
    task: &TaskStamp,
    role: &str,
) -> Result<(), Refusal> {
    match authority.task(task)? {
        Some(_) => Ok(()),
        None => unsound(format!("{role} task {task} does not exist")),
    }
}

pub(super) fn require_proposal(
    authority: &impl Authority,
    proposal: &ProposalRef,
) -> Result<Vec<u8>, Refusal> {
    let key = subject(proposal)?;
    match authority.proposal(&key)? {
        Some(body) => Ok(body),
        None => unsound(format!("proposal subject {key} does not exist")),
    }
}

/// A link must name authority that exists before the linking fact. Artifact
/// bytes live outside the journal, so their digest is the whole link.
pub(super) fn require_source(
    authority: &impl Authority,
    seq: Seq,
    source: &RoomSource,
) -> Result<(), Refusal> {
    match source {
        RoomSource::Journal { fact } => {
            if fact.get() == 0 || fact.get() >= seq.get() {
                return unsound(format!(
                    "source fact {} is not authority earlier than {seq}",
                    fact.get()
                ));
            }
        }
        RoomSource::Transcript(hunk) => require_hunk(authority, hunk)?,
        RoomSource::Artifact { .. } => {}
        RoomSource::Contribution { room, publication } => {
            if !authority.published(room.as_str(), &publication.to_string())? {
                return unsound(format!(
                    "source contribution {publication} is not published in room {room}"
                ));
            }
        }
    }
    Ok(())
}

/// A hunk must name a nonempty text range of one decoded event inside the
/// accounted extent of its transcript generation.
pub(super) fn require_hunk(
    authority: &impl Authority,
    hunk: &TranscriptHunk,
) -> Result<(), Refusal> {
    let TranscriptHunk { head, locus, text } = hunk;
    let Some(accounted) = authority.transcript(head)? else {
        return unsound(format!(
            "source transcript {} generation {} is not accounted",
            head.key().as_str(),
            head.source
        ));
    };
    if accounted.source != head.source {
        return unsound(format!(
            "source transcript {} generation {} is not accounted",
            head.key().as_str(),
            head.source
        ));
    }
    if !position_at_or_before(head.next, accounted.next) {
        return unsound(format!(
            "source transcript {} boundary {}/{} exceeds accounted boundary {}/{}",
            head.key().as_str(),
            head.next.member,
            head.next.offset,
            accounted.next.member,
            accounted.next.offset,
        ));
    }
    if !locus_within(*locus, head.next) {
        return unsound(format!(
            "source transcript {} locus {}/{}/{} exceeds linked boundary {}/{}",
            head.key().as_str(),
            locus.member,
            locus.offset,
            locus.length,
            head.next.member,
            head.next.offset,
        ));
    }
    if locus.event == u32::MAX || text.start >= text.end {
        return unsound(format!(
            "source transcript {} hunk {}..{} of event {} is not decoded event text",
            head.key().as_str(),
            text.start,
            text.end,
            locus.event,
        ));
    }
    Ok(())
}

fn position_at_or_before(
    position: wire::body::TranscriptPosition,
    boundary: wire::body::TranscriptPosition,
) -> bool {
    position.member < boundary.member
        || (position.member == boundary.member && position.offset <= boundary.offset)
}

fn locus_within(locus: wire::MessageLocus, boundary: wire::body::TranscriptPosition) -> bool {
    if locus.length == 0 {
        return false;
    }
    let Some(end) = locus.offset.checked_add(locus.length) else {
        return false;
    };
    locus.member < boundary.member || (locus.member == boundary.member && end <= boundary.offset)
}

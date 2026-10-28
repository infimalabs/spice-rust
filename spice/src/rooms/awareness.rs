//! The request text every room surface presents for a Design.
//!
//! Matching completed work to open Designs scores with the shared statement
//! engine in [`board::similarity`].

/// Most rendered bytes of whole open questions one Design request carries.
pub const DESIGN_REQUEST_QUESTIONS_BYTES: usize = 2 * 1024;

/// The exact source and authored excerpt of the work that supported one Design
/// request, phrased from the supporting claim's phase or as conversation.
fn supporting_work(request: &wire::intent::DesignRequest) -> String {
    use wire::rooms::DesignNoticeEvidence as Evidence;

    let code = |seq: wire::Seq| wire::SequenceCode::new(seq.get());
    let excerpt = &request.excerpt;
    match &request.evidence {
        Evidence::Task {
            task,
            statement,
            completed,
            support,
        } => format!(
            "you {} task {task} (statement revision {}, completion {}): \"{excerpt}\". Read \
             it with `spice task show {task} --history`.",
            support.verb(),
            code(*statement),
            code(*completed),
        ),
        Evidence::TaskClaim {
            task,
            statement,
            claim,
            terminal,
            support,
        } => format!(
            "you {} task {task} (statement revision {}, claim {claim}{}): \"{excerpt}\". Read \
             it with `spice task show {task} --history`.",
            support.verb(),
            code(*statement),
            terminal.map_or_else(String::new, |terminal| format!(
                ", terminal operation {}",
                code(terminal)
            )),
        ),
        Evidence::Conversation { source } => {
            let locus = &source.locus;
            let Some(tape) = locus.source else {
                return format!(
                    "you explained this in conversation: \"{excerpt}\" (transcript member {} \
                     byte {}).",
                    locus.member, locus.offset
                );
            };
            let command = shell_words::join([
                "spice".to_owned(),
                "session".to_owned(),
                "context".to_owned(),
                request.session.identity.clone(),
                "--lane".to_owned(),
                request.lane.to_string(),
                "--vendor".to_owned(),
                request.session.vendor.to_string(),
                "--stream".to_owned(),
                wire::body::TASK_CREATION_CONTEXT_STREAM.to_owned(),
                "--source".to_owned(),
                tape.to_string(),
                "--member".to_owned(),
                locus.member.to_string(),
                "--offset".to_owned(),
                locus.offset.to_string(),
            ]);
            format!(
                "you explained this in conversation: \"{excerpt}\" (transcript {tape} member {} \
                 byte {} length {}). Read it with `{command}`.",
                locus.member, locus.offset, locus.length
            )
        }
    }
}

/// One Design invitation as every command and catch-up surface presents it.
#[must_use]
pub fn request_text(request: &wire::intent::DesignRequest) -> String {
    use wire::rooms::DesignNoticeOpportunity as Opportunity;

    let design = &request.design;
    let opportunity = match &request.opportunity {
        Opportunity::TaskCompletion { task, .. } => format!("completion of task {task}"),
        Opportunity::TaskClaim {
            task,
            claim,
            terminal,
        } => terminal.map_or_else(
            || format!("retained task {task} claim {claim}"),
            |terminal| format!("terminal operation {terminal} under task {task} claim {claim}"),
        ),
        Opportunity::Conversation { .. } => "a settled conversation message".to_owned(),
        Opportunity::DesignStatement => "this Design statement".to_owned(),
        Opportunity::ProfileReady { .. } => "the completed session profile".to_owned(),
        Opportunity::SessionResumed { .. } => "this resumed session".to_owned(),
    };
    let support = supporting_work(request);
    let cast = match request.reach {
        Some(wire::rooms::DesignReach::Wide) => {
            " Its Design is not yet steeped, so this match was cast slightly wider than usual: \
             a lower relevance floor admitted it."
        }
        _ => "",
    };
    let head = format!(
        "Pitch: {}\nDesign request {design} \"{}\" at statement revision {} matched your work \
         on: {}.{cast} The profile was evaluated at {opportunity}.\nSupporting work: {support}\n",
        request.pitch,
        request.title,
        request.statement,
        request.terms.join(", "),
    );
    if !request.asks.is_empty() {
        return format!("{head}{}", item_request(request));
    }
    let mut questions = String::new();
    for (item, question) in request.questions.iter().enumerate() {
        questions.push_str(&format!("\n{}. {question}", item + 1));
    }
    if questions.is_empty() {
        questions.push_str("\n- none recorded");
    }
    format!(
        "{head}Open questions:{questions}\n\
         Read its room with `spice room history {design}` (`spice://task/{design}/room`). A \
         concrete observation, risk, or counterexample helps, and declining is equally fine: \
         answer exactly once with `spice room contribute {design} --charge=C --question N \
         [--cite KEY...] TEXT`, where C from -2 to +2 weighs open question N, ±1 is an opinion, \
         and 0 or ±2 cites a task, journal code, contribution stamp, or artifact digest; pose a \
         new question with `spice room contribute {design} --ask TEXT`; or decline with `spice \
         room leave {design} --clause TEXT`. Until one answer publishes, `spice task next` \
         returns this request instead of allocating. It grants no claim and starts no work; you \
         hear this room's later posts until that one answer, a contribution or a leave, ends the \
         request and its listening.",
    )
}

/// The body of a request that asks about question items: each item with its
/// standing, the one contribute form, and the request's own supporting work
/// offered as a citation.
fn item_request(request: &wire::intent::DesignRequest) -> String {
    use wire::rooms::DesignNoticeEvidence as Evidence;

    let design = &request.design;
    let mut questions = String::new();
    for ask in &request.asks {
        let lanes = match ask.lanes {
            0 => String::new(),
            lanes => format!(", {lanes} lane{}", plural(lanes as u64)),
        };
        questions.push_str(&format!(
            "\nQuestion {} ({}{lanes}): \"{}\"",
            ask.question,
            ask.state.label(),
            ask.text
        ));
    }
    let recharge = if request
        .asks
        .iter()
        .any(|ask| ask.state == wire::rooms::DesignItemState::Contested)
    {
        "\nBoth signs weigh a contested question and you charged it before: a strength-2 \
         re-charge, citing what your position rests on, would show where it now stands."
    } else {
        ""
    };
    let cite = match &request.evidence {
        Evidence::Task { task, .. } | Evidence::TaskClaim { task, .. } => {
            format!("The work that matched you cites as `--cite {task}`.")
        }
        Evidence::Conversation { .. } => "The conversation that matched you is not citable; \
                                          cite a task, journal code, contribution stamp, or \
                                          artifact digest instead."
            .to_owned(),
    };
    format!(
        "Start from no: this Design and each question must earn its place from evidence. A \
         negative charge is the expected answer when a question has not; charge every question \
         negative when the Design itself should not exist.\n\
         Weigh any one or more of these questions:{questions}{recharge}\n\
         Read its room with `spice room history {design}` (`spice://task/{design}/room`). Weigh \
         question N with `spice room contribute {design} --charge=C --question N [--cite \
         KEY...] TEXT`, where C runs from -2 to +2, ±1 is an opinion, and 0 or ±2 cites what it \
         rests on; or decline with `spice room leave {design} --clause TEXT`. Until one answer \
         publishes, `spice task next` returns this request instead of allocating; one answer \
         settles it. It grants no claim and starts no work; you hear this room's later posts \
         until that answer, a contribution or a leave, ends the request and its listening. {cite}",
    )
}

fn plural(count: u64) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// The longest in-order prefix of whole open questions that fits one request.
#[must_use]
pub fn request_questions(questions: &[String]) -> Vec<String> {
    let mut used: usize = 0;
    questions
        .iter()
        .take_while(|question| {
            let rendered = question.len().saturating_add(3);
            let fits = used.saturating_add(rendered) <= DESIGN_REQUEST_QUESTIONS_BYTES;
            if fits {
                used += rendered;
            }
            fits
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_questions_keep_only_the_whole_in_order_prefix_that_fits() {
        let first = "a".repeat(DESIGN_REQUEST_QUESTIONS_BYTES - 3);
        let questions = [first.clone(), "does not fit".to_owned(), "short".to_owned()];
        assert_eq!(request_questions(&questions), [first]);
    }
}

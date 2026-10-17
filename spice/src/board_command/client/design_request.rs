//! The private Design request `spice task next` answers before allocating.

use super::*;

fn design_request_questions(questions: &[String]) -> String {
    if questions.is_empty() {
        return "- none recorded".to_owned();
    }
    let capacity = questions
        .iter()
        .map(|question| question.len().saturating_add(3))
        .sum();
    let mut rendered = String::with_capacity(capacity);
    for (index, question) in questions.iter().enumerate() {
        if index > 0 {
            rendered.push('\n');
        }
        rendered.push_str("- ");
        rendered.push_str(question);
    }
    rendered
}
fn opportunity(request: &wire::intent::DesignRequest) -> String {
    use wire::rooms::DesignNoticeOpportunity as Opportunity;
    match &request.opportunity {
        Opportunity::TaskCompletion { task, .. } => format!("completion of task {task}"),
        Opportunity::Conversation { .. } => "a settled conversation message".to_owned(),
        Opportunity::DesignStatement => "this Design statement".to_owned(),
        Opportunity::ProfileReady { .. } => "the completed session profile".to_owned(),
        Opportunity::SessionResumed { .. } => "this resumed session".to_owned(),
    }
}

fn evidence(request: &wire::intent::DesignRequest) -> String {
    use wire::rooms::DesignNoticeEvidence as Evidence;
    match &request.evidence {
        Evidence::Task { task, .. } => format!("task {task}"),
        Evidence::Conversation { source } => format!(
            "conversation source {}:{}:{}",
            source.locus.member, source.locus.offset, source.locus.event
        ),
    }
}

pub(super) fn design_request_reply(
    board: &BoardAnswer,
    request: &wire::intent::DesignRequest,
) -> Reply<CommandAnswer> {
    let terms = request.terms.join(", ");
    let questions = design_request_questions(&request.questions);
    let design = &request.design;
    let opportunity = opportunity(request);
    let evidence = evidence(request);
    let text = format!(
        "Design request {design} \"{}\" at statement revision {} matched {evidence} on: \
         {terms}. The profile was evaluated at {opportunity}.\nPitch: {}\nOpen questions:\n\
         {questions}\nRead its room with `spice room history {design}` \
         (`spice://task/{design}/room`). Answer exactly once with \
         `spice room contribute {design} reason-for|reason-against|evidence|question TEXT` or \
         `spice room leave {design} --clause TEXT`. Until either answer publishes, \
         `spice task next` returns this request instead of allocating. It grants no claim and \
         starts no work; you hear this room's later posts until you leave it.",
        request.title, request.statement, request.pitch,
    );
    Reply {
        position: ReplyPosition::Snapshot(board.cursor),
        body: CommandAnswer::Text(TextAnswer { text }),
    }
}

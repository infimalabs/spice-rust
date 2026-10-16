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

pub(super) fn design_request_reply(
    board: &BoardAnswer,
    request: &wire::intent::DesignRequest,
) -> Reply<CommandAnswer> {
    let terms = request.terms.join(", ");
    let questions = design_request_questions(&request.questions);
    let design = &request.design;
    let text = format!(
        "Design request {design} \"{}\" at statement revision {} matches completed task {} on: \
         {terms}.\nPitch: {}\nOpen questions:\n{questions}\nRead its room with \
         `spice room history {design}` (`spice://task/{design}/room`). Answer exactly once with \
         `spice room contribute {design} reason-for|reason-against|evidence|question TEXT` or \
         `spice room leave {design} --clause TEXT`. Until either answer publishes, \
         `spice task next` returns this request instead of allocating. It grants no claim and \
         starts no work; you hear this room's later posts until you leave it.",
        request.title, request.statement, request.task, request.pitch,
    );
    Reply {
        position: ReplyPosition::Snapshot(board.cursor),
        body: CommandAnswer::Text(TextAnswer { text }),
    }
}

//! Learning-specific question and verdict over the shared adjudication process.

use super::LearningCandidate;

const MAX_ATTEMPTS: usize = 2;
const CRITERION: &str = "The candidate is a durable, repo-general operational fact worth carrying to successor agents. It is reusable across future work in this repository and is not task status, speculation, a secret, one-off implementation trivia, or a statement without reusable operational value.";

#[cfg(test)]
pub(super) const DEFAULT_JUDGE: &str = crate::cli::JUDGE_EXECUTABLE;
#[cfg(test)]
pub(super) use crate::judge::{invoke, parse_yes_no};
pub(super) type JudgeError = crate::judge::Error;

pub(super) trait Judge {
    fn judge(&mut self, candidate: &LearningCandidate) -> Result<bool, JudgeError>;
}

#[derive(Debug, Default)]
pub(super) struct ProcessJudge {
    process: crate::judge::Process,
}

impl Judge for ProcessJudge {
    fn judge(&mut self, candidate: &LearningCandidate) -> Result<bool, JudgeError> {
        let prompts = prompts(candidate);
        debug_assert_eq!(prompts.len(), MAX_ATTEMPTS);
        self.process
            .adjudicate(&prompts)
            .map(|decision| decision.confirmed())
    }
}

fn prompts(candidate: &LearningCandidate) -> [crate::judge::Prompt; MAX_ATTEMPTS] {
    let statement = [
        ("statement", candidate.statement.as_str()),
        ("evidence", candidate.evidence.as_str()),
        ("source_task", candidate.source_task.as_str()),
        ("project_stem", candidate.project_stem.as_str()),
    ]
    .into_iter()
    .filter(|(_, value)| !value.is_empty())
    .map(|(name, value)| format!("{name}: {}", crate::judge::compact(value)))
    .collect::<Vec<_>>()
    .join("; ");
    [
        crate::judge::Prompt::new(
            0,
            format!(
                "IFF \"{statement}\" SATISFIES \"{CRITERION}\": ANSWER ONLY \"YES\".\n\
                 IFF \"{statement}\" DOES NOT SATISFY \"{CRITERION}\": ANSWER ONLY \"NO\".\n"
            ),
        ),
        crate::judge::Prompt::new(
            1,
            format!(
                "ANSWER ONLY YES when this repository learning satisfies the criterion; otherwise ANSWER ONLY NO.\n\
                 CRITERION: {CRITERION}\nCANDIDATE: {statement}\n"
            ),
        ),
    ]
}

#[cfg(test)]
pub(super) struct FunctionJudge<F>(pub(super) F);

#[cfg(test)]
impl<F> Judge for FunctionJudge<F>
where
    F: FnMut(&LearningCandidate) -> Result<bool, JudgeError>,
{
    fn judge(&mut self, candidate: &LearningCandidate) -> Result<bool, JudgeError> {
        (self.0)(candidate)
    }
}

#[cfg(test)]
pub(super) fn failure(reason: &'static str, detail: &str) -> JudgeError {
    crate::judge::failure_error(reason, detail)
}

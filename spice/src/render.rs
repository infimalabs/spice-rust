//! Rendering typed command results at the binary rim.
//!
//! A command produces one result. Text and `--json` are two renderings of that
//! one value, not two behaviours: the same result speaks on the same stream
//! and exits with the same code either way, and only the bytes differ. The flag
//! therefore selects a form, never a destination and never a result.
//!
//! Which stream an outcome speaks on is a property of the outcome, not of the
//! form. A reply is the answer a caller pipes, so it goes to standard output;
//! a refusal and a fault are not answers, so they go to standard error, where
//! a text refusal is already documented to appear. If `--json` moved a refusal
//! to standard output it would be a second behaviour rather than a second
//! rendering, and a caller who redirected one stream would receive different
//! bytes depending on a flag that was supposed to change only their shape.
//!
//! Mixed outcome output is unrepresentable rather than merely avoided:
//! rendering yields one [`Rendering`], which owns exactly one stream and
//! exactly one document. A long-running text command may receive an explicit
//! progress sink from the binary rim before that outcome exists; machine mode
//! supplies no such sink, so its eventual document remains singular.

use std::io::{self, Write};

use listen::StreamOutcome;
use serde::Serialize;
use wire::{Outcome, Reply, ReplyPosition};

/// Which rendering of an outcome the caller asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// Ordered terminal text for a person reading a terminal.
    Text,
    /// The typed DTO, for a program.
    Json,
}

/// Which stream an outcome speaks on, whatever form it is rendered in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    /// The command's answer, which a caller pipes.
    Out,
    /// What is not an answer: refusals and faults.
    Err,
}

/// One outcome, rendered: the single stream it speaks on and the single
/// document it speaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendering {
    stream: Stream,
    document: Vec<u8>,
}

impl Rendering {
    #[cfg(test)]
    pub(crate) fn document(&self) -> &[u8] {
        &self.document
    }

    /// Writes this rendering to the stream it names.
    ///
    /// A terminal that has gone away is not this process's business to report:
    /// the outcome already decided the exit code, and a closed pipe would
    /// otherwise turn one outcome into a different one on its way out.
    pub fn speak(&self) {
        let mut written: Box<dyn Write> = match self.stream {
            Stream::Out => Box::new(io::stdout()),
            Stream::Err => Box::new(io::stderr()),
        };
        let _ = written.write_all(&self.document);
        let _ = written.flush();
    }
}

/// Renders one outcome in the requested form.
///
/// Text replies serialize structured bodies as their stable JSON document.
/// Callers whose reply type has an explicit human-text variant use
/// [`render_with_reply_text`] to select that prose without changing `--json`.
/// Refusals keep their typed arm's output order and faults keep their
/// one-sentence form. Long-running transcript wakes use [`render_stream`]; a
/// finite transcript command uses [`render_stream_reply`] so its one result
/// keeps the reply envelope.
///
/// # Errors
///
/// Returns the encoder's report when the selected form cannot represent the
/// outcome or its reply body. It is returned as text because that is the shape
/// the rim carries it in: the detail of a fault. Rendering that fault as text
/// performs no fallible body encoding.
pub fn render<B: Serialize>(outcome: &Outcome<B>, form: Form) -> Result<Rendering, String> {
    render_with_reply_text(outcome, form, no_reply_text::<B>)
}

/// Renders one outcome while projecting an explicitly human-text reply body.
///
/// The projector is consulted only for text replies. JSON always retains the
/// complete typed outcome, and a body without projected prose keeps its stable
/// structured JSON document in text mode.
pub fn render_with_reply_text<B: Serialize>(
    outcome: &Outcome<B>,
    form: Form,
    reply_text: fn(&B) -> Option<&str>,
) -> Result<Rendering, String> {
    let document = match form {
        Form::Text => text(outcome, reply_text)?,
        Form::Json => line(wire::encode(outcome).map_err(|error| error.to_string())?),
    };
    Ok(Rendering {
        stream: stream(outcome),
        document,
    })
}

fn no_reply_text<B>(_body: &B) -> Option<&str> {
    None
}

/// Render the commit gate's evidence-first refusal while keeping admission silent.
///
/// The shared refusal renderer owns both text and JSON grammar. Successful
/// forms return no rendering at all, so a caller cannot accidentally turn an
/// admitted battery back into output.
#[cfg(not(target_arch = "wasm32"))]
pub fn render_commit_gate<B: Serialize>(
    outcome: &Outcome<B>,
    form: Form,
) -> Result<Option<Rendering>, String> {
    match outcome {
        Outcome::Reply(_) => Ok(None),
        Outcome::Refusal(_) | Outcome::Fault(_) => render(outcome, form).map(Some),
    }
}

/// Renders one successful transcript report in the requested form.
pub fn render_stream(outcome: &StreamOutcome, form: Form) -> Result<Rendering, String> {
    let document = match form {
        Form::Text => terminated(outcome.to_string().into_bytes()),
        Form::Json => line(wire::encode(outcome).map_err(|error| error.to_string())?),
    };
    Ok(Rendering {
        stream: Stream::Out,
        document,
    })
}

/// Renders one finite transcript command result in the requested form.
///
/// Its human form remains the stream report. Its machine form is a daemonless
/// reply like every other finite command rather than a bare body.
pub fn render_stream_reply(outcome: &StreamOutcome, form: Form) -> Result<Rendering, String> {
    let document = match form {
        Form::Text => terminated(outcome.to_string().into_bytes()),
        Form::Json => {
            let outcome = Outcome::Reply(Reply {
                position: ReplyPosition::Direct {},
                body: outcome,
            });
            line(wire::encode(&outcome).map_err(|error| error.to_string())?)
        }
    };
    Ok(Rendering {
        stream: Stream::Out,
        document,
    })
}

/// The stream an outcome speaks on.
///
/// This is one of two readings the rim takes of the same value; the exit code
/// is the other. They are stated apart because they answer different questions,
/// and held together by the law that a caller branching on either one is told
/// the same thing.
pub const fn stream<B>(outcome: &Outcome<B>) -> Stream {
    match outcome {
        Outcome::Reply(_) => Stream::Out,
        Outcome::Refusal(_) | Outcome::Fault(_) => Stream::Err,
    }
}

/// One document, terminated once.
fn line(mut document: Vec<u8>) -> Vec<u8> {
    document.push(b'\n');
    document
}

fn terminated(mut document: Vec<u8>) -> Vec<u8> {
    if document.last() != Some(&b'\n') {
        document.push(b'\n');
    }
    document
}

/// The one document an outcome renders to in text form.
///
/// Refusal ordering is selected by its semantic arm. A human-text reply has no
/// JSON wrapper; other successful bodies keep their stable structured document.
fn text<B: Serialize>(
    outcome: &Outcome<B>,
    reply_text: fn(&B) -> Option<&str>,
) -> Result<Vec<u8>, String> {
    let document = match outcome {
        Outcome::Reply(reply) => {
            if let Some(text) = reply_text(&reply.body) {
                return Ok(terminated(text.as_bytes().to_vec()));
            }
            wire::encode(&reply.body).map_err(|error| error.to_string())?
        }
        Outcome::Refusal(refusal) => refusal_text(refusal).into_bytes(),
        Outcome::Fault(fault) => format!("spice: {}; {}", fault.reason, fault.detail).into_bytes(),
    };
    Ok(line(document))
}

fn refusal_text(refusal: &wire::Refusal) -> String {
    match refusal {
        wire::Refusal::Repairable {
            command,
            diagnostic,
        } => {
            let command =
                std::iter::once("spice").chain(command.arguments().iter().map(String::as_str));
            format!(
                "{}\n{}",
                shell_words::join(command),
                diagnostic_text(diagnostic)
            )
        }
        wire::Refusal::Bare { diagnostic } => diagnostic_text(diagnostic),
        wire::Refusal::Invocation { contract, .. } => contract.clone(),
        wire::Refusal::Evidence {
            reason,
            evidence,
            summary,
        } => format!(
            "{}\nspice: {reason}; {summary}",
            evidence.trim_end_matches('\n')
        ),
    }
}

fn diagnostic_text(diagnostic: &wire::RefusalDiagnostic) -> String {
    diagnostic.detail().map_or_else(
        || format!("spice: {}", diagnostic.reason()),
        |detail| format!("spice: {}; {detail}", diagnostic.reason()),
    )
}

#[cfg(test)]
mod tests {
    use serde::Serialize;
    use wire::{
        Fault, FaultClass, Outcome, Refusal, RefusalDiagnostic, Reply, ReplyPosition, SpiceCommand,
    };

    use super::{Form, Stream, render, render_commit_gate, render_with_reply_text};

    fn reply() -> Outcome<()> {
        Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body: (),
        })
    }

    #[derive(Serialize)]
    struct HumanTextBody {
        text: String,
    }

    fn human_text(body: &HumanTextBody) -> Option<&str> {
        Some(&body.text)
    }

    fn repairable() -> Outcome<()> {
        Outcome::Refusal(Refusal::repairable(
            SpiceCommand::new("task", ["show", "CLAIM with space"])
                .expect("fixture command is valid"),
            RefusalDiagnostic::new(
                "claim::not-held",
                Some("claim".to_owned()),
                Some("one current claim".to_owned()),
                Some("absent".to_owned()),
            )
            .with_detail("the task is not held"),
        ))
    }

    fn bare() -> Outcome<()> {
        Outcome::Refusal(Refusal::bare(
            RefusalDiagnostic::new(
                "claim::not-held",
                Some("claim".to_owned()),
                Some("one current claim".to_owned()),
                Some("absent".to_owned()),
            )
            .with_detail("the task is not held"),
        ))
    }

    fn invocation() -> Outcome<()> {
        Outcome::Refusal(Refusal::invocation(
            "gate::prove-action-unregistered",
            "spice gate prove ACTION requires one registered gate action",
        ))
    }

    fn evidence() -> Outcome<()> {
        Outcome::Refusal(Refusal::evidence(
            "gate::regressed",
            "file-shape: regressed\nrust: held\n",
            "repair the reported regression",
        ))
    }

    fn fault() -> Outcome<()> {
        Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "environment::unavailable".to_owned(),
            detail: "the resident socket is not there".to_owned(),
        })
    }

    #[test]
    fn the_commit_gate_is_silent_on_success_in_both_forms() {
        for form in [Form::Text, Form::Json] {
            assert_eq!(
                render_commit_gate(&reply(), form).expect("commit gate reply is representable"),
                None,
            );
        }
    }

    #[test]
    fn the_commit_gate_leads_with_its_board_and_keeps_json_typed() {
        let outcome = evidence();
        let text = render_commit_gate(&outcome, Form::Text)
            .expect("commit gate refusal renders")
            .expect("a refusal is loud");
        assert_eq!(text.stream, Stream::Err);
        assert_eq!(
            text.document,
            b"file-shape: regressed\nrust: held\nspice: gate::regressed; repair the reported regression\n"
        );

        let json = render_commit_gate(&outcome, Form::Json)
            .expect("commit gate refusal encodes")
            .expect("a refusal is loud");
        assert_eq!(json.stream, Stream::Err);
        let decoded: Outcome<()> =
            wire::decode(&json.document).expect("JSON commit refusal stays typed");
        assert_eq!(decoded, outcome);
    }

    /// Every outcome renders as the DTO it is.
    ///
    /// Valid JSON is the weaker claim. A caller reads the document to learn
    /// which outcome it holds and what it said, so the rendering has to
    /// survive the round trip through the same decoder every other consumer
    /// uses, not merely parse.
    #[test]
    fn every_outcome_renders_as_the_dto_it_is() {
        for outcome in [
            reply(),
            repairable(),
            bare(),
            invocation(),
            evidence(),
            fault(),
        ] {
            let rendered = render(&outcome, Form::Json).expect("outcome encodes");
            let decoded: Outcome<()> =
                wire::decode(&rendered.document).expect("rendering decodes as the wire DTO");

            assert_eq!(decoded, outcome);
        }
    }

    #[test]
    fn an_explicit_human_text_body_drops_only_its_default_json_wrapper() {
        let outcome = Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body: HumanTextBody {
                text: "first line\nsecond line\n".to_owned(),
            },
        });

        let text = render_with_reply_text(&outcome, Form::Text, human_text)
            .expect("human text reply renders");
        assert_eq!(text.stream, Stream::Out);
        assert_eq!(text.document, b"first line\nsecond line\n");
        assert_eq!(
            render(&outcome, Form::Text)
                .expect("structured text reply renders")
                .document,
            b"{\"text\":\"first line\\nsecond line\\n\"}\n",
        );
        assert_eq!(
            render_with_reply_text(&outcome, Form::Json, human_text)
                .expect("human reply JSON renders")
                .document,
            render(&outcome, Form::Json)
                .expect("ordinary reply JSON renders")
                .document,
            "--json is independent of the human text projection",
        );
    }

    #[test]
    fn a_daemonless_reply_keeps_its_type_without_an_empty_position_marker() {
        let outcome = reply();

        let text = render(&outcome, Form::Text).expect("direct reply renders as text");
        assert_eq!(text.stream, Stream::Out);
        assert_eq!(text.document, b"null\n", "text carries the body only");

        let json = render(&outcome, Form::Json).expect("direct reply renders as json");
        let decoded: Outcome<()> =
            wire::decode(&json.document).expect("json rendering decodes as its typed outcome");
        assert_eq!(decoded, outcome);
        let value: serde_json::Value =
            serde_json::from_slice(&json.document).expect("wire outcome is JSON");
        assert!(value.get("direct").is_none());
        assert!(value.get("cursor").is_none());
        assert!(value.get("seq").is_none());
    }

    /// The form selects bytes and never a stream.
    ///
    /// This is the whole of what `--json` is allowed to change. A form that
    /// moved a refusal to standard output would be a second behaviour, and a
    /// caller reading one stream would find the outcome missing entirely
    /// rather than differently shaped.
    #[test]
    fn the_form_selects_bytes_and_never_a_stream() {
        for (outcome, expected) in [
            (reply(), Stream::Out),
            (bare(), Stream::Err),
            (fault(), Stream::Err),
        ] {
            for form in [Form::Text, Form::Json] {
                assert_eq!(
                    render(&outcome, form).expect("outcome renders").stream,
                    expected,
                    "{outcome:?} in {form:?} must keep its stream",
                );
            }
        }
    }

    /// A rendering is one document with exactly one terminal newline.
    ///
    /// Two documents on one stream is the shape a reader cannot parse, and an
    /// unterminated one is the shape a line reader waits forever on.
    #[test]
    fn a_rendering_is_one_terminated_document() {
        for outcome in [
            reply(),
            repairable(),
            bare(),
            invocation(),
            evidence(),
            fault(),
        ] {
            for form in [Form::Text, Form::Json] {
                let rendered = render(&outcome, form).expect("outcome renders");
                assert_eq!(
                    rendered.document.last(),
                    Some(&b'\n'),
                    "{outcome:?} in {form:?} terminates",
                );
                assert!(
                    !rendered.document.ends_with(b"\n\n"),
                    "{outcome:?} in {form:?} has one terminal newline",
                );
            }
        }
    }

    /// Each refusal arm owns one exact output order.
    #[test]
    fn refusal_forms_render_their_contractual_order() {
        assert_eq!(
            render(&repairable(), Form::Text)
                .expect("repairable refusal renders")
                .document,
            b"spice task show 'CLAIM with space'\nspice: claim::not-held; the task is not held\n",
        );
        assert_eq!(
            render(&bare(), Form::Text)
                .expect("bare refusal renders")
                .document,
            b"spice: claim::not-held; the task is not held\n",
        );
        assert_eq!(
            render(&invocation(), Form::Text)
                .expect("invocation refusal renders")
                .document,
            b"spice gate prove ACTION requires one registered gate action\n",
        );
        assert_eq!(
            render(&evidence(), Form::Text)
                .expect("evidence refusal renders")
                .document,
            b"file-shape: regressed\nrust: held\nspice: gate::regressed; repair the reported regression\n",
        );
        assert_eq!(
            render(&fault(), Form::Text)
                .expect("fault renders")
                .document,
            b"spice: environment::unavailable; the resident socket is not there\n",
        );
    }

    /// A success has no envelope sentence; text carries only its stable body.
    #[test]
    fn a_successful_outcome_has_no_envelope_sentence() {
        assert_eq!(
            render(&reply(), Form::Text)
                .expect("reply renders")
                .document,
            b"null\n",
        );
    }
}

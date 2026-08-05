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
//! Mixed output is unrepresentable rather than merely avoided: rendering
//! yields one [`Rendering`], which owns exactly one stream and exactly one
//! document. There is no second stream for a stray sentence to reach and no
//! second document to append to the first.

use std::io::{self, Write};

use listen::StreamOutcome;
use serde::Serialize;
use wire::Outcome;

/// Which rendering of an outcome the caller asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    /// One sentence for a person reading a terminal.
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
    pub(crate) const fn stream(&self) -> Stream {
        self.stream
    }

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
/// Text replies are the stable machine document of the body alone; JSON
/// replies retain the complete typed outcome. Refusals and faults keep their
/// one-sentence text form. Both forms therefore carry the same value without
/// inventing prose fields for a successful body.
/// Transcript streams use [`render_stream`] because each successful wake is
/// the stream document itself rather than a reply envelope.
///
/// # Errors
///
/// Returns the encoder's report when the selected form cannot represent the
/// outcome or its reply body. It is returned as text because that is the shape
/// the rim carries it in: the detail of a fault. Rendering that fault as text
/// performs no fallible body encoding.
pub fn render<B: Serialize>(outcome: &Outcome<B>, form: Form) -> Result<Rendering, String> {
    let document = match form {
        Form::Text => text(outcome)?,
        Form::Json => line(wire::encode(outcome).map_err(|error| error.to_string())?),
    };
    Ok(Rendering {
        stream: stream(outcome),
        document,
    })
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
/// A refusal and a fault are each one sentence naming what happened and what
/// to do about it. A reply has no envelope sentence: what a successful command
/// prints is the body's stable wire document.
fn text<B: Serialize>(outcome: &Outcome<B>) -> Result<Vec<u8>, String> {
    let document = match outcome {
        Outcome::Reply(reply) => wire::encode(&reply.body).map_err(|error| error.to_string())?,
        Outcome::Refusal(refusal) => {
            format!("spice: {}; {}", refusal.reason, refusal.remedy).into_bytes()
        }
        Outcome::Fault(fault) => format!("spice: {}; {}", fault.reason, fault.detail).into_bytes(),
    };
    Ok(line(document))
}

#[cfg(test)]
mod tests {
    use spice::placeholder::PlaceholderReason;
    use wire::{Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition};

    use super::{Form, Stream, render};

    fn reply() -> Outcome<()> {
        Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body: (),
        })
    }

    fn refusal() -> Outcome<()> {
        Outcome::Refusal(Refusal {
            reason: PlaceholderReason::NotLanded.to_reason(),
            witness: None,
            expected: None,
            observed: Some("task next".to_owned()),
            remedy: "finish the battery".to_owned(),
        })
    }

    fn fault() -> Outcome<()> {
        Outcome::Fault(Fault {
            class: FaultClass::Environment,
            reason: "environment::unavailable".to_owned(),
            detail: "the resident socket is not there".to_owned(),
        })
    }

    /// Every outcome renders as the DTO it is.
    ///
    /// Valid JSON is the weaker claim. A caller reads the document to learn
    /// which outcome it holds and what it said, so the rendering has to
    /// survive the round trip through the same decoder every other consumer
    /// uses, not merely parse.
    #[test]
    fn every_outcome_renders_as_the_dto_it_is() {
        for outcome in [reply(), refusal(), fault()] {
            let rendered = render(&outcome, Form::Json).expect("outcome encodes");
            let decoded: Outcome<()> =
                wire::decode(&rendered.document).expect("rendering decodes as the wire DTO");

            assert_eq!(decoded, outcome);
        }
    }

    #[test]
    fn a_direct_reply_has_the_same_typed_position_in_both_renderings() {
        let outcome = reply();

        let text = render(&outcome, Form::Text).expect("direct reply renders as text");
        assert_eq!(text.stream, Stream::Out);
        assert_eq!(text.document, b"null\n", "text carries the body only");

        let json = render(&outcome, Form::Json).expect("direct reply renders as json");
        let decoded: Outcome<()> =
            wire::decode(&json.document).expect("json rendering decodes as its typed outcome");
        assert_eq!(decoded, outcome);
        assert!(
            String::from_utf8(json.document)
                .expect("wire JSON is utf8")
                .contains("\"direct\":{}"),
            "the JSON rendering states that the journal position is directly absent",
        );
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
            (refusal(), Stream::Err),
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

    /// A rendering is one document, terminated once.
    ///
    /// Two documents on one stream is the shape a reader cannot parse, and an
    /// unterminated one is the shape a line reader waits forever on.
    #[test]
    fn a_rendering_is_one_terminated_document() {
        for outcome in [reply(), refusal(), fault()] {
            for form in [Form::Text, Form::Json] {
                let rendered = render(&outcome, form).expect("outcome renders");
                assert_eq!(
                    rendered.document.last(),
                    Some(&b'\n'),
                    "{outcome:?} in {form:?} terminates",
                );
                assert_eq!(
                    rendered
                        .document
                        .iter()
                        .filter(|byte| **byte == b'\n')
                        .count(),
                    1,
                    "{outcome:?} in {form:?} is one line",
                );
            }
        }
    }

    /// A declined request renders the sentence CLI fixes, and a fault renders
    /// the same shape.
    #[test]
    fn a_declined_request_renders_the_documented_sentence() {
        assert_eq!(
            render(&refusal(), Form::Text)
                .expect("refusal renders")
                .document,
            format!(
                "spice: {}; finish the battery\n",
                PlaceholderReason::NotLanded.as_str()
            )
            .into_bytes(),
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

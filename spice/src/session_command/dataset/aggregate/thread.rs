//! Thread identity reports with optional whole-transcript metadata.

use wire::Refusal;

use super::{Aggregate, ResolvedSource, source_view, turn_view};
use crate::session_command::model::{Report, Residue, ThreadView};

impl Aggregate {
    pub(crate) fn thread(self, id: String) -> Report {
        let source = self
            .sources
            .last()
            .cloned()
            .expect("one resolved thread yields at least one source");
        let thread = Box::new(ThreadView {
            id,
            vendor: source.vendor,
            transcript: source.path.clone(),
            transcripts: self
                .sources
                .iter()
                .map(|source| source.path.clone())
                .collect(),
            turns: Some(self.turns),
            compactions: Some(self.compactions),
            start: self.start,
            end: self.end,
            latest_turn: self
                .latest
                .as_ref()
                .map(|turn| turn_view(turn, Some((false, 180)))),
        });
        Report::Thread {
            source,
            sources: self.sources,
            thread,
            residues: self.residues,
        }
    }

    /// Preserve a successfully resolved thread path when optional whole-tape
    /// metadata cannot be folded. Bounded history commands can still read the
    /// path, so metadata failure must not erase the resolver's answer.
    pub(crate) fn thread_without_summary(
        resolved: &[ResolvedSource],
        id: String,
        error: &Refusal,
    ) -> Report {
        let sources = resolved
            .iter()
            .map(|source| source_view(source, source.vendor))
            .collect::<Vec<_>>();
        let source = sources
            .last()
            .cloned()
            .expect("one resolved thread yields at least one source");
        let thread = Box::new(ThreadView {
            id,
            vendor: source.vendor,
            transcript: source.path.clone(),
            transcripts: sources.iter().map(|source| source.path.clone()).collect(),
            turns: None,
            compactions: None,
            start: None,
            end: None,
            latest_turn: None,
        });
        Report::Thread {
            residues: vec![Residue {
                source: source.path.clone(),
                line: 0,
                kind: "thread-summary-unavailable".to_owned(),
                detail: error.reason().to_owned(),
                recovery: "use the resolved transcript path with bounded session queries"
                    .to_owned(),
            }],
            source,
            sources,
            thread,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::session_command::model::Vendor;
    use crate::session_command::source::ResolvedSource;

    #[test]
    fn thread_metadata_failure_preserves_the_resolved_transcript() {
        let path = PathBuf::from("/fixture/session.jsonl");
        let resolved = [ResolvedSource {
            path: path.clone(),
            vendor: Vendor::declared(&spice::extension::OMP),
            dialect: Some(listen::Dialect::Omp),
            thread: Some("01234567-89ab-cdef-0123-456789abcdef".to_owned()),
        }];
        let error = crate::session_command::dataset::refusal(
            "session::aggregate-open-turn-limit",
            Some("limit=64".to_owned()),
            "narrow the immutable transcript inputs",
        );

        let report = Aggregate::thread_without_summary(
            &resolved,
            "01234567-89ab-cdef-0123-456789abcdef".to_owned(),
            &error,
        );
        let Report::Thread {
            source,
            thread,
            residues,
            ..
        } = report
        else {
            panic!("resolved thread produces a thread report");
        };
        assert_eq!(source.path, path.display().to_string());
        assert_eq!(thread.transcript, path.display().to_string());
        assert_eq!(thread.turns, None);
        assert_eq!(thread.compactions, None);
        assert_eq!(residues.len(), 1);
        assert_eq!(residues[0].kind, "thread-summary-unavailable");
        assert_eq!(residues[0].detail, error.reason());
    }
}

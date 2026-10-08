//! Source-scoped correlation between typed image reads and later Markdown.

use std::collections::BTreeMap;
use std::path::Path;

use pulldown_cmark::{Event as MarkdownEvent, LinkType, Options, Parser, Tag};
use serde_json::Value;
use wire::CanonicalUuid;

use super::AgentEvent;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Scope {
    lane: CanonicalUuid,
    occupant: CanonicalUuid,
    source: CanonicalUuid,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum Association {
    Known(crate::session_media::ProviderMediaIdentity),
    Ambiguous,
}

/// Metadata-only image-read state. Provider bytes are reacquired for each
/// projected assistant message rather than retained here.
#[derive(Debug, Default)]
pub(super) struct MediaHydration {
    active: BTreeMap<CanonicalUuid, (CanonicalUuid, CanonicalUuid)>,
    calls: BTreeMap<(Scope, String), String>,
    paths: BTreeMap<(Scope, String), Association>,
}

impl MediaHydration {
    pub(super) fn observe(
        &mut self,
        event: &AgentEvent<'_>,
        transcript: Option<&Path>,
    ) -> Vec<wire::MarkdownImageBinding> {
        let Some(source) = event.source else {
            return Vec::new();
        };
        let scope = Scope {
            lane: event.lane,
            occupant: event.author,
            source,
        };
        self.enter(scope);
        match event.decoded {
            listen::Event::Tool(tool) => {
                self.observe_tool(scope, tool);
                Vec::new()
            }
            listen::Event::Prose(prose) | listen::Event::StreamedProse(prose)
                if prose.role == listen::ProseRole::Assistant =>
            {
                transcript.map_or_else(Vec::new, |transcript| {
                    self.bindings(scope, &prose.text, transcript)
                })
            }
            _ => Vec::new(),
        }
    }

    pub(super) fn clear_lane(&mut self, lane: CanonicalUuid) {
        self.active.remove(&lane);
        self.calls.retain(|(scope, _), _| scope.lane != lane);
        self.paths.retain(|(scope, _), _| scope.lane != lane);
    }

    fn enter(&mut self, scope: Scope) {
        let current = (scope.occupant, scope.source);
        if self
            .active
            .get(&scope.lane)
            .is_some_and(|known| *known != current)
        {
            self.clear_lane(scope.lane);
        }
        self.active.insert(scope.lane, current);
    }

    fn observe_tool(&mut self, scope: Scope, tool: &listen::Tool) {
        let key = (scope, tool.call_id.clone());
        match &tool.moment {
            listen::ToolMoment::Call {
                name, arguments, ..
            } => {
                if let Some(path) = read_path(name, arguments) {
                    self.calls.insert(key, path);
                } else {
                    self.calls.remove(&key);
                }
            }
            listen::ToolMoment::Update { .. } => {}
            listen::ToolMoment::Result(outcome) => {
                let Some(path) = self.calls.remove(&key) else {
                    return;
                };
                if outcome.result != listen::ToolResult::Succeeded || outcome.media.len() != 1 {
                    return;
                }
                let identity = match outcome.external_media.as_slice() {
                    [] => {
                        crate::session_media::ProviderMediaIdentity::from_media(&outcome.media[0])
                            .ok()
                    }
                    [external] => {
                        crate::session_media::ProviderMediaIdentity::from_external(external)
                    }
                    _ => None,
                };
                let Some(identity) = identity else {
                    return;
                };
                let association = self
                    .paths
                    .entry((scope, path))
                    .or_insert_with(|| Association::Known(identity.clone()));
                if !matches!(association, Association::Known(known) if known == &identity) {
                    *association = Association::Ambiguous;
                }
            }
        }
    }

    fn bindings(
        &self,
        scope: Scope,
        markdown: &str,
        transcript: &Path,
    ) -> Vec<wire::MarkdownImageBinding> {
        let mut bindings = Vec::new();
        for destination in image_destinations(markdown) {
            if bindings
                .iter()
                .any(|binding: &wire::MarkdownImageBinding| binding.destination == destination)
            {
                continue;
            }
            let Some(Association::Known(identity)) = self.paths.get(&(scope, destination.clone()))
            else {
                continue;
            };
            let Ok(media) = identity.reacquire(transcript) else {
                continue;
            };
            bindings.push(wire::MarkdownImageBinding { destination, media });
            if bindings.len() == wire::media::MAX_MEDIA_ITEMS {
                break;
            }
        }
        if wire::media::validate_markdown_image_bindings(&bindings).is_ok() {
            bindings
        } else {
            Vec::new()
        }
    }
}

fn read_path(name: &str, arguments: &str) -> Option<String> {
    if name != "read" {
        return None;
    }
    let Value::Object(fields) = serde_json::from_str::<Value>(arguments).ok()? else {
        return None;
    };
    let path = fields.get("path")?.as_str()?;
    if path.is_empty()
        || path.len() > wire::media::MAX_MEDIA_DESTINATION_BYTES
        || path.chars().any(char::is_control)
        || !Path::new(path).is_absolute()
    {
        return None;
    }
    Some(path.to_owned())
}

fn image_destinations(markdown: &str) -> Vec<String> {
    Parser::new_ext(markdown, Options::all())
        .filter_map(|event| match event {
            MarkdownEvent::Start(Tag::Image {
                link_type: LinkType::Inline,
                dest_url,
                ..
            }) => Some(dest_url.into_string()),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::Digest as _;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: std::path::PathBuf,
        transcript: std::path::PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "spice-media-hydration-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            let transcript = root.join(".omp/agent/sessions/team").join("thread.jsonl");
            fs::create_dir_all(transcript.parent().expect("transcript parent"))
                .expect("session fixture creates");
            fs::create_dir_all(root.join(".omp/agent/blobs")).expect("blob fixture creates");
            fs::write(&transcript, b"").expect("transcript fixture writes");
            Self { root, transcript }
        }

        fn media(&self, suffix: u8) -> wire::Media {
            let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
            bytes.push(suffix);
            let media = wire::Media::from_bytes("capture.png", "image/png", &bytes)
                .expect("fixture image admits");
            let digest = format!("{:x}", sha2::Sha256::digest(&bytes));
            fs::write(self.root.join(".omp/agent/blobs").join(digest), bytes)
                .expect("provider blob writes");
            media
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.root).expect("media fixture cleanup");
        }
    }

    fn uuid(suffix: &str) -> CanonicalUuid {
        format!("018f82d4-6f59-7d06-92e3-bce9131163{suffix}")
            .parse()
            .expect("fixture identity")
    }

    fn scope(lane: &str, occupant: &str, source: &str) -> Scope {
        Scope {
            lane: uuid(lane),
            occupant: uuid(occupant),
            source: uuid(source),
        }
    }

    fn call(id: &str, path: &str) -> listen::Tool {
        listen::Tool {
            call_id: id.to_owned(),
            moment: listen::ToolMoment::Call {
                name: "read".to_owned(),
                arguments: serde_json::json!({"i": "Reading image", "path": path}).to_string(),
                intent: Some("Reading image".to_owned()),
            },
            locus: listen::Locus::new(1, 0, 1),
        }
    }

    fn result(id: &str, media: wire::Media) -> listen::Tool {
        listen::Tool {
            call_id: id.to_owned(),
            moment: listen::ToolMoment::Result(listen::ToolOutcome {
                result: listen::ToolResult::Succeeded,
                output: Some("Read image file [image/png]".to_owned()),
                exit_code: None,
                media: vec![media],
                external_media: Vec::new(),
            }),
            locus: listen::Locus::new(1, 1, 1),
        }
    }

    fn correlate(
        hydration: &mut MediaHydration,
        scope: Scope,
        id: &str,
        path: &str,
        media: wire::Media,
    ) {
        hydration.enter(scope);
        hydration.observe_tool(scope, &call(id, path));
        hydration.observe_tool(scope, &result(id, media));
    }

    #[test]
    fn one_source_bound_read_hydrates_only_an_exact_image_destination() {
        let fixture = Fixture::new();
        let media = fixture.media(1);
        let scope = scope("e5", "ef", "f0");
        let mut hydration = MediaHydration::default();
        correlate(
            &mut hydration,
            scope,
            "call-image",
            "/tmp/capture.png",
            media.clone(),
        );

        let bindings = hydration.bindings(
            scope,
            "![authored alt](/tmp/capture.png \"Authored title\")",
            &fixture.transcript,
        );
        assert_eq!(
            bindings,
            [wire::MarkdownImageBinding {
                destination: "/tmp/capture.png".to_owned(),
                media,
            }]
        );
        assert!(
            hydration
                .bindings(
                    scope,
                    "[ordinary link](/tmp/capture.png)",
                    &fixture.transcript
                )
                .is_empty()
        );
    }

    #[test]
    fn only_successful_single_image_absolute_reads_correlate() {
        let fixture = Fixture::new();
        let first = fixture.media(1);
        let second = fixture.media(2);
        let scope = scope("e5", "ef", "f0");
        let mut hydration = MediaHydration::default();
        hydration.enter(scope);

        hydration.observe_tool(scope, &call("web", "https://example.com/image.png"));
        hydration.observe_tool(scope, &result("web", first.clone()));
        assert!(
            hydration
                .bindings(
                    scope,
                    "![](https://example.com/image.png)",
                    &fixture.transcript,
                )
                .is_empty()
        );

        hydration.observe_tool(scope, &call("relative", "image.png"));
        hydration.observe_tool(scope, &result("relative", first.clone()));
        assert!(
            hydration
                .bindings(scope, "![](image.png)", &fixture.transcript)
                .is_empty()
        );

        hydration.observe_tool(scope, &call("failed", "/tmp/failed.png"));
        let mut failed = result("failed", first.clone());
        let listen::ToolMoment::Result(outcome) = &mut failed.moment else {
            unreachable!("fixture result")
        };
        outcome.result = listen::ToolResult::Failed;
        hydration.observe_tool(scope, &failed);
        assert!(
            hydration
                .bindings(scope, "![](/tmp/failed.png)", &fixture.transcript)
                .is_empty()
        );

        hydration.observe_tool(scope, &call("multiple", "/tmp/multiple.png"));
        let mut multiple = result("multiple", first);
        let listen::ToolMoment::Result(outcome) = &mut multiple.moment else {
            unreachable!("fixture result")
        };
        outcome.media.push(second);
        hydration.observe_tool(scope, &multiple);
        assert!(
            hydration
                .bindings(scope, "![](/tmp/multiple.png)", &fixture.transcript)
                .is_empty()
        );
    }

    #[test]
    fn scopes_isolate_paths_and_conflicting_content_stays_ambiguous() {
        let fixture = Fixture::new();
        let first = fixture.media(1);
        let second = fixture.media(2);
        let own = scope("e5", "ef", "f0");
        let other_lane = scope("e6", "ef", "f0");
        let mut hydration = MediaHydration::default();
        correlate(
            &mut hydration,
            own,
            "first",
            "/tmp/capture.png",
            first.clone(),
        );
        assert!(
            hydration
                .bindings(other_lane, "![](/tmp/capture.png)", &fixture.transcript,)
                .is_empty()
        );

        correlate(&mut hydration, own, "second", "/tmp/capture.png", second);
        assert!(
            hydration
                .bindings(own, "![](/tmp/capture.png)", &fixture.transcript)
                .is_empty()
        );

        let successor = scope("e5", "ee", "f1");
        hydration.enter(successor);
        assert!(
            hydration
                .bindings(own, "![](/tmp/capture.png)", &fixture.transcript)
                .is_empty()
        );
    }
}

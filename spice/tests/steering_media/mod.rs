use super::*;

const PNG_DATA_URL: &str = "data:image/png;base64,iVBORw0KGgo=";
const SECOND_PNG_DATA_URL: &str = "data:image/png;base64,iVBORw0KGgpB";

#[test]
fn claude_and_codex_hooks_open_private_live_media_projections() {
    let (fixture, resident, lanes) = setup();
    let first = send_media(
        &fixture,
        lanes,
        uuid("00000000-0000-4000-8000-000000000071"),
        uuid("00000000-0000-4000-8000-000000000072"),
        "inspect the Codex capture",
        "codex.png",
        PNG_DATA_URL,
    );
    let target = lanes.target.to_string();
    let codex = invoke_as_lane(&fixture, &target, &["hook", "codex"], false);
    let codex_path = projected_path(&codex, &fixture, PNG_DATA_URL);

    let other = invoke_as_lane(
        &fixture,
        &lanes.other.to_string(),
        &["hook", "codex"],
        false,
    );
    assert!(
        other.status.success(),
        "other hook failed: {:?}",
        other.stderr
    );
    assert!(other.stdout.is_empty(), "other lane received target media");

    let second = send_media(
        &fixture,
        lanes,
        uuid("00000000-0000-4000-8000-000000000073"),
        uuid("00000000-0000-4000-8000-000000000074"),
        "inspect the Claude capture",
        "claude.png",
        SECOND_PNG_DATA_URL,
    );
    let claude = invoke_as_lane(&fixture, &target, &["hook", "claude"], false);
    let claude_path = projected_path(&claude, &fixture, SECOND_PNG_DATA_URL);

    for directive in [first.directive, second.directive] {
        let canceled = text_directive(&invoke(
            &fixture,
            &["steer", "cancel", &directive.to_string()],
            false,
        ));
        assert_eq!(canceled.disposition, wire::Disposition::Canceled);
        let head = journal::Journal::open(fixture.journal())
            .expect("media journal opens")
            .read(|read| read.steering_head(directive))
            .expect("media head reads")
            .expect("media head remains authoritative");
        assert_eq!(head.body().attachments.len(), 1);
    }
    let cleanup = invoke_as_lane(&fixture, &target, &["hook", "codex"], false);
    assert!(
        cleanup.status.success(),
        "cleanup hook failed: {:?}",
        cleanup.stderr
    );
    assert!(cleanup.stdout.is_empty());
    assert!(!codex_path.exists());
    assert!(!claude_path.exists());
    stop(&fixture, resident);
}

fn send_media(
    fixture: &Fixture,
    lanes: Lanes,
    directive: wire::CanonicalUuid,
    fanout: wire::CanonicalUuid,
    text: &str,
    name: &str,
    data_url: &str,
) -> wire::intent::DirectiveAnswer {
    let (mut socket, discovery) = operator_socket(fixture);
    socket_send(
        &mut socket,
        &wire::Command {
            v: wire::WireVersion::CURRENT,
            id: directive,
            epoch: discovery.epoch,
            kind: wire::CommandKind::SteerSend,
            expected: vec![wire::Witness::Residency {
                lane: lanes.target,
                occupant: current_occupant(fixture, lanes.target),
                epoch: discovery.epoch,
            }],
            body: wire::intent::SteerSend {
                fanout,
                lane: lanes.target,
                message: text.to_owned(),
                attachments: vec![
                    wire::Media::admit(name, "image/png", data_url)
                        .expect("process media fixture admits"),
                ],
            },
        },
    );
    let response: wire::Response<wire::intent::DirectiveAnswer> = socket_receive(&mut socket);
    assert_eq!((response.id, response.epoch), (directive, discovery.epoch));
    let outcome = response.outcome;
    let Outcome::Reply(reply) = outcome else {
        panic!("operator media steering did not reply: {outcome:?}")
    };
    reply.body
}

fn projected_path(output: &Output, fixture: &Fixture, source: &str) -> PathBuf {
    assert!(output.status.success(), "hook failed: {:?}", output.stderr);
    assert!(output.stderr.is_empty());
    let document: serde_json::Value = wire::decode(&output.stdout).expect("hook document decodes");
    let context = document["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("hook context is text");
    assert!(!context.contains("data:image"));
    assert!(!context.contains("iVBORw0KG"));
    let projection = context
        .lines()
        .find_map(|line| line.strip_prefix("media-projection="))
        .expect("hook names a media projection");
    let projection: serde_json::Value =
        serde_json::from_str(projection).expect("projection metadata decodes");
    let path = PathBuf::from(projection["path"].as_str().expect("projection path"));
    let root = fs::canonicalize(fixture.runtime.join("steering-media"))
        .expect("projection root canonicalizes");
    assert!(path.starts_with(root));
    let expected = wire::Media::admit("fixture.png", "image/png", source)
        .expect("expected media admits")
        .decoded_bytes()
        .expect("expected media decodes");
    assert_eq!(fs::read(&path).expect("agent opens projection"), expected);
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    path
}

use super::{
    AppRoute, CreatorContextTarget, RouteEvidence, normalize_app_location, opening_auth_token,
    team_stream_card_target, team_stream_card_team,
};

const CARD: &str = "message-agent-01234567-89ab-cdef-0123-456789abcdef-4-8-2-1";

#[test]
fn canonical_durable_routes_round_trip_exactly() {
    for (path, route) in [
        ("/chat", AppRoute::chat()),
        (
            "/chat/card/message-agent-01234567-89ab-cdef-0123-456789abcdef-4-8-2-1",
            AppRoute::card(CARD.to_owned()).expect("fixture card"),
        ),
    ] {
        assert_eq!(AppRoute::from_path(path), Some(route.clone()), "{path}");
        assert_eq!(route.path(), path);
    }
}

#[test]
fn team_stream_card_targets_name_one_canonical_team() {
    let team = "01234567-89ab-cdef-0123-456789abcdef"
        .parse()
        .expect("fixture team");
    let target = "team-stream-01234567-89ab-cdef-0123-456789abcdef";
    assert_eq!(team_stream_card_target(team), target);
    assert_eq!(team_stream_card_team(target), Some(team));
    for malformed in [
        "team-stream-",
        "team-stream-not-a-team",
        "team-stream-01234567-89ab-cdef-0123-456789abcdef-extra",
        "other-01234567-89ab-cdef-0123-456789abcdef",
    ] {
        assert_eq!(team_stream_card_team(malformed), None, "{malformed}");
    }
}

#[test]
fn only_resident_token_fragments_become_opening_credentials() {
    assert_eq!(
        opening_auth_token("#remote-token-0123456789"),
        Some("remote-token-0123456789")
    );
    for hash in [
        "",
        "#dash",
        "#/chat",
        "#/work",
        "#/chat/card/message-agent-01234567",
        "#short",
        "#remote/token/0123456789",
        &format!("#{}", "a".repeat(257)),
    ] {
        assert_eq!(opening_auth_token(hash), None, "{hash}");
    }
}

#[test]
fn retired_or_malformed_locations_are_not_routes() {
    for path in [
        "/work",
        "/work/task/dTv7zpzTN",
        "/work/public/canvas",
        "/work/team/01234567-89ab-cdef-0123-456789abcdef",
        "/work/directions",
        "/work/direction/dTvP0GpW6",
        "/card/message-agent",
        "/chat/card/",
        "/chat/card/message/agent",
        "/chat/card/message_agent",
        "/chat/message-agent",
        "/dash/card/message-agent",
    ] {
        assert_eq!(AppRoute::from_path(path), None, "{path}");
    }
}

#[test]
fn app_locations_canonicalize_chat_spellings_without_a_notice() {
    for (hash, replace) in [
        ("#/chat", false),
        ("", true),
        ("#", true),
        ("#/", true),
        ("#chat", true),
        ("#//chat/", true),
    ] {
        let location = normalize_app_location("", hash);
        assert_eq!(location.route(), &AppRoute::chat(), "{hash}");
        assert_eq!(location.needs_replace(), replace, "{hash}");
        assert_eq!(location.notice(), None, "{hash}");
    }

    let hash = "#/chat/card/message-agent-01234567-89ab-cdef-0123-456789abcdef-4-8-2-1";
    let location = normalize_app_location("", hash);
    assert_eq!(format!("#{}", location.route().path()), hash);
    assert!(!location.needs_replace());
    assert_eq!(location.notice(), None);
}

#[test]
fn unknown_app_locations_open_chat_and_name_the_refused_input() {
    for (search, hash) in [
        ("?page=work", ""),
        ("", "#/unknown"),
        ("", "#/work?scope=all"),
        ("", "#/card/message-agent"),
        ("", "#/work/public/a%2f"),
        ("", "#/work/team/not-a-team"),
        ("", "#/work/teams/01234567-89ab-cdef-0123-456789abcdef"),
        ("", "#/dash"),
        ("", "#dash"),
        ("", "#//dash/"),
    ] {
        let location = normalize_app_location(search, hash);
        assert_eq!(location.route(), &AppRoute::chat());
        assert!(location.needs_replace());
        let notice = location.notice().expect("invalid input is named");
        assert!(notice.contains(&format!("{search}{hash}")), "{notice}");
    }
}

#[test]
fn settled_missing_chat_card_rewrites_to_chat() {
    let card = AppRoute::card(CARD.to_owned()).expect("fixture card");
    assert_eq!(card.unavailable(RouteEvidence::default()), None);
    let (next, notice) = card
        .unavailable(RouteEvidence { card: Some(false) })
        .expect("missing card refuses");
    assert_eq!(next, AppRoute::chat());
    assert!(notice.contains(CARD), "{notice}");
}

#[test]
fn creator_context_targets_round_trip_the_exact_source_boundary() {
    let lane = "01234567-89ab-cdef-0123-456789abcdef"
        .parse()
        .expect("fixture lane");
    let source = "fedcba98-7654-3210-fedc-ba9876543210"
        .parse()
        .expect("fixture source");
    let context = wire::body::TaskCreationContext::Transcript {
        session: "session".to_owned(),
        boundary: wire::TranscriptSourceHead {
            lane,
            vendor: "omp".parse().expect("fixture vendor"),
            stream: wire::body::TASK_CREATION_CONTEXT_STREAM.to_owned(),
            source,
            next: wire::body::TranscriptPosition {
                member: 17,
                offset: 4096,
            },
        },
        creator_team: None,
    };
    let expected = concat!(
        "creator-context-01234567-89ab-cdef-0123-456789abcdef-",
        "fedcba98-7654-3210-fedc-ba9876543210-17-4096",
    );
    let target = CreatorContextTarget::from_creation(&context).expect("transcript context links");

    assert_eq!(target.target(), expected);
    assert_eq!(CreatorContextTarget::parse(expected), Some(target.clone()));
    assert!(AppRoute::card(expected.to_owned()).is_some());
    let locus = |member, offset| wire::MessageLocus {
        member,
        offset,
        length: 1,
        event: 0,
    };
    assert!(target.owns_source_parts(lane, Some(source)));
    assert!(!target.owns_source_parts(lane, None));
    assert!(
        !target.owns_source_parts(
            lane,
            Some(
                "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"
                    .parse()
                    .expect("other fixture source"),
            ),
        )
    );
    assert!(target.contains_locus(lane, locus(17, 4095)));
    assert!(target.contains_locus(lane, locus(16, u64::MAX)));
    assert!(!target.contains_locus(lane, locus(17, 4096)));
    assert!(!target.contains_locus(lane, locus(18, 0)));
    assert!(
        CreatorContextTarget::from_creation(&wire::body::TaskCreationContext::unavailable(
            wire::body::TaskCreationUnavailable::OperatorActor,
        ),)
        .is_none()
    );
}

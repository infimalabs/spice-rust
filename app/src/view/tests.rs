use super::{
    AppDestination, AppNotice, AppRoute, ConnectionStage, CreatorContextTarget, DirectionRef,
    DirectionsFocus, RouteEvidence, RouteStep, app_notice, direction_quote_source,
    normalize_app_location, opening_auth_token, quote_source_href, team_stream_card_target,
    team_stream_card_team,
};
use crate::work::{WorkScope, WorkStemIdentity, WorkVisibility};

const CARD: &str = "message-agent-01234567-89ab-cdef-0123-456789abcdef-4-8-2-1";

fn stem(visibility: WorkVisibility, stem: &str) -> WorkScope {
    WorkScope::Stem(WorkStemIdentity::new(visibility, stem.to_owned()))
}

fn task(stamp: &str) -> wire::TaskStamp {
    stamp.parse().expect("fixture task stamp")
}

fn work(scope: WorkScope, task: Option<wire::TaskStamp>) -> AppRoute {
    AppRoute::Work { scope, task }
}

#[test]
fn canonical_routes_round_trip_exactly() {
    for (path, route) in [
        ("/dash", AppRoute::Dash),
        ("/work", work(WorkScope::AllOpen, None)),
        (
            "/work/task/dTv7zpzTN",
            work(WorkScope::AllOpen, Some(task("dTv7zpzTN"))),
        ),
        (
            "/work/public/canvas",
            work(stem(WorkVisibility::Public, "canvas"), None),
        ),
        (
            "/work/hidden/oops/task/dTv7zpzT",
            work(stem(WorkVisibility::Hidden, "oops"), Some(task("dTv7zpzT"))),
        ),
        (
            "/work/public/a%20b%2F%C3%A9.-_~",
            work(stem(WorkVisibility::Public, "a b/é.-_~"), None),
        ),
        (
            "/work/team/01234567-89ab-cdef-0123-456789abcdef",
            work(
                WorkScope::Team(uuid("01234567-89ab-cdef-0123-456789abcdef")),
                None,
            ),
        ),
        (
            "/work/team/01234567-89ab-cdef-0123-456789abcdef/task/dTv7zpzTN",
            work(
                WorkScope::Team(uuid("01234567-89ab-cdef-0123-456789abcdef")),
                Some(task("dTv7zpzTN")),
            ),
        ),
        ("/chat", AppRoute::Chat { card: None }),
        (
            "/chat/card/message-agent-01234567-89ab-cdef-0123-456789abcdef-4-8-2-1",
            AppRoute::card(CARD.to_owned()).expect("fixture card"),
        ),
        ("/work/directions", AppRoute::Directions { focus: None }),
        (
            "/work/directions/team/01234567-89ab-cdef-0123-456789abcdef",
            directions(DirectionsFocus::Team(uuid(
                "01234567-89ab-cdef-0123-456789abcdef",
            ))),
        ),
        (
            "/work/direction/fedcba98-7654-3210-fedc-ba9876543210",
            directions(DirectionsFocus::Direction(DirectionRef::Identity(uuid(
                "fedcba98-7654-3210-fedc-ba9876543210",
            )))),
        ),
        (
            "/work/direction/dTvP0GpW6",
            directions(DirectionsFocus::Direction(DirectionRef::Key(
                "dTvP0GpW6".to_owned(),
            ))),
        ),
    ] {
        assert_eq!(AppRoute::from_path(path), Some(route.clone()), "{path}");
        assert_eq!(route.path(), path);
    }
    let card = AppRoute::from_path(&format!("/chat/card/{CARD}")).expect("card route");
    assert_eq!(card.app_destination(), AppDestination::Chat);
    assert_eq!(card.card_target(), Some(CARD));
}

fn uuid(text: &str) -> wire::CanonicalUuid {
    text.parse().expect("fixture identity")
}

fn directions(focus: DirectionsFocus) -> AppRoute {
    AppRoute::Directions { focus: Some(focus) }
}

#[test]
fn directions_routes_refine_all_open_work_and_refuse_to_the_section() {
    let direction = directions(DirectionsFocus::Direction(DirectionRef::Key(
        "dTvP0GpW6".to_owned(),
    )));
    assert_eq!(direction.app_destination(), AppDestination::Work);
    assert_eq!(direction.work_scope(), Some(&WorkScope::AllOpen));
    assert_eq!(direction.task(), None);
    let section = AppRoute::Directions { focus: None };
    assert_eq!(section.directions(), Some(None));
    assert_eq!(work(WorkScope::AllOpen, None).directions(), None);
    let pending = RouteEvidence::default();
    assert_eq!(
        direction.unavailable(pending),
        None,
        "unsettled evidence waits"
    );
    let refused = RouteEvidence {
        direction: Some(false),
        ..RouteEvidence::default()
    };
    let (next, notice) = direction
        .unavailable(refused)
        .expect("an unknown direction is refused");
    assert_eq!(next, section);
    assert!(notice.contains("dTvP0GpW6"), "{notice}");
    let team = directions(DirectionsFocus::Team(uuid(
        "01234567-89ab-cdef-0123-456789abcdef",
    )));
    let (next, notice) = team
        .unavailable(refused)
        .expect("an unknown team is refused");
    assert_eq!(next, section);
    assert!(
        notice.contains("01234567-89ab-cdef-0123-456789abcdef"),
        "{notice}"
    );
    assert_eq!(
        section.unavailable(refused),
        None,
        "the section itself names nothing"
    );
    assert_eq!(
        direction.show_work(WorkScope::AllOpen),
        Some(RouteStep::Push(work(WorkScope::AllOpen, None)))
    );
}

#[test]
fn malformed_direction_segments_open_directions_and_name_the_segment() {
    for (hash, segment) in [
        (
            "#/work/direction/FEDCBA98-7654-3210-FEDC-BA9876543210",
            "FEDCBA98-7654-3210-FEDC-BA9876543210",
        ),
        ("#/work/direction/not_a_key", "not_a_key"),
        ("#/work/direction/short", "short"),
        ("#/work/directions/team/not-a-team", "not-a-team"),
    ] {
        let location = normalize_app_location("", hash);
        assert_eq!(
            location.route(),
            &AppRoute::Directions { focus: None },
            "{hash}"
        );
        assert!(location.needs_replace(), "{hash}");
        let notice = location.notice().expect("the refused segment is named");
        assert!(notice.contains(segment), "{notice}");
    }
}

#[test]
fn direction_quote_sources_link_the_canonical_global_direction_route() {
    let identity = uuid("fedcba98-7654-3210-fedc-ba9876543210");
    for direction in [
        DirectionRef::Identity(identity),
        DirectionRef::Key("dTvP0GpW6".to_owned()),
    ] {
        for actor in [
            wire::Actor::Spice,
            wire::Actor::Lane(uuid("01234567-89ab-cdef-0123-456789abcdef")),
        ] {
            assert_eq!(direction_quote_source(&actor, &direction), None, "{actor}");
        }
        let href = quote_source_href(
            &direction_quote_source(&wire::Actor::Operator, &direction)
                .expect("operator directions have global history"),
        );
        let location = normalize_app_location("", &href);
        assert_eq!(
            location.route(),
            &directions(DirectionsFocus::Direction(direction)),
            "{href}"
        );
        assert!(!location.needs_replace(), "{href}");
    }
    assert_eq!(quote_source_href(CARD), format!("#/chat/card/{CARD}"));
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
fn stem_visibility_is_part_of_route_identity() {
    let public = AppRoute::from_path("/work/public/oops").expect("public stem");
    let hidden = AppRoute::from_path("/work/hidden/oops").expect("hidden stem");
    assert_ne!(public, hidden);
    assert_eq!(
        public.work_scope(),
        Some(&stem(WorkVisibility::Public, "oops"))
    );
    assert_eq!(
        hidden.work_scope(),
        Some(&stem(WorkVisibility::Hidden, "oops"))
    );
}

#[test]
fn malformed_or_noncanonical_segments_are_not_routes() {
    for path in [
        "/card/message-agent",
        "/chat/card/",
        "/chat/card/message/agent",
        "/chat/card/message_agent",
        "/chat/message-agent",
        "/dash/card/message-agent",
        "/work/public",
        "/work/public/",
        "/work/private/canvas",
        "/work/public/a%2",
        "/work/public/a%zz",
        "/work/public/a%2f",
        "/work/public/%61",
        "/work/public/a b",
        "/work/public/%FF",
        "/work/task",
        "/work/task/dTv7zpzTNx",
        "/work/task/aeiouaeio",
        "/work/public/canvas/task/dTv7zpzTN/",
        "/work/public/canvas/card/dTv7zpzTN",
        "/direction/01234567-89ab-cdef-0123-456789abcdef/fedcba98-7654-3210-fedc-ba9876543210",
        "/chat/direction/not-a-team/not-a-direction",
        "/chat/direction/01234567-89ab-cdef-0123-456789abcdef",
        "/work/directions/",
        "/work/direction",
        "/work/direction/",
        "/work/direction/FEDCBA98-7654-3210-FEDC-BA9876543210",
        "/work/direction/fedcba9876543210fedcba9876543210",
        "/work/direction/dTvP0GpW6/",
        "/work/direction/dTv-0GpW6",
        "/work/directions/team",
        "/work/directions/team/dTvP0GpW6",
        "/work/directions/fedcba98-7654-3210-fedc-ba9876543210",
        "/work/public/canvas/directions",
    ] {
        assert_eq!(AppRoute::from_path(path), None, "{path}");
    }
}

#[test]
fn app_locations_canonicalize_destination_spellings_without_changing_routes() {
    for (hash, destination, replace) in [
        ("#/chat", AppDestination::Chat, false),
        ("#/dash", AppDestination::Dash, false),
        ("#/work", AppDestination::Work, false),
        ("", AppDestination::Chat, true),
        ("#", AppDestination::Chat, true),
        ("#/", AppDestination::Chat, true),
        ("#dash", AppDestination::Dash, true),
        ("#/work/", AppDestination::Work, true),
        ("#//dash/", AppDestination::Dash, true),
    ] {
        let location = normalize_app_location("", hash);
        assert_eq!(location.route().app_destination(), destination, "{hash}");
        assert_eq!(location.needs_replace(), replace, "{hash}");
        assert_eq!(location.notice(), None, "{hash}");
    }

    for hash in [
        "#/chat/card/message-agent-01234567-89ab-cdef-0123-456789abcdef-4-8-2-1",
        "#/work/hidden/oops/task/dTv7zpzTN",
    ] {
        let location = normalize_app_location("", hash);
        assert_eq!(format!("#{}", location.route().path()), hash);
        assert!(!location.needs_replace(), "{hash}");
        assert_eq!(location.notice(), None, "{hash}");
    }
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
    ] {
        let location = normalize_app_location(search, hash);
        assert_eq!(location.route(), &AppRoute::Chat { card: None });
        assert!(location.needs_replace());
        let notice = location.notice().expect("invalid input is named");
        assert!(notice.contains(&format!("{search}{hash}")), "{notice}");
    }
}

#[test]
fn history_steps_push_openings_and_replace_changes_inside_one_view() {
    let canvas = stem(WorkVisibility::Public, "canvas");
    let team = WorkScope::Team(uuid("01234567-89ab-cdef-0123-456789abcdef"));
    let all = work(WorkScope::AllOpen, None);
    let open = work(canvas.clone(), Some(task("dTv7zpzTN")));
    let chat = AppRoute::Chat { card: None };

    assert_eq!(chat.show(AppDestination::Chat), None);
    assert_eq!(
        open.show(AppDestination::Work),
        Some(RouteStep::Push(all.clone()))
    );
    assert_eq!(
        AppRoute::card(CARD.to_owned())
            .expect("fixture card")
            .show(AppDestination::Chat),
        Some(RouteStep::Push(chat.clone()))
    );

    assert_eq!(
        chat.show_work(canvas.clone()),
        Some(RouteStep::Push(work(canvas.clone(), None)))
    );
    assert_eq!(
        all.show_work(canvas.clone()),
        Some(RouteStep::Replace(work(canvas.clone(), None)))
    );
    assert_eq!(
        open.show_work(WorkScope::AllOpen),
        Some(RouteStep::Replace(work(
            WorkScope::AllOpen,
            Some(task("dTv7zpzTN"))
        )))
    );
    assert_eq!(open.show_work(canvas.clone()), None);
    assert_eq!(
        open.show_work(team.clone()),
        Some(RouteStep::Replace(work(
            team.clone(),
            Some(task("dTv7zpzTN"))
        )))
    );
    assert_eq!(
        work(team.clone(), Some(task("dTv7zpzTN"))).without_task(),
        Some(work(team, None))
    );

    assert_eq!(
        all.open_task(task("dTv7zpzTN"), canvas.clone()),
        Some(RouteStep::Push(open.clone()))
    );
    assert_eq!(
        open.open_task(task("dTv7xh1gD"), WorkScope::AllOpen),
        Some(RouteStep::Replace(work(
            WorkScope::AllOpen,
            Some(task("dTv7xh1gD"))
        )))
    );
    assert_eq!(open.open_task(task("dTv7zpzTN"), WorkScope::AllOpen), None);

    assert_eq!(open.without_task(), Some(work(canvas, None)));
    assert_eq!(all.without_task(), None);
    assert_eq!(chat.without_task(), None);
}

#[test]
fn settled_missing_segments_rewrite_to_their_containing_view() {
    let canvas = stem(WorkVisibility::Hidden, "canvas");
    let open = work(canvas.clone(), Some(task("dTv7zpzTN")));
    let card = AppRoute::card(CARD.to_owned()).expect("fixture card");
    let pending = RouteEvidence::default();
    let settled = |scope, task, card| RouteEvidence {
        scope,
        task,
        card,
        direction: None,
    };

    assert_eq!(open.unavailable(pending), None);
    assert_eq!(card.unavailable(pending), None);
    assert_eq!(
        open.unavailable(settled(Some(true), Some(true), None)),
        None
    );

    let (next, notice) = open
        .unavailable(settled(Some(false), None, None))
        .expect("missing stem refuses");
    assert_eq!(next, work(WorkScope::AllOpen, None));
    assert!(notice.contains(".canvas.*"), "{notice}");

    let (next, notice) = open
        .unavailable(settled(Some(false), Some(false), None))
        .expect("a missing stem outranks its task");
    assert_eq!(next, work(WorkScope::AllOpen, None));
    assert!(notice.contains(".canvas.*"), "{notice}");

    let missing_team = work(
        WorkScope::Team(uuid("01234567-89ab-cdef-0123-456789abcdef")),
        Some(task("dTv7zpzTN")),
    );
    let (next, notice) = missing_team
        .unavailable(settled(Some(false), Some(true), None))
        .expect("missing team refuses");
    assert_eq!(next, work(WorkScope::AllOpen, None));
    assert!(
        notice.contains("01234567-89ab-cdef-0123-456789abcdef"),
        "{notice}"
    );

    let (next, notice) = open
        .unavailable(settled(Some(true), Some(false), None))
        .expect("missing task refuses");
    assert_eq!(next, work(canvas, None));
    assert!(notice.contains("dTv7zpzTN"), "{notice}");

    let (next, notice) = card
        .unavailable(settled(None, None, Some(false)))
        .expect("missing card refuses");
    assert_eq!(next, AppRoute::Chat { card: None });
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

#[test]
fn global_notice_yields_only_to_an_online_admitted_lane_snapshot() {
    assert_eq!(
        app_notice(ConnectionStage::Starting, true, true),
        Some(AppNotice::LoadingApp)
    );
    assert_eq!(
        app_notice(ConnectionStage::Connecting, true, true),
        Some(AppNotice::OpeningResident)
    );
    assert_eq!(
        app_notice(ConnectionStage::Offline, true, true),
        Some(AppNotice::Reconnecting)
    );
    assert_eq!(
        app_notice(ConnectionStage::Online, false, false),
        Some(AppNotice::LoadingApp)
    );
    assert_eq!(
        app_notice(ConnectionStage::Online, true, false),
        Some(AppNotice::LoadingAgents)
    );
    assert_eq!(app_notice(ConnectionStage::Online, true, true), None);
}

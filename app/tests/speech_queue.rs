use app::speech_queue::{
    Advance, Enqueue, ExternalPause, MAX_NARRATION_CURSORS, NarrationCursors, NarrationMessage,
    SpeechQueue, SpeechRequest, SuppressAutomatic,
};

fn narration(lane: &str, text: &str) -> SpeechRequest {
    SpeechRequest::narration(lane.to_owned(), text.to_owned(), text.to_owned())
}

fn manual(lane: &str, key: &str, text: &str) -> SpeechRequest {
    SpeechRequest::message(lane.to_owned(), key.to_owned(), text.to_owned())
}

fn started(result: Enqueue) -> app::speech_queue::SpeechEntry {
    match result {
        Enqueue::Start(entry) => entry,
        Enqueue::Queued => panic!("an idle queue must start its head"),
    }
}

#[test]
fn one_global_head_survives_a_burst_and_each_team_keeps_one_fresh_tail() {
    let mut queue = SpeechQueue::default();
    let head = started(queue.enqueue(narration("team-a", "a1")));
    assert_eq!(head.message_key(), Some("a1"));

    assert_eq!(
        queue.enqueue(narration("team-b", "stale b")),
        Enqueue::Queued
    );
    assert_eq!(
        queue.enqueue(narration("team-b", "fresh b")),
        Enqueue::Queued
    );
    assert_eq!(
        queue.enqueue(narration("team-a", "fresh a")),
        Enqueue::Queued
    );
    assert!(queue.is_active(head.id()));
    assert_eq!(queue.pending(), 2);

    let b = match queue.finish(head.id()) {
        Advance::Start(entry) => entry,
        other => panic!("head did not advance to the oldest fresh tail: {other:?}"),
    };
    assert_eq!(b.lane(), Some("team-b"));
    assert_eq!(b.message_key(), Some("fresh b"));
    assert_eq!(b.text(), "fresh b");
    let a = match queue.finish(b.id()) {
        Advance::Start(entry) => entry,
        other => panic!("second team did not advance: {other:?}"),
    };
    assert_eq!(a.lane(), Some("team-a"));
    assert_eq!(a.text(), "fresh a");
    assert_eq!(queue.finish(a.id()), Advance::Idle);
}

#[test]
fn stop_and_lane_abandonment_cannot_revive_superseded_playback() {
    let mut queue = SpeechQueue::default();
    let head = started(queue.enqueue(manual("team-head", "surface", "surface")));
    assert_eq!(
        queue.enqueue(narration("removed", "drop me")),
        Enqueue::Queued
    );
    assert_eq!(
        queue.enqueue(narration("survivor", "keep me")),
        Enqueue::Queued
    );

    queue.abandon_lane("removed");
    let survivor = match queue.finish(head.id()) {
        Advance::Start(entry) => entry,
        other => panic!("abandonment removed another team: {other:?}"),
    };
    assert_eq!(survivor.lane(), Some("survivor"));
    queue.stop();
    queue.stop();
    assert!(!queue.active());
    assert_eq!(queue.pending(), 0);
    assert_eq!(queue.finish(survivor.id()), Advance::Ignored);
}

#[test]
fn an_external_pause_stops_manual_speech_but_advances_automatic_narration() {
    let mut manual_queue = SpeechQueue::default();
    let head = started(manual_queue.enqueue(manual("lineage", "surface", "surface")));
    assert_eq!(
        manual_queue.enqueue(narration("lineage", "queued")),
        Enqueue::Queued
    );
    assert_eq!(
        manual_queue.external_pause(head.id()),
        ExternalPause::Stopped
    );
    assert_eq!(manual_queue.pending(), 0);
    assert_eq!(manual_queue.finish(head.id()), Advance::Ignored);

    let mut automatic = SpeechQueue::default();
    let head = started(automatic.enqueue(narration("team-a", "head")));
    assert_eq!(
        automatic.enqueue(narration("team-b", "successor")),
        Enqueue::Queued
    );
    let successor = match automatic.external_pause(head.id()) {
        ExternalPause::Start(entry) => entry,
        other => panic!("paused narration did not advance its queue: {other:?}"),
    };
    assert_eq!(successor.text(), "successor");
}

#[test]
fn quiet_stops_active_and_queued_automatic_speech_but_preserves_manual_work() {
    let mut queue = SpeechQueue::default();
    let automatic = started(queue.enqueue(narration("team-a", "automatic head")));
    assert_eq!(
        queue.enqueue(narration("team-b", "automatic tail")),
        Enqueue::Queued
    );
    assert_eq!(queue.suppress_automatic(), SuppressAutomatic::Idle);
    assert!(!queue.active());
    assert_eq!(queue.pending(), 0);
    assert_eq!(queue.finish(automatic.id()), Advance::Ignored);

    assert_eq!(queue.suppress_automatic(), SuppressAutomatic::Preserved);
    let manual_head = started(queue.enqueue(manual("team-a", "manual", "explicit playback")));
    assert_eq!(
        queue.enqueue(narration("team-b", "discarded tail")),
        Enqueue::Queued
    );
    assert_eq!(queue.suppress_automatic(), SuppressAutomatic::Preserved);
    assert!(queue.is_active(manual_head.id()));
    assert_eq!(queue.pending(), 0);

    queue.stop();
    let automatic = started(queue.enqueue(narration("team-a", "automatic head")));
    assert_eq!(
        queue.enqueue(manual("team-b", "manual", "queued explicit playback")),
        Enqueue::Queued
    );
    let promoted = match queue.suppress_automatic() {
        SuppressAutomatic::Start(entry) => entry,
        other => panic!("quiet did not preserve the queued manual successor: {other:?}"),
    };
    assert_eq!(promoted.message_key(), Some("manual"));
    assert_eq!(queue.finish(automatic.id()), Advance::Ignored);
}

#[test]
fn narration_cursors_suppress_materialized_history_and_admit_only_live_suffixes() {
    let mut cursors = NarrationCursors::default();
    let history = vec![
        message("agent-a", "a1", "old a"),
        message("agent-b", "b1", "old b"),
    ];
    assert!(cursors.observe(true, &history).is_empty());

    let live = vec![
        history[0].clone(),
        automatic_message("agent-a", "a2", "live a", "selected a"),
        history[1].clone(),
        message("agent-b", "b2", "live b"),
    ];
    assert_eq!(
        cursors.observe(false, &live),
        [
            app::speech_queue::NarrationLine {
                lane: "agent-b".to_owned(),
                key: "b2".to_owned(),
                narration: Some("live b".to_owned()),
                automatic_speech: None,
            },
            app::speech_queue::NarrationLine {
                lane: "agent-a".to_owned(),
                key: "a2".to_owned(),
                narration: Some("live a".to_owned()),
                automatic_speech: Some("selected a".to_owned()),
            },
        ]
    );

    let restored = NarrationCursors::restore(cursors.rows());
    let old_reconnect = vec![
        live[1].clone(),
        message("agent-a", "a3", "arrived while disconnected"),
        live[3].clone(),
    ];
    let mut restored = restored;
    assert!(restored.observe(true, &old_reconnect).is_empty());
}

#[test]
fn a_two_message_narration_batch_admits_only_its_newest_message() {
    let mut cursors = NarrationCursors::default();
    let history = message("agent-a", "a1", "history");
    assert!(
        cursors
            .observe(true, std::slice::from_ref(&history))
            .is_empty()
    );

    let lines = cursors.observe(
        false,
        &[
            history,
            message("agent-a", "a2", "older unseen"),
            message("agent-a", "a3", "newest unseen"),
        ],
    );
    assert_eq!(
        lines
            .iter()
            .map(|line| line.narration.as_deref().expect("a narrated line speaks"))
            .collect::<Vec<_>>(),
        ["newest unseen"]
    );

    let mut busy_queue = SpeechQueue::default();
    let busy_head = started(busy_queue.enqueue(narration("team-b", "active head")));
    for line in lines.clone() {
        assert_eq!(
            busy_queue.enqueue(narration(&line.lane, line.narration.as_deref().unwrap())),
            Enqueue::Queued
        );
    }
    let newest = match busy_queue.finish(busy_head.id()) {
        Advance::Start(entry) => entry,
        other => panic!("busy queue did not advance to the newest batched line: {other:?}"),
    };
    assert_eq!(newest.text(), "newest unseen");

    let mut queue = SpeechQueue::default();
    let first = lines
        .into_iter()
        .find_map(|line| {
            match queue.enqueue(narration(&line.lane, line.narration.as_deref().unwrap())) {
                Enqueue::Start(entry) => Some(entry),
                Enqueue::Queued => None,
            }
        })
        .expect("an idle queue commits the first narration line");
    assert_eq!(first.text(), "newest unseen");
}

#[test]
fn cursor_storage_is_bounded_and_a_missing_cursor_rebases_without_reading_history() {
    let mut cursors = NarrationCursors::default();
    for index in 0..=MAX_NARRATION_CURSORS {
        let lane = format!("agent-{index}");
        let key = format!("message-{index}");
        assert!(
            cursors
                .observe(false, &[message(&lane, &key, "history")])
                .is_empty()
        );
    }
    assert_eq!(cursors.len(), MAX_NARRATION_CURSORS);
    assert_eq!(cursors.rows().first().unwrap().0, "agent-1");

    assert!(
        cursors
            .observe(
                false,
                &[
                    message("agent-1", "history-not-containing-cursor", "old"),
                    message("agent-1", "newest", "also old"),
                ],
            )
            .is_empty()
    );
}

fn message(lane: &str, key: &str, text: &str) -> NarrationMessage {
    NarrationMessage {
        lane: lane.to_owned(),
        key: key.to_owned(),
        narration: Some(text.to_owned()),
        automatic_speech: None,
    }
}

fn automatic_message(lane: &str, key: &str, text: &str, speech: &str) -> NarrationMessage {
    NarrationMessage {
        automatic_speech: Some(speech.to_owned()),
        ..message(lane, key, text)
    }
}

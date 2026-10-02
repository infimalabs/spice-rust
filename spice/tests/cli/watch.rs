use super::*;

#[test]
fn watch_snapshot_and_bounded_follow_exit() {
    let scratch = Scratch::new("watch-finite");
    let (_, tape) = session(scratch.path());
    tape.tee(&record(PROSE)).expect("fixture record is durable");
    let root = scratch.path().to_string_lossy().into_owned();

    let snapshot = invoke(&["watch", &root]);
    assert_eq!(snapshot.code, Some(0));
    assert!(snapshot.err.is_empty());
    let snapshot = utf8(snapshot.out);
    assert!(snapshot.contains("working"));
    assert!(snapshot.contains("watch end=snapshot"));
    assert!(snapshot.contains("cursor=watch.v1."));

    let followed = invoke(&[
        "--json",
        "watch",
        &root,
        "--follow",
        "--deadline-seconds",
        "0.05",
    ]);
    assert_eq!(followed.code, Some(0));
    assert!(followed.err.is_empty());
    let document: serde_json::Value =
        serde_json::from_slice(&followed.out).expect("bounded follow emits one JSON reply");
    assert_eq!(document["kind"], "reply");
    assert_eq!(document["body"]["end"], "deadline");
    assert!(
        document["body"]["cursor"]
            .as_str()
            .is_some_and(|cursor| cursor.starts_with("watch.v1."))
    );

    scratch.remove();
}

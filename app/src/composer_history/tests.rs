use super::*;

fn entries(history: &InputHistory, lane: &str) -> Vec<String> {
    history.entries(lane).to_vec()
}

#[test]
fn each_lane_keeps_its_own_sent_text_once_across_restore() {
    let mut history = InputHistory::default();
    history.record("a", ["first", "  ", "second"]);
    history.record("b", ["other"]);
    history.record("a", ["second", "first"]);

    assert_eq!(entries(&history, "a"), ["first", "second", "first"]);
    assert_eq!(entries(&history, "b"), ["other"]);
    assert_eq!(InputHistory::restore(&history.document()), history);
    assert!(InputHistory::restore("{not json").entries("a").is_empty());
}

#[test]
fn retention_keeps_each_lanes_newest_entries_and_trims_least_recent_lanes_first() {
    let mut history = InputHistory::default();
    let texts = (0..LANE_ENTRIES + 5)
        .map(|index| format!("entry {index}"))
        .collect::<Vec<_>>();
    history.record("a", texts.iter().map(String::as_str));
    assert_eq!(history.entries("a").len(), LANE_ENTRIES);
    assert_eq!(history.entries("a")[0], "entry 5");

    let large = "x".repeat(BUDGET_BYTES / 4);
    for lane in ["old", "middle", "new"] {
        history.record(lane, [large.as_str(), &format!("{large}{lane}")]);
    }
    history.record("a", ["latest"]);

    assert!(history.document().len() <= BUDGET_BYTES);
    assert!(history.entries("old").is_empty());
    assert_eq!(history.entries("new").len(), 2);
    assert_eq!(history.entries("a"), ["latest"]);
}

#[test]
fn a_restored_document_beyond_the_bounds_is_trimmed_before_use() {
    let large = "y".repeat(BUDGET_BYTES / 10);
    let mut lanes = (0..11)
        .map(|lane| LaneHistory {
            lane: format!("lane-{lane}"),
            entries: vec![large.clone()],
        })
        .collect::<Vec<_>>();
    lanes.push(LaneHistory {
        lane: "recent".to_owned(),
        entries: (0..=LANE_ENTRIES).map(|index| index.to_string()).collect(),
    });
    let oversized = InputHistory { lanes }.document();

    let restored = InputHistory::restore(&oversized);

    assert!(oversized.len() > BUDGET_BYTES);
    assert!(restored.document().len() <= BUDGET_BYTES);
    assert!(restored.entries("lane-0").is_empty());
    assert_eq!(restored.entries("lane-10"), [large]);
    assert_eq!(restored.entries("recent").len(), LANE_ENTRIES);
    assert_eq!(restored.entries("recent")[0], "1");
}

#[test]
fn recall_walks_a_snapshot_keeping_the_draft_and_edits_until_the_lane_sends() {
    let mut lane = vec!["one".to_owned(), "two".to_owned()];
    let mut recall = Recall::default();
    let older = |recall: &mut Recall, lane: &[String], current: &str| {
        recall.older(|| lane.to_vec(), current)
    };

    assert_eq!(recall.newer("draft"), None);
    assert_eq!(older(&mut recall, &lane, "draft").as_deref(), Some("two"));
    lane.push("other tab".to_owned());
    assert_eq!(
        older(&mut recall, &lane, "two edited").as_deref(),
        Some("one")
    );
    assert_eq!(older(&mut recall, &lane, "one"), None);
    assert_eq!(recall.newer("one").as_deref(), Some("two edited"));
    assert_eq!(recall.newer("two edited").as_deref(), Some("draft"));
    assert_eq!(recall.newer("draft"), None);
    assert_eq!(
        older(&mut recall, &lane, "draft changed").as_deref(),
        Some("two edited")
    );
    recall.sync(0);
    assert_eq!(recall.newer("two").as_deref(), Some("draft changed"));
    assert_eq!(
        older(&mut recall, &lane, "draft changed").as_deref(),
        Some("two")
    );

    recall.sync(1);
    assert_eq!(recall.newer("two"), None);
    assert_eq!(older(&mut recall, &lane, "").as_deref(), Some("other tab"));
}

#[test]
fn only_a_collapsed_caret_on_the_outer_line_leaves_the_text() {
    let text = "first\nsecond\nthird";
    assert!(leaves_text(text, 0, 0, true));
    assert!(leaves_text(text, 5, 5, true));
    assert!(!leaves_text(text, 6, 6, true));
    assert!(!leaves_text(text, 0, 0, false));
    assert!(leaves_text(text, 13, 13, false));
    assert!(leaves_text(text, 18, 18, false));
    assert!(!leaves_text(text, 0, 3, true));
    assert!(leaves_text("", 0, 0, true) && leaves_text("", 0, 0, false));

    let wide = "😀\nnext";
    assert!(leaves_text(wide, 2, 2, true));
    assert!(!leaves_text(wide, 3, 3, true));
}

#[test]
fn a_utf16_caret_splits_on_character_boundaries() {
    assert_eq!(split_utf16("ab\ncd", 3), ("ab\n", "cd"));
    assert_eq!(split_utf16("😀x", 2), ("😀", "x"));
    assert_eq!(split_utf16("😀x", 1), ("", "😀x"));
    assert_eq!(split_utf16("éx", 1), ("é", "x"));
    assert_eq!(split_utf16("ab", 9), ("ab", ""));
    assert_eq!(split_utf16("", 0), ("", ""));
}

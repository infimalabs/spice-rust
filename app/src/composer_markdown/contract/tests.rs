use super::render_task_contract;

const KNOWN: &str = "MOSAIC-1kRYqQxy";
const ALSO_KNOWN: &str = "CONTROL-1kKbC3j4";
const UNKNOWN: &str = "IDENTIT-dRyDYZqJJ";

fn render(markdown: &str) -> String {
    let known = [KNOWN, ALSO_KNOWN].map(|handle| {
        handle
            .parse::<wire::TaskHandle>()
            .expect("fixture handle")
            .born()
            .expect("fixture stamp")
    });
    render_task_contract(markdown, |stamp| known.contains(stamp))
}

fn chips(html: &str) -> Vec<&str> {
    html.split("data-task-handle=\"")
        .skip(1)
        .filter_map(|tail| tail.split('"').next())
        .collect()
}

#[test]
fn only_board_resolved_handles_become_chips_at_every_mention() {
    let html = render(&format!(
        "Depends on ({KNOWN}), {ALSO_KNOWN}; then {KNOWN}. Not {UNKNOWN} or X-{KNOWN}x."
    ));

    assert_eq!(chips(&html), [KNOWN, ALSO_KNOWN, KNOWN]);
    assert!(
        html.contains(&format!("Not {UNKNOWN} or X-{KNOWN}x.")),
        "{html}"
    );
    assert!(
        html.contains("(<span"),
        "punctuation stays outside the chip: {html}"
    );
}

#[test]
fn code_and_link_text_never_gain_task_navigation() {
    let html = render(&format!(
        "Run `{KNOWN}` and [{ALSO_KNOWN}](https://example.test/a).\n\n```\n{KNOWN}\n```"
    ));

    assert!(chips(&html).is_empty(), "{html}");
    assert!(html.contains(&format!("<code>{KNOWN}</code>")), "{html}");
    assert!(
        html.contains(&format!(
            "<a href=\"https://example.test/a\">{ALSO_KNOWN}</a>"
        )),
        "{html}"
    );
}

#[test]
fn raw_html_and_unsafe_schemes_stay_inert() {
    let html = render(
        "<img src=x onerror=alert(1)> [run](javascript:alert(1)) [mail](mailto:a@example.test)",
    );

    assert!(!html.contains("<img"), "{html}");
    assert!(
        html.contains("&lt;img src=x onerror=alert(1)&gt;"),
        "{html}"
    );
    assert!(html.contains("<a href=\"\">run</a>"), "{html}");
    assert!(
        html.contains("<a href=\"mailto:a@example.test\">mail</a>"),
        "{html}"
    );
}

#[test]
fn authored_structure_survives_under_the_detail_headings() {
    let html = render(&format!(
        "# Scope\nfirst line\nsecond {KNOWN}\n\n- one\n- two"
    ));

    assert!(html.contains("<h5>Scope</h5>"), "{html}");
    assert!(html.contains("first line<br />"), "{html}");
    assert!(html.contains("<li>one</li>\n<li>two</li>"), "{html}");
    assert_eq!(chips(&html), [KNOWN]);
}

use std::fs;

use spice::repository::Repository;

use super::{Access, Query, Request, Resource, Text, rewrite_continuations, served_reads};
use crate::board_command::{TaskText, ledger_output};

const HANDLE: &str = "TASK-dVlFq9qKp";

fn arguments(url: &str) -> Vec<String> {
    match Request::parse(url) {
        Ok(Request {
            resource: Resource::Read(query),
            ..
        }) => query.arguments(),
        other => panic!("{url} names no CLI read: {other:?}"),
    }
}

fn refusal(url: &str) -> String {
    Request::parse(url).expect_err(url)
}

#[test]
fn each_resource_runs_exactly_its_cli_read() {
    for (url, expected) in [
        ("spice://board", "task status"),
        ("spice://task", "task list"),
        (
            "spice://task?all&status=completed&limit=5&after=TASK-dVlFq9qKp",
            "task list --all --status completed --limit 5 --after TASK-dVlFq9qKp",
        ),
        (
            "spice://task?project=spice.task",
            "task list --project spice.task",
        ),
        ("spice://task/TASK-dVlFq9qKp", "task show TASK-dVlFq9qKp"),
        ("spice://task/TASK-dVlFq9qKp/", "task show TASK-dVlFq9qKp"),
        (
            "spice://task/TASK-dVlFq9qKp/context",
            "task show TASK-dVlFq9qKp --context",
        ),
        (
            "spice://task/TASK-dVlFq9qKp/history",
            "task show TASK-dVlFq9qKp --history",
        ),
        (
            "spice://task/TASK-dVlFq9qKp/detail?limit=3&after=context%3A2",
            "task show TASK-dVlFq9qKp --context --history --limit 3 --after context:2",
        ),
        (
            "spice://task/TASK-dVlFq9qKp/artifact?max-bytes=900",
            "task artifact list TASK-dVlFq9qKp --max-bytes 900",
        ),
        (
            "spice://task/TASK-dVlFq9qKp/artifact/a1?offset=4096&bytes=10",
            "task artifact show TASK-dVlFq9qKp a1 --offset 4096 --bytes 10",
        ),
        (
            "spice://task/TASK-dVlFq9qKp/room?limit=2",
            "room history TASK-dVlFq9qKp --limit 2",
        ),
        (
            "spice://task/TASK-dVlFq9qKp/preparation?limit=2&after=Ab",
            "preparation show TASK-dVlFq9qKp --limit 2 --after Ab",
        ),
        (
            "spice://task/TASK-dVlFq9qKp/ledger?max-bytes=900",
            "doc ledger TASK-dVlFq9qKp --max-bytes 900",
        ),
        (
            concat!(
                "spice://task/TASK-dVlFq9qKp/ledger?max-bytes=900&offset=64&digest=",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
            concat!(
                "doc ledger TASK-dVlFq9qKp --max-bytes 900 --offset 64 --digest ",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        ),
        (
            "spice://board/graph/dependencies?all&limit=5&at=x",
            "board graph dependencies --all --limit 5 --at x",
        ),
        ("spice://inbox?limit=2", "inbox --limit 2"),
        ("spice://lane", "lane list"),
        (
            "spice://session/briefing?file=a.jsonl&deadline-seconds=5",
            "session briefing a.jsonl --deadline-seconds 5",
        ),
        (
            "spice://session/turns?tool=shell&file=a&tool=read&file=b&view=full",
            "session turns a b --tool shell --tool read --view full",
        ),
        ("spice://session/sweep?limit=4", "session sweep --limit 4"),
        (
            "spice://session/context/S1?lane=L&vendor=omp&stream=session&source=U&member=1&offset=0&after=c%3A2",
            "session context S1 --lane L --vendor omp --stream session --source U --member 1 --offset 0 --after c:2",
        ),
    ] {
        assert_eq!(arguments(url).join(" "), expected, "{url}");
    }
}

#[test]
fn a_served_seat_reads_each_quoted_served_read_as_its_url_and_nothing_else() {
    let text = concat!(
        "- … 3 more: `spice session sweep /t/x.jsonl --limit 4`\n",
        "- board: `spice task status`; steering: `spice inbox`; peers: `spice lane list`\n",
        "- typed: `spice session briefing --json`; act: `spice task next`\n",
        "- held: `spice task show HANDLE` and `spice task show TASK-dVlFq9qKp`\n",
        "- prose `not a command` then an unbalanced ` spice inbox",
    );
    let served = served_reads(text);
    for url in [
        "`spice://board`",
        "`spice://inbox`",
        "`spice://lane`",
        "`spice://task/TASK-dVlFq9qKp`",
    ] {
        assert!(served.contains(url), "{url} missing: {served}");
    }
    let sweep = served
        .lines()
        .next()
        .and_then(|line| line.split('`').nth(1))
        .expect("first row quotes its read");
    assert_eq!(
        Query::from_arguments(&arguments(sweep)).map(|query| query.url()),
        Some(sweep.to_owned()),
        "the cut row's URL resumes the same sweep"
    );
    assert_eq!(
        arguments(sweep).join(" "),
        "session sweep /t/x.jsonl --limit 4"
    );
    for kept in [
        "`spice session briefing --json`",
        "`spice task next`",
        "`spice task show HANDLE`",
        "`not a command`",
        "unbalanced ` spice inbox",
    ] {
        assert!(served.contains(kept), "{kept} rewritten: {served}");
    }
}

#[test]
fn the_root_skill_and_quote_targets_are_not_cli_reads() {
    assert_eq!(
        Request::parse("spice://")
            .expect("the root is the index")
            .resource,
        Resource::Index
    );
    assert_eq!(
        Request::parse(spice::install::SKILL_RESOURCE)
            .expect("the installed skill is static")
            .resource,
        Resource::Skill
    );
    assert_eq!(
        Request::parse("spice://quote/v1/steer/1kQnrzNZ/00:2-3")
            .expect("a quote target with a selector")
            .resource,
        Resource::Quote("spice://quote/v1/steer/1kQnrzNZ/00".to_owned())
    );
}

#[test]
fn the_installed_skill_resource_is_exact_bounded_and_prompt_compact() {
    let ambient = spice::ambient::capture();
    let seat = super::Seat {
        worktree: "/".into(),
        lane: wire::CanonicalUuid::new(uuid::Uuid::from_u128(7)),
        environment: crate::session_environment(&ambient).rooted("/".into()),
        ambient,
        graphs: Default::default(),
    };
    let read = |url: &str| {
        seat.serve(url, Access::Read, |_| panic!("no quote is read"))
            .expect("the static skill serves")
    };
    let document = spice::install::skill_document();
    let bootstrap = spice::install::skill_bootstrap();
    assert_eq!(read(spice::install::SKILL_RESOURCE), document);
    assert_eq!(
        read("spice://SKILL.md:1-2"),
        document.lines().take(2).collect::<Vec<_>>().join("\n")
    );
    assert!(
        document.len() <= spice::install::SKILL_BYTES_CEILING,
        "the authoritative document stays within its 8 KiB ceiling"
    );
    assert!(
        bootstrap.len() < spice::install::SKILL_BOOTSTRAP_BYTES_CEILING,
        "the bootstrap stays below its separate 1 KiB budget"
    );
    assert!(bootstrap.contains(spice::install::SKILL_RESOURCE));
    assert!(!bootstrap.contains(document));
    refusal("spice://SKILL.md?release=current");
}

#[test]
fn unknown_paths_and_malformed_handles_are_refused() {
    for url in [
        "spice://?limit=3",
        "spice://board/graph",
        "spice://session/turns/x",
        "spice://task/TASK-dVlFq9qKp/notes",
        "spice://task/TASK-dVlFq9qKp/artifact/a1/raw",
        "http://task",
    ] {
        let refused = refusal(url);
        assert!(!refused.contains("does not serve"), "{url}: {refused}");
    }
    assert!(refusal("spice://task/not-a-handle").contains("not-a-handle"));
    refusal("spice://task/TASK-dVlFq9qKp%ZZ");
}

#[test]
fn urls_naming_unserved_commands_are_refused_naming_them() {
    for (url, command) in [
        ("spice://task/next", "spice task next"),
        ("spice://task/add", "spice task add"),
        ("spice://task/TASK-dVlFq9qKp/done", "spice task done"),
        ("spice://task/TASK-dVlFq9qKp/review", "spice task review"),
        ("spice://task/TASK-dVlFq9qKp/modify", "spice task modify"),
        (
            "spice://task/TASK-dVlFq9qKp/artifact/add",
            "spice task artifact add",
        ),
        (
            "spice://task/TASK-dVlFq9qKp/artifact/prune",
            "spice task artifact prune",
        ),
        ("spice://lane/add/x", "spice lane add"),
        ("spice://steer", "spice steer"),
        ("spice://board/handout", "spice board handout"),
        ("spice://doc", "spice doc"),
    ] {
        let refused = refusal(url);
        assert!(
            refused.contains(&format!("`{command}`")) && refused.contains("does not serve"),
            "{url}: {refused}"
        );
    }
}

#[test]
fn query_keys_are_those_the_read_admits_each_once() {
    for url in [
        "spice://board?limit=3",
        "spice://task/TASK-dVlFq9qKp?offset=3",
        "spice://task?output=x",
        "spice://task?limit",
        "spice://task?limit=",
        "spice://task?all=yes",
        "spice://task?limit=3&limit=4",
        "spice://task/TASK-dVlFq9qKp/artifact/a1?output=/tmp/x",
        "spice://session/turns?export=x",
        "spice://session/turns?view=full&view=dialogue",
        "spice://session/briefing?file=",
        "spice://board/graph/review?output=x",
        "spice://inbox?all",
    ] {
        refusal(url);
    }
}

#[test]
fn a_task_summary_takes_no_query_and_names_the_detail_reads_that_do() {
    for query in [
        "limit=3",
        "max-bytes=4000",
        "max-text=4000",
        "after=x",
        "history",
    ] {
        let url = format!("spice://task/{HANDLE}?{query}");
        let refused = refusal(&url);
        let told = refused
            .strip_prefix(url.as_str())
            .expect("the refusal names its URL");
        assert!(
            told.contains(&format!("spice://task/{HANDLE}/context"))
                && told.contains("/history")
                && told.contains("/detail"),
            "{url}: {refused}"
        );
        for key in ["limit", "max-bytes", "max-text", "after"] {
            assert!(told.contains(key), "{url}: {refused}");
        }
    }
    for (detail, flags) in [
        ("context", &["--context"][..]),
        ("history", &["--history"][..]),
        ("detail", &["--context", "--history"][..]),
    ] {
        let url =
            format!("spice://task/{HANDLE}/{detail}?limit=3&max-bytes=4000&max-text=4000&after=x");
        let mut expected = vec!["task", "show", HANDLE];
        expected.extend(flags);
        expected.extend([
            "--limit",
            "3",
            "--max-bytes",
            "4000",
            "--max-text",
            "4000",
            "--after",
            "x",
        ]);
        assert_eq!(arguments(&url), expected, "{url}");
    }
    let summary = Query::from_arguments(&["task", "show", HANDLE].map(str::to_owned))
        .expect("a task summary is a served read");
    assert_eq!(summary.url(), format!("spice://task/{HANDLE}"));
}

#[test]
fn a_context_url_without_every_coordinate_names_each_absent_one() {
    let url = "spice://session/context/S1?vendor=omp&stream=session&member=1&offset=0&limit=3";
    let refused = refusal(url);
    let told = refused
        .strip_prefix(url)
        .expect("the refusal names its URL");
    for absent in ["lane", "source"] {
        assert!(told.contains(absent), "{absent} unnamed: {refused}");
    }
    for present in ["vendor", "stream", "member", "offset", "limit"] {
        assert!(!told.contains(present), "{present} named absent: {refused}");
    }
    let command = ["session", "context", "S1", "--lane", "L", "--limit", "3"];
    assert_eq!(
        Query::from_arguments(&command.map(str::to_owned)),
        None,
        "a command missing coordinates is no served read"
    );
}

#[test]
fn a_task_view_names_its_context_read_as_the_url_that_runs_it() {
    let command = "spice session context 'S 1' --lane L --vendor omp --stream session --source U --member 1 --offset 40";
    let view = format!(
        "creation_context session=S\n{command}\n  spice session context quoted in a description\nspice session context S2 --lane L"
    );
    let rewritten = super::context_urls(&view);
    let lines = rewritten.lines().collect::<Vec<_>>();
    assert_eq!(lines[0], "creation_context session=S");
    assert_eq!(
        arguments(lines[1]),
        shell_words::split(command.strip_prefix("spice ").expect("command"))
            .expect("rendered command is shell words"),
        "the URL runs exactly the rendered read"
    );
    assert_eq!(&lines[2..], &view.lines().collect::<Vec<_>>()[2..]);
}

#[test]
fn a_task_text_url_names_its_field_and_the_digest_a_write_carries() {
    let digest = wire::Digest::compute(wire::Domain::TaskText, b"read text");
    let handle: wire::TaskHandle = HANDLE.parse().expect("fixture handle");
    for (url, field, named) in [
        (
            format!("spice://task/{HANDLE}/description"),
            TaskText::Description,
            None,
        ),
        (
            format!("spice://task/{HANDLE}/acceptance?digest={digest}"),
            TaskText::Acceptance,
            Some(digest.clone()),
        ),
    ] {
        assert_eq!(
            Request::parse(&url).expect(&url).resource,
            Resource::Text(Text {
                handle: handle.clone(),
                field,
                digest: named,
            }),
            "{url}"
        );
    }
    for url in [
        format!("spice://task/{HANDLE}/description?digest=abc"),
        format!("spice://task/{HANDLE}/description?limit=3"),
        format!("spice://task/{HANDLE}/acceptance?digest={digest}&digest={digest}"),
        format!("spice://task/{HANDLE}/title"),
    ] {
        refusal(&url);
    }
}

#[test]
fn only_a_whole_task_text_takes_a_write_and_only_a_write_takes_a_digest() {
    let ambient = spice::ambient::capture();
    let seat = super::Seat {
        worktree: "/".into(),
        lane: wire::CanonicalUuid::new(uuid::Uuid::from_u128(7)),
        environment: crate::session_environment(&ambient).rooted("/".into()),
        ambient,
        graphs: Default::default(),
    };
    let digest = wire::Digest::compute(wire::Domain::TaskText, b"read text");
    let serve = |url: &str, access| seat.serve(url, access, |_| panic!("no quote is read"));
    for url in [
        "spice://".to_owned(),
        spice::install::SKILL_RESOURCE.to_owned(),
        "spice://board".to_owned(),
        format!("spice://task/{HANDLE}"),
        format!("spice://task/{HANDLE}/context"),
        format!("spice://task/{HANDLE}/description?digest={digest}:2-3"),
    ] {
        serve(&url, Access::Write("text")).expect_err(&url);
    }
    serve(
        &format!("spice://task/{HANDLE}/description?digest={digest}"),
        Access::Read,
    )
    .expect_err("a read names no digest");
}

#[test]
fn out_of_range_limits_are_refused_by_the_cli_read() {
    let root = std::env::temp_dir().join(format!("spice-resource-limits-{}", std::process::id()));
    fs::create_dir_all(root.join(".git")).expect("fixture Git directory exists");
    let repository = Repository::discover(&root).expect("fixture repository resolves");
    let ambient = spice::ambient::capture();
    let seat = super::Seat {
        worktree: root.clone(),
        lane: "ca38ffa1-2c8c-4ac2-87ae-64a413e15261"
            .parse()
            .expect("lane UUID"),
        environment: crate::session_environment(&ambient).rooted(root.clone()),
        ambient,
        graphs: Default::default(),
    };
    for url in [
        "spice://task?limit=100000",
        "spice://task?max-bytes=0",
        "spice://task/TASK-dVlFq9qKp/detail?max-text=1000000",
        "spice://task/TASK-dVlFq9qKp/history?limit=-1",
    ] {
        let Ok(Request {
            resource: Resource::Read(query),
            ..
        }) = Request::parse(url)
        else {
            panic!("{url} parses");
        };
        let refused = seat.run(&repository, &query).expect_err(url);
        assert!(!refused.is_empty(), "{url}");
    }
    fs::remove_dir_all(&root).expect("fixture cleanup");
}

#[test]
fn a_continuation_url_resumes_exactly_the_command_it_replaces() {
    let commands: [&[&str]; 9] = [
        &[
            "task",
            "show",
            HANDLE,
            "--context",
            "--history",
            "--limit",
            "20",
            "--max-bytes",
            "16000",
            "--max-text",
            "4096",
            "--after",
            "context:20",
        ],
        &[
            "task",
            "list",
            "--project",
            "spice.task",
            "--status",
            "pending",
            "--limit",
            "3",
            "--max-bytes",
            "8192",
            "--max-text",
            "240",
            "--after",
            HANDLE,
        ],
        &[
            "task",
            "artifact",
            "list",
            HANDLE,
            "--after",
            "a 1/x",
            "--limit",
            "20",
            "--max-bytes",
            "8192",
        ],
        &[
            "task", "artifact", "show", HANDLE, "a1", "--offset", "4096", "--bytes", "10",
        ],
        &[
            "room",
            "history",
            HANDLE,
            "--limit",
            "2",
            "--after",
            "dVsJd2J43",
            "--max-bytes",
            "8192",
        ],
        &["room", "inbox", "--limit", "1", "--after", "dVv51t4wR"],
        &[
            "session",
            "turns",
            "/tmp/a b.jsonl",
            "/tmp/c.jsonl",
            "--tool",
            "shell",
            "--tool",
            "read",
            "--limit",
            "3",
            "--after",
            "turn:7",
        ],
        &[
            "session",
            "context",
            "01a1276a-903c-7071-89d5-345c833e93c2",
            "--lane",
            "ca38ffa1-2c8c-4ac2-87ae-64a413e15261",
            "--vendor",
            "omp",
            "--stream",
            "session",
            "--source",
            "4df45d2d-b61a-4fe0-a041-6e65e1403cfd",
            "--member",
            "1",
            "--offset",
            "40",
            "--after",
            "row:40",
        ],
        &[
            "doc",
            "ledger",
            HANDLE,
            "--max-bytes",
            "4000",
            "--offset",
            "64",
            "--digest",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ],
    ];
    for command in commands {
        let command = command
            .iter()
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>();
        let url = Query::from_arguments(&command)
            .expect("a served read has a URL")
            .url();
        assert_eq!(arguments(&url), command, "{url}");
    }
    let mutation = ["task", "done", HANDLE].map(str::to_owned);
    assert_eq!(Query::from_arguments(&mutation), None);
}

#[test]
fn a_page_names_its_continuation_as_a_url() {
    let command = wire::SpiceCommand::new(
        "task",
        [
            "show",
            HANDLE,
            "--history",
            "--limit",
            "2",
            "--after",
            "step:9",
        ],
    )
    .expect("continuation command");
    let rows = [1_u8, 2, 3];
    let mut admission = wire::output::Admission::new(
        wire::output::INTERACTIVE
            .budget(wire::output::Request {
                rows: Some(2),
                bytes: None,
                text: None,
            })
            .expect("budget"),
    );
    for row in rows {
        admission.offer(row).expect("row encodes");
    }
    let omission = admission
        .finish(|_| command.clone())
        .omission
        .expect("a row is withheld");
    let page = format!("handle {HANDLE}\n{omission}\nnext: spice task done {HANDLE}");
    let rewritten = rewrite_continuations(&page);
    let url = format!("spice://task/{HANDLE}/history?limit=2&after=step%3A9");
    assert_eq!(
        rewritten,
        page.replace(&command.to_string(), &url),
        "only the continuation changes"
    );
    assert_eq!(
        arguments(&url),
        command.arguments().to_vec(),
        "the URL resumes the same read"
    );
}

#[test]
fn a_bounded_ledger_page_names_its_native_continuation() {
    let task = HANDLE.parse().expect("fixture handle");
    let document = format!("# Ledger\n\n{}\n", "é".repeat(40));
    let budget = wire::output::INTERACTIVE
        .budget(wire::output::Request {
            rows: None,
            bytes: Some(17),
            text: None,
        })
        .expect("small ledger page is admitted");
    let page = ledger_output(&task, &document, None, budget, 0, None).expect("ledger page renders");
    let page = rewrite_continuations(&page);
    let url = page
        .lines()
        .find_map(|line| line.split_once("; continue: ").map(|(_, url)| url))
        .expect("ledger page carries a continuation URL");
    let offset = wire::output::clip(&document, budget.limits().bytes)
        .text
        .len();
    let digest = wire::Digest::compute(wire::Domain::CliExport, document.as_bytes());
    assert_eq!(
        url,
        format!(
            "spice://task/{HANDLE}/ledger?max-bytes={}&offset={offset}&digest={digest}",
            budget.limits().bytes
        )
    );
    assert_eq!(
        arguments(url),
        vec![
            "doc".to_owned(),
            "ledger".to_owned(),
            HANDLE.to_owned(),
            "--max-bytes".to_owned(),
            budget.limits().bytes.to_string(),
            "--offset".to_owned(),
            offset.to_string(),
            "--digest".to_owned(),
            digest.to_string(),
        ],
    );
}

#[test]
fn a_selector_selects_lines_or_is_refused() {
    let text = "one\ntwo\nthree\nfour\nfive";
    let select = |url: &str| {
        let request = Request::parse(url)?;
        request.selector.expect("a selector is named").select(text)
    };
    for (selector, expected) in [
        ("2-3", "two\nthree"),
        ("4", "four\nfive"),
        ("4-", "four\nfive"),
        ("2+2", "two\nthree"),
        ("-2", "four\nfive"),
        ("1,5", "one\ntwo\nthree\nfour\nfive\nfive"),
        ("3-9", "three\nfour\nfive"),
        ("raw", text),
        ("2-3:raw", "two\nthree"),
    ] {
        assert_eq!(
            select(&format!("spice://board:{selector}")).as_deref(),
            Ok(expected),
            ":{selector}"
        );
    }
    assert!(select("spice://board:6").is_err(), "a start past the end");
    for selector in [
        "img",
        "0",
        "3-2",
        "x",
        "2-3:img",
        "",
        "2+18446744073709551615",
    ] {
        assert!(
            Request::parse(&format!("spice://board:{selector}")).is_err(),
            ":{selector}"
        );
    }
}

#[test]
fn a_task_list_ends_with_each_shown_task_url() {
    let mut page = format!(
        "scope global --all\n{HANDLE} [todo] spice.task Resolve reads\nAPP-dVlMW4bKm [todo] spice.app Show jobs\nwithheld 1 of 3 rows"
    );
    super::board_trailer(&mut page, &super::Shape::Tasks);
    assert!(
        page.ends_with(&format!(
            "\nresources:\n  spice://task/{HANDLE}\n  spice://task/APP-dVlMW4bKm"
        )),
        "{page}"
    );
}

use super::*;

#[test]
fn default_task_list_selects_each_complete_actor_route_branch_only() {
    let (root, bus, epoch, _) = fixture("task-list-actor-route", 0);
    let public = add_project(
        &bus,
        epoch,
        Actor::Operator,
        "CLI-route0001",
        "Public route row",
        "task.cli",
        Some("todo"),
    );
    let mut answer = board(&bus, epoch, Actor::Operator);
    let (included, excluded) = add_actor_route_rows(&mut answer, &public);
    let listed = task_list(
        &answer,
        &TaskListOptions {
            all: false,
            project: None,
            limit: None,
            status: None,
        },
    );
    for included in included {
        assert!(
            listed.contains(included.as_str()),
            "missing {included}: {listed}"
        );
    }
    assert!(!listed.contains(excluded.as_str()), "{listed}");
    assert!(listed.starts_with(concat!(
        "scope actor-route filter ",
        "( project:agent.operator.task or origin_thread.is:operator or ",
        "( project:board or project:rim or project:serve or project:task ) )\n"
    )));
    let status = task_status(&answer).expect("planted actor-route rows validate");
    assert!(status.lines().any(|line| line == "ready 3"), "{status}");
    assert!(
        status.lines().any(|line| {
            line == concat!(
                "filter ( project:agent.operator.task or origin_thread.is:operator or ",
                "( project:board or project:rim or project:serve or project:task ) )"
            )
        }),
        "{status}"
    );
    clean(&root, bus);
}

fn add_actor_route_rows(
    answer: &mut BoardAnswer,
    public: &TaskHandle,
) -> ([TaskHandle; 3], TaskHandle) {
    let template = at(&answer, &public).clone();
    answer
        .tasks
        .iter_mut()
        .find(|task| task.statement.body.handle == *public)
        .expect("public route row")
        .origin_thread = "someone_else".to_owned();
    let mut actor_private = template.clone();
    actor_private.statement.body.born =
        wire::TaskStamp::for_millis(8_002).expect("fixture task stamp");
    actor_private.statement.body.project = answer.actor_route.project.clone();
    actor_private.statement.body.handle = TaskHandle::for_project(
        &actor_private.statement.body.project,
        &actor_private.statement.body.born,
    );
    actor_private.statement.body.provenance = "task:0000002z"
        .parse()
        .expect("actor-private fixture origin");
    actor_private.origin_thread = "someone_else".to_owned();
    actor_private.presentation_title = "Actor-private route row".to_owned();

    let mut origin_thread = template.clone();
    origin_thread.statement.body.born =
        wire::TaskStamp::for_millis(8_003).expect("fixture task stamp");
    origin_thread.statement.body.project = "external.route".to_owned();
    origin_thread.statement.body.handle = TaskHandle::for_project(
        &origin_thread.statement.body.project,
        &origin_thread.statement.body.born,
    );
    origin_thread.statement.body.provenance = "task:0000002B"
        .parse()
        .expect("origin-thread fixture provenance");
    origin_thread.origin_thread = answer.actor_route.origin_thread.clone();
    origin_thread.presentation_title = "Origin-thread route row".to_owned();

    let mut outside = template;
    outside.statement.body.born = wire::TaskStamp::for_millis(8_004).expect("fixture task stamp");
    outside.statement.body.project = "agent.someone_else.task".to_owned();
    outside.statement.body.handle = TaskHandle::for_project(
        &outside.statement.body.project,
        &outside.statement.body.born,
    );
    outside.statement.body.provenance =
        "task:0000002C".parse().expect("outside fixture provenance");
    outside.origin_thread = "someone_else".to_owned();
    outside.presentation_title = "Outside route row".to_owned();

    let included = [
        public.clone(),
        actor_private.statement.body.handle.clone(),
        origin_thread.statement.body.handle.clone(),
    ];
    let excluded = outside.statement.body.handle.clone();
    answer.tasks.extend([actor_private, origin_thread, outside]);
    (included, excluded)
}

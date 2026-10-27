use super::*;
use crate::work::tests::task;

fn seq(value: u64) -> wire::Seq {
    wire::Seq::new(value).expect("fixture sequence is positive")
}

fn draft(value: i64) -> BoardTask {
    let mut row = task(value, "spice.app");
    row.presentation_title = format!("Draft {value}");
    row.statement.body.filing = wire::TaskFiling::Draft;
    row.statement.body.description = Some("Before".to_owned());
    row.statement.body.acceptance = vec!["One".to_owned(), "Two".to_owned()];
    row
}

/// `row` as a later statement fact at `position` left it.
fn revised(row: &BoardTask, position: u64, revise: impl FnOnce(&mut BoardTask)) -> BoardTask {
    let mut next = row.clone();
    next.statement.position = seq(position);
    revise(&mut next);
    next
}

fn save(form: &DraftForm) -> (TaskEdit, Vec<Witness>) {
    let Ok(DraftRequest::Edit { request, expected }) = form.revision() else {
        panic!("the form revises its Draft");
    };
    (request, expected)
}

fn witness(row: &BoardTask, position: u64) -> Vec<Witness> {
    vec![Witness::Task {
        task: row.statement.body.born.clone(),
        head: seq(position),
    }]
}

#[test]
fn an_opened_form_holds_the_admitted_content_and_saves_nothing_untouched() {
    let row = draft(1);
    let form = DraftForm::open(&row);
    let fields = form.fields();
    assert_eq!(
        [
            DraftField::Title,
            DraftField::Project,
            DraftField::Intent,
            DraftField::Flow,
            DraftField::Description,
            DraftField::Acceptance,
        ]
        .map(|field| fields.get(field)),
        ["Draft 1", "spice.app", "", "todo", "Before", "One\nTwo"],
    );
    assert!(form.revision().is_err());
}

#[test]
fn refreshes_that_carry_no_newer_revision_keep_every_typed_field() {
    let row = draft(1);
    let mut form = DraftForm::open(&row);
    form.edit(DraftField::Description, "Typed".to_owned());
    form.edit(DraftField::Title, "Typed title".to_owned());
    let typed = form.fields().clone();

    let mut claimed = row.clone();
    claimed.grouped = true;
    let mut behind = revised(&row, 1, |row| row.presentation_title = "Older".to_owned());
    behind.workflow.position = seq(1);
    for refresh in [&row, &claimed, &behind] {
        form.refresh(refresh);
    }

    assert_eq!(form.fields(), &typed);
    let (request, expected) = save(&form);
    assert_eq!(request.title.as_deref(), Some("Typed title"));
    assert_eq!(request.description.as_deref(), Some("Typed"));
    assert_eq!(expected, witness(&row, 1));
}

#[test]
fn a_newer_revision_rebases_untouched_fields_and_keeps_typed_ones() {
    let row = draft(1);
    let mut form = DraftForm::open(&row);
    form.edit(DraftField::Description, "Typed".to_owned());

    form.refresh(&revised(&row, 5, |row| {
        row.presentation_title = "Retitled elsewhere".to_owned();
        row.statement.body.description = Some("Elsewhere".to_owned());
    }));

    assert_eq!(form.fields().get(DraftField::Title), "Retitled elsewhere");
    assert_eq!(form.fields().get(DraftField::Description), "Typed");
    let (request, expected) = save(&form);
    assert_eq!(request.title, None);
    assert_eq!(request.description.as_deref(), Some("Typed"));
    assert_eq!(expected, witness(&row, 5));
}

#[test]
fn only_a_landed_save_adopts_the_admitted_spelling_of_what_it_sent() {
    let row = draft(1);
    let admitted = revised(&row, 5, |row| {
        row.statement.body.acceptance = vec!["One".to_owned(), "Three".to_owned()];
    });
    let typing = |form: &mut DraftForm| {
        form.edit(DraftField::Acceptance, "One\n\n  Three ".to_owned());
        let sent = form.fields().clone();
        form.edit(DraftField::Description, "Typed after Save".to_owned());
        sent
    };

    let mut landed = DraftForm::open(&row);
    let sent = typing(&mut landed);
    landed.saved(sent);
    landed.refresh(&admitted);
    assert_eq!(landed.fields().get(DraftField::Acceptance), "One\nThree");
    assert_eq!(
        landed.fields().get(DraftField::Description),
        "Typed after Save"
    );

    let mut refused = DraftForm::open(&row);
    typing(&mut refused);
    refused.refresh(&admitted);
    assert_eq!(
        refused.fields().get(DraftField::Acceptance),
        "One\n\n  Three "
    );
    assert_eq!(
        refused.fields().get(DraftField::Description),
        "Typed after Save"
    );

    for form in [&landed, &refused] {
        let (request, expected) = save(form);
        assert!(request.acceptance.is_empty());
        assert_eq!(request.description.as_deref(), Some("Typed after Save"));
        assert_eq!(expected, witness(&row, 5));
    }
}

#[test]
fn another_task_reopens_the_form_on_its_own_latest_content() {
    let first = draft(1);
    let second = draft(2);
    let mut form = DraftForm::open(&first);
    form.edit(DraftField::Title, "Typed into the first".to_owned());

    form.refresh(&second);
    assert_eq!(form.fields(), DraftForm::open(&second).fields());

    let latest = revised(&first, 7, |row| {
        row.presentation_title = "First, revised".to_owned();
    });
    form.edit(DraftField::Description, "Typed into the second".to_owned());
    form.refresh(&latest);
    assert_eq!(form.fields().get(DraftField::Title), "First, revised");
    assert_eq!(form.fields().get(DraftField::Description), "Before");
    assert!(form.revision().is_err());
}

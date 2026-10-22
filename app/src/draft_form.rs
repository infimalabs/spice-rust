//! An open Draft editor's own fields.
//!
//! What the operator types belongs to the editor: a live projection of the
//! Draft, whatever refreshed it, never rewrites a field they changed. The
//! editor revises the latest admitted revision it has seen and witnesses it,
//! so a Save sends exactly the fields the operator changed against that
//! revision, and a refused Save keeps every typed field for the retry.

use wire::intent::{BoardTask, TaskEdit};
use wire::pitch::{PITCH_CHARS, PitchRefusal};
use wire::{TaskIntent, TaskIntentEdit, Witness};

use crate::client_action::DraftRequest;

/// One editable control of an open Draft.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DraftField {
    Title,
    /// One plain sentence saying why the work matters.
    Pitch,
    Project,
    /// The Kind control: empty for ordinary work, else the intent's name.
    Intent,
    /// Comma-separated phases.
    Flow,
    Description,
    /// One criterion, or one Design question, per line.
    Acceptance,
}

impl DraftField {
    const ALL: [Self; 7] = [
        Self::Title,
        Self::Pitch,
        Self::Project,
        Self::Intent,
        Self::Flow,
        Self::Description,
        Self::Acceptance,
    ];
}

/// The text each control of an open Draft holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DraftFields {
    title: String,
    pitch: String,
    project: String,
    intent: String,
    flow: String,
    description: String,
    acceptance: String,
}

impl DraftFields {
    fn admitted(row: &BoardTask) -> Self {
        let statement = &row.statement.body;
        Self {
            title: row.presentation_title.clone(),
            pitch: statement.pitch.clone(),
            project: statement.project.clone(),
            intent: statement
                .intent
                .map_or_else(String::new, |intent| intent.as_str().to_owned()),
            flow: flow(row),
            description: statement.description.clone().unwrap_or_default(),
            acceptance: statement.acceptance.join("\n"),
        }
    }

    pub(crate) fn get(&self, field: DraftField) -> &str {
        match field {
            DraftField::Title => &self.title,
            DraftField::Pitch => &self.pitch,
            DraftField::Project => &self.project,
            DraftField::Intent => &self.intent,
            DraftField::Flow => &self.flow,
            DraftField::Description => &self.description,
            DraftField::Acceptance => &self.acceptance,
        }
    }

    fn slot(&mut self, field: DraftField) -> &mut String {
        match field {
            DraftField::Title => &mut self.title,
            DraftField::Pitch => &mut self.pitch,
            DraftField::Project => &mut self.project,
            DraftField::Intent => &mut self.intent,
            DraftField::Flow => &mut self.flow,
            DraftField::Description => &mut self.description,
            DraftField::Acceptance => &mut self.acceptance,
        }
    }

    /// The intent the Kind control names.
    pub(crate) fn intent(&self) -> Result<Option<TaskIntent>, String> {
        if self.intent.is_empty() {
            return Ok(None);
        }
        self.intent.parse().map(Some)
    }
}

/// One mounted Draft editor: the admitted revision it revises and the fields
/// the operator is editing against it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DraftForm {
    basis: BoardTask,
    fields: DraftFields,
    /// The fields of the operator's last landed Save, until the revision it
    /// admitted arrives.
    saved: Option<DraftFields>,
}

impl DraftForm {
    /// An editor opened on `row`'s admitted content.
    pub(crate) fn open(row: &BoardTask) -> Self {
        Self {
            basis: row.clone(),
            fields: DraftFields::admitted(row),
            saved: None,
        }
    }

    pub(crate) fn fields(&self) -> &DraftFields {
        &self.fields
    }

    /// The operator typed `value` into `field`.
    pub(crate) fn edit(&mut self, field: DraftField, value: String) {
        *self.fields.slot(field) = value;
    }

    /// The operator's Save of `fields` landed.
    pub(crate) fn saved(&mut self, fields: DraftFields) {
        self.saved = Some(fields);
    }

    /// A projection of the Draft rendered.
    ///
    /// Another task reopens the editor on it, and a revision no newer than
    /// the basis changes nothing. A newer revision becomes the basis: each
    /// field the operator has not changed from the old basis, or still holds
    /// as their landed Save sent it, takes the newly admitted value, and every
    /// other field keeps their text.
    pub(crate) fn refresh(&mut self, row: &BoardTask) {
        if row.statement.body.born != self.basis.statement.body.born {
            *self = Self::open(row);
            return;
        }
        if !newer(row, &self.basis) {
            return;
        }
        let was = DraftFields::admitted(&self.basis);
        let now = DraftFields::admitted(row);
        let sent = self.saved.take();
        for field in DraftField::ALL {
            let typed = self.fields.get(field);
            if typed == was.get(field) || sent.as_ref().is_some_and(|sent| sent.get(field) == typed)
            {
                now.get(field).clone_into(self.fields.slot(field));
            }
        }
        self.basis = row.clone();
    }

    /// The Save request for every field that differs from the basis,
    /// witnessing the basis revision.
    pub(crate) fn revision(&self) -> Result<DraftRequest, String> {
        let statement = &self.basis.statement.body;
        let moved = |field: DraftField, current: &str| {
            let value = self.fields.get(field).trim();
            (value != current.trim()).then(|| value.to_owned())
        };
        let acceptance = criteria(&self.fields.acceptance);
        let intent = self.fields.intent()?;
        let pitch = admitted_pitch(&self.fields.pitch)?;
        let request = TaskEdit {
            task: statement.handle.clone(),
            title: moved(DraftField::Title, &self.basis.presentation_title),
            pitch: (pitch != statement.pitch).then_some(pitch),
            description: moved(
                DraftField::Description,
                statement.description.as_deref().unwrap_or_default(),
            ),
            acceptance: if acceptance == statement.acceptance {
                Vec::new()
            } else {
                acceptance
            },
            requirements: None,
            intent: (intent != statement.intent).then_some(match intent {
                Some(intent) => TaskIntentEdit::Set { intent },
                None => TaskIntentEdit::Clear {},
            }),
            priority: None,
            project: moved(DraftField::Project, &statement.project),
            flow: moved(DraftField::Flow, &flow(&self.basis)),
        };
        if request.title.is_none()
            && request.pitch.is_none()
            && request.description.is_none()
            && request.acceptance.is_empty()
            && request.intent.is_none()
            && request.project.is_none()
            && request.flow.is_none()
        {
            return Err("Nothing changed in this Draft.".to_owned());
        }
        Ok(DraftRequest::Edit {
            request,
            expected: vec![Witness::Task {
                task: statement.born.clone(),
                head: self.basis.statement.position,
            }],
        })
    }
}

/// Non-empty trimmed lines, one criterion each.
pub(crate) fn criteria(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The trimmed pitch, or the sentence the editor shows instead of sending a
/// pitch the appliance would refuse.
pub(crate) fn admitted_pitch(text: &str) -> Result<String, String> {
    match wire::pitch::admit(text) {
        Ok(()) => Ok(text.trim().to_owned()),
        Err(PitchRefusal::Empty) => {
            Err("A Draft needs one plain sentence saying why it matters.".to_owned())
        }
        Err(PitchRefusal::Multiline) => Err("A pitch stays on one line.".to_owned()),
        Err(PitchRefusal::TooLong { chars }) => Err(format!(
            "A pitch stays within {PITCH_CHARS} characters; this one has {chars}."
        )),
    }
}

fn flow(row: &BoardTask) -> String {
    row.workflow
        .body
        .flow
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

/// Whether `row` carries a later revision of the editable content than
/// `basis`: its statement, or the workflow that carries the flow.
fn newer(row: &BoardTask, basis: &BoardTask) -> bool {
    row.statement.position > basis.statement.position
        || row.workflow.position > basis.workflow.position
}

#[cfg(test)]
mod tests;

//! A working group's browser-local panel state and its team-panel controls:
//! TURBO, the speech slider, and the shared brake slider. Group state lives
//! beside the team controls because a group's speech raises the speech of the
//! team planes its claimants draw in.

use super::*;
use crate::client_action::GroupChange;
use crate::speech_queue::{LaneSpeech, SpeechMode, group_speech};
use wire::rooms::{
    RoomAuthorityStamp, WorkingGroupFast, WorkingGroupStance, WorkingGroupStanceSet,
};

const GROUP_SPEECH_STORAGE: &str = "spice:group-speech:v1";

/// One working group's browser-local panel state.
#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct GroupControl {
    speech: SpeechMode,
    /// Group stance requested and not yet answered by the resident.
    pending_stance: Option<WorkingGroupStance>,
    /// Group fast-mode flag requested and not yet answered by the resident.
    pending_fast: Option<bool>,
}

pub(crate) enum GroupControlAction {
    SetSpeech {
        group: RoomAuthorityStamp,
        speech: SpeechMode,
    },
    RequestStance {
        group: RoomAuthorityStamp,
        stance: WorkingGroupStance,
    },
    /// The resident answered the request for `stance`.
    SettleStance {
        group: RoomAuthorityStamp,
        stance: WorkingGroupStance,
    },
    RequestFast {
        group: RoomAuthorityStamp,
        fast_mode: bool,
    },
    /// The resident answered the request for `fast_mode`.
    SettleFast {
        group: RoomAuthorityStamp,
        fast_mode: bool,
    },
}

/// Every group speech mode this browser stored.
pub(super) fn restore(storage: &web_sys::Storage) -> BTreeMap<RoomAuthorityStamp, GroupControl> {
    storage
        .get_item(GROUP_SPEECH_STORAGE)
        .ok()
        .flatten()
        .and_then(|document| serde_json::from_str::<Vec<(RoomAuthorityStamp, u8)>>(&document).ok())
        .into_iter()
        .flatten()
        .filter_map(|(group, value)| {
            let speech = SpeechMode::from_storage(value)?;
            Some((
                group,
                GroupControl {
                    speech,
                    ..GroupControl::default()
                },
            ))
        })
        .collect()
}

/// One drawn team plane's controls and the speech its lanes are voiced at.
pub(in crate::browser::watchband) struct Plane {
    pub(in crate::browser::watchband) control: TeamControl,
    pub(in crate::browser::watchband) speech: LaneSpeech,
}

impl TeamControls {
    pub(super) fn reduce_group(&mut self, action: GroupControlAction) {
        match action {
            GroupControlAction::SetSpeech { group, speech } => {
                self.groups.entry(group).or_default().speech = speech;
                self.store_group_speech();
            }
            GroupControlAction::RequestStance { group, stance } => {
                self.groups.entry(group).or_default().pending_stance = Some(stance);
            }
            GroupControlAction::SettleStance { group, stance } => {
                if let Some(control) = self.groups.get_mut(&group)
                    && control.pending_stance == Some(stance)
                {
                    control.pending_stance = None;
                }
            }
            GroupControlAction::RequestFast { group, fast_mode } => {
                self.groups.entry(group).or_default().pending_fast = Some(fast_mode);
            }
            GroupControlAction::SettleFast { group, fast_mode } => {
                if let Some(control) = self.groups.get_mut(&group)
                    && control.pending_fast == Some(fast_mode)
                {
                    control.pending_fast = None;
                }
            }
        }
    }

    /// Only a voiced group is stored: an absent group reads as Quiet.
    fn store_group_speech(&self) {
        let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        else {
            return;
        };
        let rows = self
            .groups
            .iter()
            .filter(|(_, control)| control.speech != SpeechMode::Quiet)
            .map(|(group, control)| (group, control.speech.storage_value()))
            .collect::<Vec<_>>();
        if let Ok(document) = serde_json::to_string(&rows) {
            let _ = storage.set_item(GROUP_SPEECH_STORAGE, &document);
        }
    }

    /// One team plane's controls, its lanes voiced at the louder of the
    /// team's mode and every covering group's.
    pub(in crate::browser::watchband) fn plane(
        &self,
        team: &crate::runtime::LaneRow,
        runtime: &crate::AppRuntime,
    ) -> Plane {
        let control = self.current(team.lane, team.subscription);
        let speech = LaneSpeech::new(
            control.speech,
            team.members.iter().map(|member| member.lane),
            &self.coverage(runtime),
        );
        Plane { control, speech }
    }

    /// Each lane's loudest covering group mode. Nothing is read while every
    /// group is Quiet.
    fn coverage(&self, runtime: &crate::AppRuntime) -> BTreeMap<CanonicalUuid, SpeechMode> {
        let voiced = || {
            self.groups
                .iter()
                .filter(|(_, control)| control.speech != SpeechMode::Quiet)
        };
        let (Some(_), Some(board)) = (voiced().next(), runtime.board()) else {
            return BTreeMap::new();
        };
        let claimants = crate::roster::claimants(runtime.room_projection().partition(), board);
        group_speech(
            voiced().filter_map(|(group, control)| Some((control.speech, claimants.get(group)?))),
        )
    }

    /// Each team holding one of `lanes`, with whether it admits automatic
    /// speech were `group` voiced at `speech`.
    fn admissions(
        &self,
        runtime: &crate::AppRuntime,
        group: &RoomAuthorityStamp,
        speech: SpeechMode,
        lanes: &BTreeSet<CanonicalUuid>,
    ) -> Vec<(String, bool)> {
        let teams = runtime.lanes().map_or_else(Vec::new, |view| {
            view.lanes
                .iter()
                .filter(|row| {
                    row.teamed()
                        && row
                            .members
                            .iter()
                            .any(|member| lanes.contains(&member.lane))
                })
                .collect()
        });
        if teams.is_empty() {
            return Vec::new();
        }
        let mut next = self.clone();
        next.groups.entry(group.clone()).or_default().speech = speech;
        let coverage = next.coverage(runtime);
        teams
            .into_iter()
            .map(|row| {
                let team = next.current(row.lane, row.subscription).speech;
                let speech = LaneSpeech::new(
                    team,
                    row.members.iter().map(|member| member.lane),
                    &coverage,
                );
                (row.lane.to_string(), speech.admits(team))
            })
            .collect()
    }
}

/// Everything one working group's panel controls read.
pub(in crate::browser) struct GroupPanel<'a> {
    pub(in crate::browser) runtime: &'a crate::AppRuntime,
    pub(in crate::browser) controls: &'a UseReducerHandle<TeamControls>,
    pub(in crate::browser) set_automatic_speech: &'a Callback<(String, bool)>,
    pub(in crate::browser) sender: &'a SenderHandle,
    /// Shows a refused request's reason.
    pub(in crate::browser) report: Callback<String>,
    pub(in crate::browser) group: &'a RoomAuthorityStamp,
    /// The lanes holding live claims on Design-associated work.
    pub(in crate::browser) claimants: &'a BTreeSet<CanonicalUuid>,
    /// Their display names, in the same order.
    pub(in crate::browser) names: &'a [String],
}

/// A group panel's TURBO, the stops its two sliders show, and the sliders.
pub(in crate::browser) struct GroupControls {
    pub(in crate::browser) turbo: Html,
    pub(in crate::browser) speech: &'static str,
    pub(in crate::browser) stance: &'static str,
    pub(in crate::browser) sliders: Html,
}

impl GroupPanel<'_> {
    pub(in crate::browser) fn render(&self) -> GroupControls {
        let control = self
            .controls
            .groups
            .get(self.group)
            .cloned()
            .unwrap_or_default();
        let stance = displayed_stance(self.runtime, self.controls, self.group);
        GroupControls {
            turbo: self.turbo(control.pending_fast),
            speech: control.speech.label(),
            stance: stance_label(stance),
            sliders: html! {
                <>
                    {self.speech_slider(control.speech)}
                    {stance_slider(self.runtime, self.controls, self.sender, &self.report, self.group)}
                </>
            },
        }
    }

    /// The group's fast-mode toggle. Fast mode is additive: switching it off
    /// leaves fast every lane another level keeps fast. A press shows its
    /// request until the resident answers that command.
    fn turbo(&self, pending: Option<bool>) -> Html {
        let partition = self.runtime.room_projection().partition();
        let revision = partition.revision;
        let engaged = pending.unwrap_or_else(|| partition.is_fast(self.group));
        let standing = partition
            .groups
            .iter()
            .any(|group| group.group == *self.group);
        let can_configure = standing && self.runtime.offers(wire::CommandKind::WorkingGroupFast);
        let title = match (engaged, self.names.join(", ")) {
            (true, lanes) if lanes.is_empty() => {
                "Turbo: no lane works on this group's Design-associated tasks".to_owned()
            }
            (false, lanes) if lanes.is_empty() => {
                "Turbo off: no lane works on this group's Design-associated tasks".to_owned()
            }
            (true, lanes) => format!("Turbo: {lanes} run in fast mode"),
            (false, lanes) => format!("Turbo off: press to run {lanes} in fast mode"),
        };
        let onclick = {
            let controls = self.controls.clone();
            let sender = self.sender.clone();
            let report = self.report.clone();
            let group = self.group.clone();
            Callback::from(move |_: MouseEvent| {
                let fast_mode = !engaged;
                let respond = {
                    let (controls, report, group) =
                        (controls.clone(), report.clone(), group.clone());
                    Rc::new(
                        move |result: Result<wire::intent::WorkingGroupChange, String>| {
                            controls.dispatch(TeamControlAction::Group(
                                GroupControlAction::SettleFast {
                                    group: group.clone(),
                                    fast_mode,
                                },
                            ));
                            if let Err(error) = result {
                                report.emit(error);
                            }
                        },
                    )
                };
                if admit_action(
                    &sender,
                    ClientAction::WorkingGroupChange {
                        change: GroupChange::Fast(WorkingGroupFast {
                            group: group.clone(),
                            fast_mode,
                        }),
                        revision,
                        respond,
                    },
                ) {
                    controls.dispatch(TeamControlAction::Group(GroupControlAction::RequestFast {
                        group: group.clone(),
                        fast_mode,
                    }));
                }
            })
        };
        html! {
            <crate::browser::view::chrome::TurboButton
                class={classes!("team-turbo")}
                {engaged}
                disabled={!can_configure}
                {title}
                {onclick}
            />
        }
    }

    /// The group's speech mode, covering each Design-associated worker with
    /// the team rules and applied in the input turn that selects it.
    fn speech_slider(&self, speech: SpeechMode) -> Html {
        let admissions = SpeechMode::ALL.map(|mode| {
            self.controls
                .admissions(self.runtime, self.group, mode, self.claimants)
        });
        let controls = self.controls.clone();
        let set_automatic_speech = self.set_automatic_speech.clone();
        let group = self.group.clone();
        let oninput = Callback::from(move |event: InputEvent| {
            let input: HtmlInputElement = event.target_unchecked_into();
            let speech = SpeechMode::at(input.value().parse::<usize>().unwrap_or(1));
            for admission in &admissions[speech.index()] {
                set_automatic_speech.emit(admission.clone());
            }
            controls.dispatch(TeamControlAction::Group(GroupControlAction::SetSpeech {
                group: group.clone(),
                speech,
            }));
        });
        html! {
            <label
                class="team-slider team-speech"
                data-stop={speech.index().to_string()}
            >
                <input
                    type="range"
                    min="0"
                    max="2"
                    step="1"
                    value={speech.index().to_string()}
                    aria-label={format!("Group speech mode: {}", speech.label())}
                    title="Voices the lanes working on this group's Design-associated tasks"
                    {oninput}
                />
                {slider_words(SpeechMode::ALL.map(SpeechMode::label), speech.index())}
            </label>
        }
    }
}

/// The stance a group's controls show: the one this browser requested and the
/// resident has not answered, else the partition's.
fn displayed_stance(
    runtime: &crate::AppRuntime,
    controls: &TeamControls,
    group: &RoomAuthorityStamp,
) -> WorkingGroupStance {
    controls
        .groups
        .get(group)
        .and_then(|control| control.pending_stance)
        .unwrap_or_else(|| {
            runtime
                .room_projection()
                .partition()
                .groups
                .iter()
                .find(|standing| standing.group == *group)
                .map_or(WorkingGroupStance::Paused, |standing| standing.stance)
        })
}

/// One group's shared brake, as its console panel and its catalog card both
/// draw it. Paused keeps its Designs passive; Running makes them active. A
/// move shows its request until the resident answers, and `report` shows a
/// refusal or the shelving it still owes.
pub(in crate::browser) fn stance_slider(
    runtime: &crate::AppRuntime,
    controls: &UseReducerHandle<TeamControls>,
    sender: &SenderHandle,
    report: &Callback<String>,
    group: &RoomAuthorityStamp,
) -> Html {
    let stance = displayed_stance(runtime, controls, group);
    let partition = runtime.room_projection().partition();
    let revision = partition.revision;
    let standing = partition
        .groups
        .iter()
        .any(|standing| standing.group == *group);
    let can_configure = standing && runtime.offers(wire::CommandKind::WorkingGroupStance);
    let controls = controls.clone();
    let sender = sender.clone();
    let report = report.clone();
    let group = group.clone();
    let oninput = Callback::from(move |event: InputEvent| {
        let input: HtmlInputElement = event.target_unchecked_into();
        let index = input.value().parse::<usize>().unwrap_or(0);
        let stance = WorkingGroupStance::ALL[index.min(WorkingGroupStance::ALL.len() - 1)];
        let respond = {
            let (controls, report, group) = (controls.clone(), report.clone(), group.clone());
            Rc::new(
                move |result: Result<wire::intent::WorkingGroupChange, String>| {
                    controls.dispatch(TeamControlAction::Group(GroupControlAction::SettleStance {
                        group: group.clone(),
                        stance,
                    }));
                    match result {
                        Err(error) => report.emit(error),
                        Ok(change) if !change.owed.is_empty() => {
                            let owed = change
                                .owed
                                .iter()
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join(", ");
                            report.emit(format!(
                                "Shelving is still owed for {owed}; set the group Paused again."
                            ));
                        }
                        Ok(_) => {}
                    }
                },
            )
        };
        if admit_action(
            &sender,
            ClientAction::WorkingGroupChange {
                change: GroupChange::Stance(WorkingGroupStanceSet {
                    group: group.clone(),
                    stance,
                }),
                revision,
                respond,
            },
        ) {
            controls.dispatch(TeamControlAction::Group(
                GroupControlAction::RequestStance {
                    group: group.clone(),
                    stance,
                },
            ));
        }
    });
    html! {
        <label
            class="team-slider team-lifetime"
            data-stop={if stance.is_active() { "2" } else { "0" }}
        >
            <input
                type="range"
                min="0"
                max="1"
                step="1"
                value={stance.index().to_string()}
                disabled={!can_configure}
                aria-label={format!("Group brake: {}", stance_label(stance))}
                title="Paused keeps this group's Designs passive; Running makes them active"
                {oninput}
            />
            {slider_words(WorkingGroupStance::ALL.map(stance_label), stance.index())}
        </label>
    }
}

const fn stance_label(stance: WorkingGroupStance) -> &'static str {
    match stance {
        WorkingGroupStance::Paused => "Paused",
        WorkingGroupStance::Running => "Running",
    }
}

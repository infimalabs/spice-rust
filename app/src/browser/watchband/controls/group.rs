//! A working group's browser-local panel state and its team-panel controls:
//! TURBO, the speech slider, and the mock stance slider. Group state lives
//! beside the team controls because a group's speech raises the speech of the
//! team planes its claimants draw in.

use super::*;
use crate::client_action::GroupChange;
use crate::speech_queue::{LaneSpeech, SpeechMode, group_speech};
use wire::rooms::{RoomAuthorityStamp, WorkingGroupFast};

const GROUP_SPEECH_STORAGE: &str = "spice:group-speech:v1";

/// The second group slider's stops. It decides nothing yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum GroupStance {
    #[default]
    Hold,
    Explore,
    Converge,
}

impl GroupStance {
    const ALL: [Self; 3] = [Self::Hold, Self::Explore, Self::Converge];

    const fn label(self) -> &'static str {
        match self {
            Self::Hold => "Hold",
            Self::Explore => "Explore",
            Self::Converge => "Converge",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Hold => 0,
            Self::Explore => 1,
            Self::Converge => 2,
        }
    }
}

/// One working group's browser-local panel state.
#[derive(Clone, Default, PartialEq, Eq)]
pub(super) struct GroupControl {
    speech: SpeechMode,
    stance: GroupStance,
    /// Group fast-mode flag requested and not yet answered by the resident.
    pending_fast: Option<bool>,
}

pub(crate) enum GroupControlAction {
    SetSpeech {
        group: RoomAuthorityStamp,
        speech: SpeechMode,
    },
    SetStance {
        group: RoomAuthorityStamp,
        stance: GroupStance,
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
            GroupControlAction::SetStance { group, stance } => {
                self.groups.entry(group).or_default().stance = stance;
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
    /// The lanes holding a live claim on the group's designs.
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
        GroupControls {
            turbo: self.turbo(control.pending_fast),
            speech: control.speech.label(),
            stance: control.stance.label(),
            sliders: html! {
                <>
                    {self.speech_slider(control.speech)}
                    {self.stance_slider(control.stance)}
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
                "Turbo: no lane holds a claim on this group's designs".to_owned()
            }
            (false, lanes) if lanes.is_empty() => {
                "Turbo off: no lane holds a claim on this group's designs".to_owned()
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

    /// The group's speech mode, covering each claimant with the team rules
    /// and applied in the input turn that selects it.
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
                    title="Voices the lanes holding a claim on this group's designs"
                    {oninput}
                />
                {slider_words(SpeechMode::ALL.map(SpeechMode::label), speech.index())}
            </label>
        }
    }

    /// A mock until its purpose is decided: it sends nothing and changes no
    /// authority.
    fn stance_slider(&self, stance: GroupStance) -> Html {
        let controls = self.controls.clone();
        let group = self.group.clone();
        let oninput = Callback::from(move |event: InputEvent| {
            let input: HtmlInputElement = event.target_unchecked_into();
            let index = input.value().parse::<usize>().unwrap_or(0);
            controls.dispatch(TeamControlAction::Group(GroupControlAction::SetStance {
                group: group.clone(),
                stance: GroupStance::ALL[index.min(GroupStance::ALL.len() - 1)],
            }));
        });
        html! {
            <label
                class="team-slider team-lifetime"
                data-stop={stance.index().to_string()}
            >
                <input
                    type="range"
                    min="0"
                    max="2"
                    step="1"
                    value={stance.index().to_string()}
                    aria-label={format!("Group stance, which does nothing yet: {}", stance.label())}
                    title="Hold, Explore, or Converge: this slider does nothing yet"
                    {oninput}
                />
                {slider_words(GroupStance::ALL.map(GroupStance::label), stance.index())}
            </label>
        }
    }
}

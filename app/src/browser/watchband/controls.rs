use super::*;
use crate::speech_queue::SpeechMode;

mod frame;
mod group;
pub(in crate::browser) use frame::{ControlsFrame, SendSlot, holds_text};
use group::GroupControlAction;
pub(in crate::browser) use group::GroupPanel;
pub(super) use group::Plane;

const TEAM_SPEECH_STORAGE: &str = "spice:team-speech:v1";
const HIDDEN_TEAMS_STORAGE: &str = "spice:hidden-teams:v1";

/// One policy shared by every agent member in the visual lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TeamLife {
    Steer,
    Drive,
    Drain,
}

impl TeamLife {
    const ALL: [Self; 3] = [Self::Steer, Self::Drive, Self::Drain];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Steer => "Steer",
            Self::Drive => "Drive",
            Self::Drain => "Drain",
        }
    }

    const fn help(self) -> &'static str {
        match self {
            Self::Steer => "Operator direction and explicit task capture only",
            Self::Drive => "Automatically share tasks created for this team",
            Self::Drain => "Boundary dissolved: see all assignable work",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Steer => 0,
            Self::Drive => 1,
            Self::Drain => 2,
        }
    }

    const fn wire(self) -> wire::intent::TeamLife {
        match self {
            Self::Steer => wire::intent::TeamLife::Steer,
            Self::Drive => wire::intent::TeamLife::Drive,
            Self::Drain => wire::intent::TeamLife::Drain,
        }
    }
}

impl From<wire::intent::TeamLife> for TeamLife {
    fn from(value: wire::intent::TeamLife) -> Self {
        match value {
            wire::intent::TeamLife::Steer => Self::Steer,
            wire::intent::TeamLife::Drive => Self::Drive,
            wire::intent::TeamLife::Drain => Self::Drain,
        }
    }
}

/// The control that opened a team's arrangement, where dismissal returns focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::browser) enum ArrangementOpener {
    Lights,
    Menu,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct TeamControl {
    pub(super) arrangement: Option<ArrangementOpener>,
    pub(super) speech: SpeechMode,
    pub(super) hidden: bool,
    pub(super) life: TeamLife,
    preview_life: Option<TeamLife>,
    pub(super) pending_life: Option<TeamLife>,
    /// Team fast-mode flag requested and not yet answered by the resident.
    pending_fast: Option<bool>,
    authoritative_life: TeamLife,
    issue: Option<String>,
}

impl Default for TeamControl {
    fn default() -> Self {
        Self {
            arrangement: None,
            speech: SpeechMode::Quiet,
            hidden: false,
            life: TeamLife::Steer,
            preview_life: None,
            pending_life: None,
            pending_fast: None,
            authoritative_life: TeamLife::Steer,
            issue: None,
        }
    }
}

/// Per-team and per-working-group control state retained across Mosaic
/// redraws.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct TeamControls {
    teams: BTreeMap<CanonicalUuid, TeamControl>,
    groups: BTreeMap<wire::rooms::RoomAuthorityStamp, group::GroupControl>,
}

/// The selections one in-flight team configuration command carries, which
/// only that command's own answer settles.
#[derive(Clone, Copy)]
pub(crate) struct TeamRequest {
    pub(crate) team: CanonicalUuid,
    pub(crate) life: Option<TeamLife>,
    pub(crate) fast_mode: Option<bool>,
}

pub(crate) enum TeamControlAction {
    ToggleArrangement {
        team: CanonicalUuid,
        opener: ArrangementOpener,
    },
    HideTeam {
        team: CanonicalUuid,
    },
    ShowTeam {
        team: CanonicalUuid,
    },
    CloseArrangement,
    SetSpeech {
        team: CanonicalUuid,
        authoritative_life: wire::intent::TeamLife,
        speech: SpeechMode,
    },
    PreviewLife {
        team: CanonicalUuid,
        authoritative_life: wire::intent::TeamLife,
        life: TeamLife,
    },
    CancelLifePreview {
        team: CanonicalUuid,
    },
    RequestLife {
        team: CanonicalUuid,
        authoritative_life: wire::intent::TeamLife,
        life: TeamLife,
    },
    RequestFast {
        team: CanonicalUuid,
        authoritative_life: wire::intent::TeamLife,
        fast_mode: bool,
    },
    SyncLanes {
        answer: wire::intent::LaneAnswer,
        settle: Option<TeamRequest>,
    },
    SettleIssue {
        request: TeamRequest,
        issue: String,
    },
    /// The resident answered this request, but newer lane authority already
    /// arrived: settle the request to that authority, reporting the team issue
    /// only when that authority rolls the control back from what was asked.
    SettleSuperseded {
        request: TeamRequest,
        /// The team's held TURBO flag, absent when the team is no longer held.
        fast_mode: Option<bool>,
    },
    Group(GroupControlAction),
}

impl Reducible for TeamControls {
    type Action = TeamControlAction;

    fn reduce(self: Rc<Self>, action: Self::Action) -> Rc<Self> {
        let mut next = (*self).clone();
        match action {
            TeamControlAction::ToggleArrangement { team, opener } => {
                next.toggle_arrangement(team, opener);
            }
            TeamControlAction::HideTeam { team } => next.set_hidden(team, true),
            TeamControlAction::ShowTeam { team } => next.set_hidden(team, false),
            TeamControlAction::CloseArrangement => next.close_arrangements(),
            TeamControlAction::SetSpeech {
                team,
                authoritative_life,
                speech,
            } => next.set_speech(team, authoritative_life, speech),
            TeamControlAction::PreviewLife {
                team,
                authoritative_life,
                life,
            } => next.preview_life(team, authoritative_life, life),
            TeamControlAction::CancelLifePreview { team } => {
                next.cancel_life_preview(team);
            }
            TeamControlAction::RequestLife {
                team,
                authoritative_life,
                life,
            } => next.request_life(team, authoritative_life, life),
            TeamControlAction::RequestFast {
                team,
                authoritative_life,
                fast_mode,
            } => next.request_fast(team, authoritative_life, fast_mode),
            TeamControlAction::SyncLanes { answer, settle } => {
                next.sync_lanes(&answer, settle);
            }
            TeamControlAction::SettleIssue { request, issue } => {
                next.settle_issue(request, issue);
            }
            TeamControlAction::SettleSuperseded { request, fast_mode } => {
                next.settle_superseded(request, fast_mode);
            }
            TeamControlAction::Group(action) => next.reduce_group(action),
        }
        Rc::new(next)
    }
}

impl TeamControls {
    pub(in crate::browser) fn restore() -> Self {
        let mut controls = Self::default();
        let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        else {
            return controls;
        };
        controls.groups = group::restore(&storage);
        if let Some(document) = storage.get_item(TEAM_SPEECH_STORAGE).ok().flatten()
            && let Ok(rows) = serde_json::from_str::<Vec<(String, u8)>>(&document)
        {
            for (team, value) in rows {
                let (Ok(team), Some(speech)) = (team.parse(), SpeechMode::from_storage(value))
                else {
                    continue;
                };
                controls.teams.entry(team).or_default().speech = speech;
            }
        }
        if let Some(document) = storage.get_item(HIDDEN_TEAMS_STORAGE).ok().flatten()
            && let Ok(teams) = serde_json::from_str::<Vec<String>>(&document)
        {
            for team in teams.into_iter().filter_map(|team| team.parse().ok()) {
                controls.teams.entry(team).or_default().hidden = true;
            }
        }
        controls
    }

    pub(in crate::browser) fn is_hidden(&self, team: CanonicalUuid) -> bool {
        self.teams.get(&team).is_some_and(|control| control.hidden)
    }

    fn set_hidden(&mut self, team: CanonicalUuid, hidden: bool) {
        let control = self.teams.entry(team).or_default();
        control.hidden = hidden;
        control.arrangement = None;
        self.store_hidden();
    }

    fn store_hidden(&self) {
        let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        else {
            return;
        };
        let teams = self
            .teams
            .iter()
            .filter(|(_, control)| control.hidden)
            .map(|(team, _)| team.to_string())
            .collect::<Vec<_>>();
        if teams.is_empty() {
            let _ = storage.remove_item(HIDDEN_TEAMS_STORAGE);
        } else if let Ok(document) = serde_json::to_string(&teams) {
            let _ = storage.set_item(HIDDEN_TEAMS_STORAGE, &document);
        }
    }

    pub(super) fn current(
        &self,
        team: CanonicalUuid,
        authoritative_life: wire::intent::TeamLife,
    ) -> TeamControl {
        self.teams
            .get(&team)
            .cloned()
            .unwrap_or_else(|| TeamControl {
                life: authoritative_life.into(),
                authoritative_life: authoritative_life.into(),
                ..TeamControl::default()
            })
    }

    /// The open arrangement's team and the control that opened it.
    pub(in crate::browser) fn open_arrangement(
        &self,
    ) -> Option<(CanonicalUuid, ArrangementOpener)> {
        self.teams
            .iter()
            .find_map(|(team, state)| state.arrangement.map(|opener| (*team, opener)))
    }

    fn toggle_arrangement(&mut self, team: CanonicalUuid, opener: ArrangementOpener) {
        let open = !self
            .teams
            .get(&team)
            .is_some_and(|state| state.arrangement.is_some());
        self.close_arrangements();
        self.teams.entry(team).or_default().arrangement = open.then_some(opener);
    }

    fn close_arrangements(&mut self) {
        for state in self.teams.values_mut() {
            state.arrangement = None;
        }
    }

    fn set_speech(
        &mut self,
        team: CanonicalUuid,
        authoritative_life: wire::intent::TeamLife,
        speech: SpeechMode,
    ) {
        self.teams
            .entry(team)
            .or_insert_with(|| TeamControl {
                life: authoritative_life.into(),
                authoritative_life: authoritative_life.into(),
                ..TeamControl::default()
            })
            .speech = speech;
        self.store_speech();
    }

    fn store_speech(&self) {
        let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        else {
            return;
        };
        let rows = self
            .teams
            .iter()
            .map(|(team, control)| (team.to_string(), control.speech.storage_value()))
            .collect::<Vec<_>>();
        if let Ok(document) = serde_json::to_string(&rows) {
            let _ = storage.set_item(TEAM_SPEECH_STORAGE, &document);
        }
    }

    /// Each team's last policy refusal, which the next send clears. A send
    /// that lands needs no report: the control holds its new stop.
    pub(super) fn refusals(&self) -> impl Iterator<Item = (CanonicalUuid, &str)> {
        self.teams
            .iter()
            .filter_map(|(team, control)| Some((*team, control.issue.as_deref()?)))
    }

    fn request_life(
        &mut self,
        team: CanonicalUuid,
        authoritative_life: wire::intent::TeamLife,
        life: TeamLife,
    ) {
        let state = self.teams.entry(team).or_insert_with(|| TeamControl {
            life: authoritative_life.into(),
            authoritative_life: authoritative_life.into(),
            ..TeamControl::default()
        });
        state.life = life;
        state.preview_life = None;
        state.pending_life = Some(life);
        state.issue = None;
    }

    fn request_fast(
        &mut self,
        team: CanonicalUuid,
        authoritative_life: wire::intent::TeamLife,
        fast_mode: bool,
    ) {
        let state = self.teams.entry(team).or_insert_with(|| TeamControl {
            life: authoritative_life.into(),
            authoritative_life: authoritative_life.into(),
            ..TeamControl::default()
        });
        state.pending_fast = Some(fast_mode);
        state.issue = None;
    }

    fn preview_life(
        &mut self,
        team: CanonicalUuid,
        authoritative_life: wire::intent::TeamLife,
        life: TeamLife,
    ) {
        let state = self.teams.entry(team).or_insert_with(|| TeamControl {
            life: authoritative_life.into(),
            authoritative_life: authoritative_life.into(),
            ..TeamControl::default()
        });
        if state.pending_life.is_none() {
            state.preview_life = (life != state.authoritative_life).then_some(life);
        }
    }

    fn cancel_life_preview(&mut self, team: CanonicalUuid) {
        let Some(state) = self.teams.get_mut(&team) else {
            return;
        };
        if state.pending_life.is_none() {
            state.preview_life = None;
            state.life = state.authoritative_life;
        }
    }

    /// Reconcile a lane table. Ordinary reads cannot overtake an in-flight
    /// selection; the command's own authoritative reply can.
    fn sync_lanes(&mut self, answer: &wire::intent::LaneAnswer, settle: Option<TeamRequest>) {
        self.close_arrangements();
        // A team that left the authoritative subscription set is gone, not
        // hidden. Drop its browser-local controls.
        let live_teams = answer
            .subscriptions
            .iter()
            .map(|subscription| subscription.team)
            .collect::<BTreeSet<_>>();
        self.teams.retain(|team, _| live_teams.contains(team));

        for subscription in &answer.subscriptions {
            let team = subscription.team;
            let life = subscription.policy.into();
            let state = self.teams.entry(team).or_default();
            state.authoritative_life = life;
            let request = settle.filter(|request| request.team == team);
            let settles_life = request.is_some_and(|request| {
                request.life.is_some() && state.pending_life == request.life
            });
            if settles_life || (state.pending_life.is_none() && state.preview_life.is_none()) {
                state.life = life;
                state.pending_life = None;
            }
            let settles_fast = request.is_some_and(|request| {
                request.fast_mode.is_some() && state.pending_fast == request.fast_mode
            });
            if settles_fast {
                state.pending_fast = None;
            }
            if settles_life || settles_fast {
                state.issue = None;
            }
        }
    }

    fn settle_issue(&mut self, request: TeamRequest, issue: String) {
        self.settle_stale(request);
        self.teams.entry(request.team).or_default().issue = Some(issue);
    }

    /// Settle a request a newer read already answered. Convergence on what it
    /// asked for is silent; a still-shown selection that the held authority
    /// replaces is a visible rollback and reports the team issue.
    fn settle_superseded(&mut self, request: TeamRequest, fast_mode: Option<bool>) {
        let Some(state) = self.teams.get(&request.team) else {
            return;
        };
        let life_rolled_back = request.life.is_some()
            && state.pending_life == request.life
            && request.life != Some(state.authoritative_life);
        let fast_rolled_back = request.fast_mode.is_some()
            && state.pending_fast == request.fast_mode
            && fast_mode.is_some_and(|held| request.fast_mode != Some(held));
        if life_rolled_back || fast_rolled_back {
            self.settle_issue(request, ROLLED_BACK.to_owned());
        } else {
            self.settle_stale(request);
        }
    }

    /// Restore authority for the selections this request carried, unless a
    /// later request has already replaced them.
    fn settle_stale(&mut self, request: TeamRequest) {
        let Some(state) = self.teams.get_mut(&request.team) else {
            return;
        };
        if request.life.is_some() && state.pending_life == request.life {
            state.life = state.authoritative_life;
            state.preview_life = None;
            state.pending_life = None;
        }
        if request.fast_mode.is_some() && state.pending_fast == request.fast_mode {
            state.pending_fast = None;
        }
    }
}

/// The team issue when newer resident state replaces a requested selection.
const ROLLED_BACK: &str = "Newer team state replaced this request; showing the resident's setting.";

fn preview_life_callback(
    team: CanonicalUuid,
    authoritative_life: wire::intent::TeamLife,
    controls: UseReducerHandle<TeamControls>,
) -> Callback<InputEvent> {
    Callback::from(move |event: InputEvent| {
        let input: HtmlInputElement = event.target_unchecked_into();
        let index = input.value().parse::<usize>().unwrap_or(0).min(2);
        controls.dispatch(TeamControlAction::PreviewLife {
            team,
            authoritative_life,
            life: TeamLife::ALL[index],
        });
    })
}

fn commit_life_callback(
    team: CanonicalUuid,
    authoritative_life: wire::intent::TeamLife,
    topology_revision: u64,
    controls: UseReducerHandle<TeamControls>,
    sender: SenderHandle,
) -> Callback<usize> {
    Callback::from(move |index: usize| {
        let life = TeamLife::ALL[index.min(TeamLife::ALL.len() - 1)];
        if !admit_action(
            &sender,
            ClientAction::ConfigureTeam {
                topology_revision,
                team,
                member: None,
                life: Some(life.wire()),
                fast_mode: None,
            },
        ) {
            return;
        }
        controls.dispatch(TeamControlAction::RequestLife {
            team,
            authoritative_life,
            life,
        });
    })
}

fn cancel_life_preview_callback(
    team: CanonicalUuid,
    controls: UseReducerHandle<TeamControls>,
) -> Callback<FocusEvent> {
    Callback::from(move |_| {
        controls.dispatch(TeamControlAction::CancelLifePreview { team });
    })
}

/// Whether the team can take a send right now: `ready` when no life change is
/// pending and some slot is reachable, `dormant` when the controls step back,
/// and the slots each member offers.
pub(super) struct SendReadiness {
    pub(super) ready: bool,
    pub(super) dormant: bool,
    pub(super) slots: Vec<SendSlot>,
}

pub(super) fn render_team_controls(
    render: &super::fleet::FleetRender<'_, '_>,
    team_row: &crate::runtime::LaneRow,
    plane: &Plane,
    readiness: SendReadiness,
) -> Html {
    let (team, authoritative_life, fast_mode) =
        (team_row.lane, team_row.subscription, team_row.fast_mode);
    let (topology_revision, can_configure) = (render.topology_revision, render.can_configure);
    let (team_controls, sender) = (render.team_controls, render.sender);
    let set_automatic_speech = &render.controls.set_automatic_speech;
    let SendReadiness {
        ready,
        dormant,
        slots,
    } = readiness;
    let control = &plane.control;
    let speech = control.speech;
    let selected = control.preview_life.unwrap_or(control.life);
    let preview_life = preview_life_callback(team, authoritative_life, team_controls.clone());
    let commit_life = commit_life_callback(
        team,
        authoritative_life,
        topology_revision,
        team_controls.clone(),
        sender.clone(),
    );
    let cancel_life_preview = cancel_life_preview_callback(team, team_controls.clone());
    let turbo = turbo_button(
        team,
        authoritative_life,
        topology_revision,
        control.pending_fast.unwrap_or(fast_mode),
        team_controls.clone(),
        sender.clone(),
        can_configure,
    );
    let menu = {
        let controls = team_controls.clone();
        html! {
            <button
                class="team-menu"
                type="button"
                aria-label="Team menu"
                aria-haspopup="menu"
                aria-expanded={control.arrangement.is_some().to_string()}
                title="Team menu"
                onclick={Callback::from(move |event: MouseEvent| {
                    event.stop_propagation();
                    // Safari does not focus a clicked button; closing from here keeps focus here.
                    if let Some(button) = event
                        .current_target()
                        .and_then(|target| target.dyn_into::<web_sys::HtmlElement>().ok())
                    {
                        let _ = button.focus();
                    }
                    controls.dispatch(TeamControlAction::ToggleArrangement {
                        team,
                        opener: ArrangementOpener::Menu,
                    });
                })}
            >
                <span class="composer-menu-icon" aria-hidden="true">
                    <span></span><span></span><span></span>
                </span>
            </button>
        }
    };
    html! {
        <ControlsFrame
            {dormant}
            {ready}
            {topology_revision}
            {slots}
            verb="Send"
            noun="agent"
            speech={Some(speech.label())}
            lifetime={Some(selected.label())}
            lifetime_pending={control.pending_life.is_some()}
            {turbo}
            {menu}
        >
            {speech_slider(
                team,
                authoritative_life,
                speech,
                SpeechMode::ALL.map(|mode| plane.speech.admits(mode)),
                team_controls.clone(),
                set_automatic_speech.clone(),
            )}
            {lifetime_slider(
                selected,
                !can_configure || control.pending_life.is_some(),
                preview_life,
                commit_life,
                cancel_life_preview,
            )}
        </ControlsFrame>
    }
}

/// The team's fast-mode toggle retains TURBO lettering in both states.
/// Fast mode is additive: switching this flag off does not turn off a member
/// whose own flag or the global setting keeps it fast. Running OMP agents
/// whose effective value changes take it at once. A press shows its request
/// until the resident answers that command.
fn turbo_button(
    team: CanonicalUuid,
    authoritative_life: wire::intent::TeamLife,
    topology_revision: u64,
    engaged: bool,
    controls: UseReducerHandle<TeamControls>,
    sender: SenderHandle,
    can_configure: bool,
) -> Html {
    let onclick = Callback::from(move |_: MouseEvent| {
        if admit_action(
            &sender,
            ClientAction::ConfigureTeam {
                topology_revision,
                team,
                member: None,
                life: None,
                fast_mode: Some(!engaged),
            },
        ) {
            controls.dispatch(TeamControlAction::RequestFast {
                team,
                authoritative_life,
                fast_mode: !engaged,
            });
        }
    });
    let title = if engaged {
        "Turbo: every agent in this team runs in fast mode"
    } else {
        "Turbo off: press to run every agent in this team in fast mode"
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

/// The team's speech mode, applied in the input turn that selects it so the
/// queue never narrates under a mode the control no longer shows. A Quiet team
/// still admits automatic speech while a working group voices a member.
fn speech_slider(
    team: CanonicalUuid,
    authoritative_life: wire::intent::TeamLife,
    speech: SpeechMode,
    admits: [bool; 3],
    controls: UseReducerHandle<TeamControls>,
    set_automatic_speech: Callback<(String, bool)>,
) -> Html {
    let automatic_team = team.to_string();
    let oninput = Callback::from(move |event: InputEvent| {
        let input: HtmlInputElement = event.target_unchecked_into();
        let speech = SpeechMode::at(input.value().parse::<usize>().unwrap_or(1));
        set_automatic_speech.emit((automatic_team.clone(), admits[speech.index()]));
        controls.dispatch(TeamControlAction::SetSpeech {
            team,
            authoritative_life,
            speech,
        });
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
                aria-label={format!("Speech mode: {}", speech.label())}
                {oninput}
            />
            {slider_words(SpeechMode::ALL.map(SpeechMode::label), speech.index())}
        </label>
    }
}

fn lifetime_slider(
    selected: TeamLife,
    disabled: bool,
    preview: Callback<InputEvent>,
    commit: Callback<usize>,
    cancel_preview: Callback<FocusEvent>,
) -> Html {
    let onchange = commit.reform(|event: Event| {
        let input: HtmlInputElement = event.target_unchecked_into();
        input.value().parse::<usize>().unwrap_or(0)
    });
    html! {
        <label
            class="team-slider team-lifetime"
            data-stop={selected.index().to_string()}
        >
            <input
                type="range"
                min="0"
                max="2"
                step="1"
                value={selected.index().to_string()}
                aria-label={format!("Task allocation policy: {}", selected.help())}
                title={selected.help()}
                disabled={disabled}
                oninput={preview}
                onchange={onchange}
                onblur={cancel_preview}
            />
            {slider_words(TeamLife::ALL.map(|life| life.label()), selected.index())}
        </label>
    }
}

fn slider_words<const N: usize>(words: [&'static str; N], selected: usize) -> Html {
    let word = words[selected.min(words.len() - 1)];
    html! {
        <span class="team-slider-words" aria-hidden="true">
            <span>{crate::browser::view::chrome::fit_word(word)}</span>
        </span>
    }
}

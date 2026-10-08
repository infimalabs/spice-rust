use super::*;

mod frame;
use frame::ControlsFrame;
pub(super) use frame::SendSlot;

const TEAM_SPEECH_STORAGE: &str = "spice:team-speech:v1";

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

/// Browser-local speech posture for one app team.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TeamSpeech {
    Quiet,
    Speak,
    Narrate,
}

impl TeamSpeech {
    const ALL: [Self; 3] = [Self::Quiet, Self::Speak, Self::Narrate];

    const fn label(self) -> &'static str {
        match self {
            Self::Quiet => "Quiet",
            Self::Speak => "Speak",
            Self::Narrate => "Narrate",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Quiet => 0,
            Self::Speak => 1,
            Self::Narrate => 2,
        }
    }

    const fn storage_value(self) -> u8 {
        self.index() as u8
    }

    fn from_storage(value: u8) -> Option<Self> {
        Self::ALL.get(usize::from(value)).copied()
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct TeamControl {
    pub(super) arrangement_open: bool,
    pub(super) speech: TeamSpeech,
    pub(super) life: TeamLife,
    preview_life: Option<TeamLife>,
    pub(super) pending_life: Option<TeamLife>,
    authoritative_life: TeamLife,
    issue: Option<String>,
}

impl Default for TeamControl {
    fn default() -> Self {
        Self {
            arrangement_open: false,
            speech: TeamSpeech::Quiet,
            life: TeamLife::Steer,
            preview_life: None,
            pending_life: None,
            authoritative_life: TeamLife::Steer,
            issue: None,
        }
    }
}

/// Per-team control state retained across Mosaic redraws.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct TeamControls {
    teams: BTreeMap<CanonicalUuid, TeamControl>,
}

pub(crate) enum TeamControlAction {
    ToggleArrangement {
        team: CanonicalUuid,
    },
    CloseArrangement,
    SetSpeech {
        team: CanonicalUuid,
        authoritative_life: wire::intent::TeamLife,
        speech: TeamSpeech,
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
    SyncLanes {
        answer: wire::intent::LaneAnswer,
        settle: Option<(CanonicalUuid, TeamLife)>,
    },
    SettleIssue {
        team: CanonicalUuid,
        life: Option<TeamLife>,
        issue: String,
    },
    SettleStaleLife {
        team: CanonicalUuid,
        life: TeamLife,
    },
}

impl Reducible for TeamControls {
    type Action = TeamControlAction;

    fn reduce(self: Rc<Self>, action: Self::Action) -> Rc<Self> {
        let mut next = (*self).clone();
        match action {
            TeamControlAction::ToggleArrangement { team } => {
                next.toggle_arrangement(team);
            }
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
            TeamControlAction::SyncLanes { answer, settle } => {
                next.sync_lanes(&answer, settle);
            }
            TeamControlAction::SettleIssue { team, life, issue } => {
                next.settle_issue(team, life, issue);
            }
            TeamControlAction::SettleStaleLife { team, life } => {
                next.settle_stale_life(team, life);
            }
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
        let Some(document) = storage.get_item(TEAM_SPEECH_STORAGE).ok().flatten() else {
            return controls;
        };
        let Ok(rows) = serde_json::from_str::<Vec<(String, u8)>>(&document) else {
            return controls;
        };
        for (team, value) in rows {
            let (Ok(team), Some(speech)) = (team.parse(), TeamSpeech::from_storage(value)) else {
                continue;
            };
            controls.teams.entry(team).or_default().speech = speech;
        }
        controls
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

    pub(in crate::browser) fn any_arrangement_open(&self) -> bool {
        self.teams.values().any(|team| team.arrangement_open)
    }

    fn toggle_arrangement(&mut self, team: CanonicalUuid) {
        let open = !self
            .teams
            .get(&team)
            .is_some_and(|state| state.arrangement_open);
        self.close_arrangements();
        self.teams.entry(team).or_default().arrangement_open = open;
    }

    fn close_arrangements(&mut self) {
        for state in self.teams.values_mut() {
            state.arrangement_open = false;
        }
    }

    fn set_speech(
        &mut self,
        team: CanonicalUuid,
        authoritative_life: wire::intent::TeamLife,
        speech: TeamSpeech,
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

    /// Each team's in-flight policy send and its last refusal, which the
    /// next send clears.
    pub(super) fn requests(
        &self,
    ) -> impl Iterator<Item = (CanonicalUuid, Option<TeamLife>, Option<&str>)> {
        self.teams
            .iter()
            .map(|(team, control)| (*team, control.pending_life, control.issue.as_deref()))
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
    fn sync_lanes(
        &mut self,
        answer: &wire::intent::LaneAnswer,
        settle: Option<(CanonicalUuid, TeamLife)>,
    ) {
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
            let settles_pending = settle.is_some_and(|(settled_team, requested)| {
                settled_team == team && state.pending_life == Some(requested)
            });
            if settles_pending || (state.pending_life.is_none() && state.preview_life.is_none()) {
                state.life = life;
                state.pending_life = None;
                if settles_pending {
                    state.issue = None;
                }
            }
        }
    }

    fn settle_issue(&mut self, team: CanonicalUuid, life: Option<TeamLife>, issue: String) {
        let state = self.teams.entry(team).or_default();
        if life.is_some_and(|life| state.pending_life == Some(life)) {
            state.life = state.authoritative_life;
            state.preview_life = None;
            state.pending_life = None;
        }
        state.issue = Some(issue);
    }

    fn settle_stale_life(&mut self, team: CanonicalUuid, life: TeamLife) {
        let Some(state) = self.teams.get_mut(&team) else {
            return;
        };
        if state.pending_life == Some(life) {
            state.life = state.authoritative_life;
            state.preview_life = None;
            state.pending_life = None;
        }
    }
}

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

#[allow(
    clippy::too_many_arguments,
    reason = "watchband rendering binds team identity and interactive control state"
)]
pub(super) fn render_team_controls(
    team: CanonicalUuid,
    authoritative_life: wire::intent::TeamLife,
    topology_revision: u64,
    control: &TeamControl,
    team_controls: &UseReducerHandle<TeamControls>,
    set_automatic_speech: &Callback<(String, bool)>,
    sender: &SenderHandle,
    can_arrange: bool,
    can_configure: bool,
    ready: bool,
    dormant: bool,
    slots: Vec<SendSlot>,
    turbo: Html,
) -> Html {
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
    let menu = {
        let controls = team_controls.clone();
        html! {
            <button
                class="team-menu"
                type="button"
                disabled={!can_arrange}
                aria-label="Team menu"
                aria-haspopup="menu"
                aria-expanded={control.arrangement_open.to_string()}
                title={can_arrange.then_some("Arrange this team")}
                onclick={Callback::from(move |event: MouseEvent| {
                    event.stop_propagation();
                    controls.dispatch(TeamControlAction::ToggleArrangement { team });
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
            speech={speech.label()}
            lifetime={selected.label()}
            lifetime_pending={control.pending_life.is_some()}
            {turbo}
            {menu}
        >
            {speech_slider(
                team,
                authoritative_life,
                speech,
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

fn speech_slider(
    team: CanonicalUuid,
    authoritative_life: wire::intent::TeamLife,
    speech: TeamSpeech,
    controls: UseReducerHandle<TeamControls>,
    set_automatic_speech: Callback<(String, bool)>,
) -> Html {
    let automatic_team = team.to_string();
    let oninput = Callback::from(move |event: InputEvent| {
        let input: HtmlInputElement = event.target_unchecked_into();
        let index = input.value().parse::<usize>().unwrap_or(1);
        let speech = TeamSpeech::ALL[index.min(TeamSpeech::ALL.len() - 1)];
        set_automatic_speech.emit((automatic_team.clone(), speech != TeamSpeech::Quiet));
        controls.dispatch(TeamControlAction::SetSpeech {
            team,
            authoritative_life,
            speech,
        });
    });
    html! {
        <label
            class={classes!(
                "team-slider",
                "team-speech",
                (speech == TeamSpeech::Quiet).then_some("armed"),
                (speech == TeamSpeech::Narrate).then_some("maxed"),
            )}
            data-stop={speech.index().to_string()}
        >
            <input
                type="range"
                min="0"
                max="2"
                step="1"
                value={speech.index().to_string()}
                aria-label={format!("Speech mode: {}", speech.label())}
                oninput={oninput}
            />
            {fit_word(speech.label())}
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
            class={classes!(
                "team-slider",
                "team-lifetime",
                (selected == TeamLife::Steer).then_some("armed"),
                (selected == TeamLife::Drain).then_some("maxed"),
            )}
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
            {fit_word(selected.label())}
        </label>
    }
}

/// One control word drawn to fill its box exactly: every word occupies the
/// same physical width and height, squeezed or stretched to fit, so a long
/// word and a short one read as equal labels on equal buttons.
pub(super) fn fit_word(word: &'static str) -> Html {
    html! {
        <svg class="fit-word" viewBox="0 0 100 11" preserveAspectRatio="none" aria-hidden="true">
            <text x="0" y="11" font-size="15" textLength="100" lengthAdjust="spacingAndGlyphs">
                {word}
            </text>
        </svg>
    }
}

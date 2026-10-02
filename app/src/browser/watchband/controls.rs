use super::*;

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

/// Browser-local speech posture for one Canvas team.
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
    pub(super) mode: TeamMode,
    pub(super) menu_open: bool,
    pub(super) import_open: bool,
    pub(super) arrangement_open: bool,
    pub(super) previous_topology: Option<Vec<TeamArrangement>>,
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
            mode: TeamMode::Chat,
            menu_open: false,
            import_open: false,
            arrangement_open: false,
            previous_topology: None,
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
    empty_mode: TeamMode,
}

pub(crate) enum TeamControlAction {
    SetMode {
        team: Option<CanonicalUuid>,
        mode: TeamMode,
    },
    ToggleMenu {
        team: CanonicalUuid,
    },
    ToggleImport {
        team: CanonicalUuid,
    },
    ToggleArrangement {
        team: CanonicalUuid,
    },
    CloseMenus,
    /// Switch a team to Chat, where its members' composers live.
    ShowChat {
        team: CanonicalUuid,
    },
    SaveSplit {
        team: CanonicalUuid,
        topology: Vec<TeamArrangement>,
    },
    ClearRestore {
        team: CanonicalUuid,
    },
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
            TeamControlAction::SetMode { team, mode } => next.set_mode(team, mode),
            TeamControlAction::ToggleMenu { team } => next.toggle_menu(team),
            TeamControlAction::ToggleImport { team } => next.toggle_import(team),
            TeamControlAction::ToggleArrangement { team } => {
                next.toggle_arrangement(team);
            }
            TeamControlAction::CloseMenus => next.close_menus(),
            TeamControlAction::ShowChat { team } => next.set_mode(Some(team), TeamMode::Chat),
            TeamControlAction::SaveSplit { team, topology } => {
                next.save_split(team, topology);
            }
            TeamControlAction::ClearRestore { team } => {
                next.teams.entry(team).or_default().previous_topology = None;
            }
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

    pub(in crate::browser) fn mode(&self, team: CanonicalUuid) -> TeamMode {
        self.teams
            .get(&team)
            .map_or(TeamMode::Chat, |team| team.mode)
    }

    pub(super) const fn empty_mode(&self) -> TeamMode {
        self.empty_mode
    }

    pub(in crate::browser) fn any_menu_open(&self) -> bool {
        self.teams
            .values()
            .any(|team| team.menu_open || team.import_open || team.arrangement_open)
    }

    fn set_mode(&mut self, team: Option<CanonicalUuid>, mode: TeamMode) {
        if let Some(team) = team {
            self.teams.entry(team).or_default().mode = mode;
        } else {
            self.empty_mode = mode;
        }
    }

    fn toggle_menu(&mut self, team: CanonicalUuid) {
        let state = self.teams.entry(team).or_default();
        state.arrangement_open = false;
        if state.import_open {
            state.import_open = false;
            state.menu_open = false;
            return;
        }
        state.menu_open = !state.menu_open;
    }

    fn toggle_import(&mut self, team: CanonicalUuid) {
        let state = self.teams.entry(team).or_default();
        state.menu_open = false;
        state.arrangement_open = false;
        state.import_open = !state.import_open;
    }

    fn toggle_arrangement(&mut self, team: CanonicalUuid) {
        let open = !self
            .teams
            .get(&team)
            .is_some_and(|state| state.arrangement_open);
        self.close_menus();
        self.teams.entry(team).or_default().arrangement_open = open;
    }

    fn close_menus(&mut self) {
        for state in self.teams.values_mut() {
            state.menu_open = false;
            state.import_open = false;
            state.arrangement_open = false;
        }
    }

    fn close_arrangements(&mut self) {
        for state in self.teams.values_mut() {
            state.arrangement_open = false;
        }
    }

    fn save_split(&mut self, team: CanonicalUuid, topology: Vec<TeamArrangement>) {
        let state = self.teams.entry(team).or_default();
        state.menu_open = false;
        state.import_open = false;
        state.arrangement_open = false;
        state.previous_topology = Some(topology);
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
        // hidden. Drop its browser-local controls before deciding whether a
        // newly seated first team should inherit the empty surface.
        let live_teams = answer
            .subscriptions
            .iter()
            .map(|subscription| subscription.team)
            .collect::<BTreeSet<_>>();
        self.teams.retain(|team, _| live_teams.contains(team));

        // A seat submitted from the empty Work view must keep that view when
        // its first real surface arrives. The resident's authoritative
        // subscription order resolves the transition: its first team
        // inherits the empty selection exactly once, and every sibling keeps
        // the explicit Chat default.
        if self.teams.is_empty()
            && self.empty_mode != TeamMode::Chat
            && let Some(subscription) = answer.subscriptions.first()
        {
            self.teams.entry(subscription.team).or_default().mode = self.empty_mode;
            self.empty_mode = TeamMode::Chat;
        }
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
) -> Callback<Event> {
    Callback::from(move |event: Event| {
        let input: HtmlInputElement = event.target_unchecked_into();
        let index = input.value().parse::<usize>().unwrap_or(0).min(2);
        let life = TeamLife::ALL[index];
        if !admit_action(
            &sender,
            ClientAction::ConfigureTeam {
                topology_revision,
                team,
                member: None,
                life: Some(life.wire()),
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
    suppress_automatic_speech: &Callback<()>,
    sender: &SenderHandle,
    can_configure: bool,
    ready: bool,
) -> Html {
    let speech = control.speech;
    let update_speech = team_controls.clone();
    let suppress_automatic_speech = suppress_automatic_speech.clone();
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
    html! {
        <aside
            class="team-composer-controls"
            data-speech-mode={speech.label()}
            data-lifetime={selected.label()}
            data-lifetime-pending={control.pending_life.is_some().to_string()}
        >
            <label class={classes!(
                "team-slider",
                "team-speech",
                (speech == TeamSpeech::Quiet).then_some("armed"),
                (speech == TeamSpeech::Narrate).then_some("maxed"),
            )}>
                <input
                    type="range"
                    min="0"
                    max="2"
                    step="1"
                    aria-orientation="vertical"
                    value={speech.index().to_string()}
                    aria-label={format!("Speech mode: {}", speech.label())}
                    oninput={Callback::from(move |event: InputEvent| {
                        let input: HtmlInputElement = event.target_unchecked_into();
                        let index = input.value().parse::<usize>().unwrap_or(1).min(2);
                        let speech = TeamSpeech::ALL[index];
                        if speech == TeamSpeech::Quiet {
                            suppress_automatic_speech.emit(());
                        }
                        update_speech.dispatch(TeamControlAction::SetSpeech {
                            team,
                            authoritative_life,
                            speech,
                        });
                    })}
                />
                <span>{speech.label()}</span>
            </label>
            <label class={classes!(
                "team-slider",
                "team-lifetime",
                (selected == TeamLife::Steer).then_some("armed"),
                (selected == TeamLife::Drain).then_some("maxed"),
            )}>
                <input
                    type="range"
                    min="0"
                    max="2"
                    step="1"
                    aria-orientation="vertical"
                    value={selected.index().to_string()}
                    aria-label={format!("Task allocation policy: {}", selected.help())}
                    title={selected.help()}
                    disabled={!can_configure || control.pending_life.is_some()}
                    oninput={preview_life}
                    onchange={commit_life}
                    onblur={cancel_life_preview}
                />
                <span>{selected.label()}</span>
            </label>
            <button
                class={classes!(
                    "team-send",
                    (selected == TeamLife::Steer).then_some("armed"),
                    (selected == TeamLife::Drain).then_some("maxed"),
                )}
                type="submit"
                disabled={!ready}
                title={format!("Send with {}: {}", selected.label(), selected.help())}
            >{selected.label()}</button>
        </aside>
    }
}

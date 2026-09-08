use super::*;

const LINEAGE_SPEECH_STORAGE: &str = "spice:lineage-speech:v1";

/// One policy shared by every agent member in the visual lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineageLife {
    Steer,
    Drive,
    Drain,
}

impl LineageLife {
    const ALL: [Self; 3] = [Self::Steer, Self::Drive, Self::Drain];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::Steer => "Steer",
            Self::Drive => "Drive",
            Self::Drain => "Drain",
        }
    }

    const fn help(self) -> &'static str {
        match self.wire().scope() {
            wire::intent::TaskSubscriptionScope::Manual => "Use operator-selected work only",
            wire::intent::TaskSubscriptionScope::ManualAndAutoProject => {
                "Auto-subscribe to projects this organization creates or claims"
            }
            wire::intent::TaskSubscriptionScope::AllAssignable => {
                "Boundary dissolved: see all assignable work"
            }
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Steer => 0,
            Self::Drive => 1,
            Self::Drain => 2,
        }
    }

    const fn wire(self) -> wire::intent::LineageLife {
        match self {
            Self::Steer => wire::intent::LineageLife::Steer,
            Self::Drive => wire::intent::LineageLife::Drive,
            Self::Drain => wire::intent::LineageLife::Drain,
        }
    }
}

impl From<wire::intent::LineageLife> for LineageLife {
    fn from(value: wire::intent::LineageLife) -> Self {
        match value {
            wire::intent::LineageLife::Steer => Self::Steer,
            wire::intent::LineageLife::Drive => Self::Drive,
            wire::intent::LineageLife::Drain => Self::Drain,
        }
    }
}

/// Browser-local speech posture for one Canvas organization.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineageSpeech {
    Quiet,
    Speak,
    Narrate,
}

impl LineageSpeech {
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
pub(super) struct LineageControl {
    pub(super) mode: LineageMode,
    pub(super) menu_open: bool,
    pub(super) import_open: bool,
    pub(super) arrangement_open: bool,
    pub(super) previous_topology: Option<Vec<LineageArrangement>>,
    pub(super) speech: LineageSpeech,
    pub(super) life: LineageLife,
    pub(super) pending_life: Option<LineageLife>,
    authoritative_life: LineageLife,
    issue: Option<String>,
}

impl Default for LineageControl {
    fn default() -> Self {
        Self {
            mode: LineageMode::Lanes,
            menu_open: false,
            import_open: false,
            arrangement_open: false,
            previous_topology: None,
            speech: LineageSpeech::Speak,
            life: LineageLife::Steer,
            pending_life: None,
            authoritative_life: LineageLife::Steer,
            issue: None,
        }
    }
}

/// Per-organization control state retained across Mosaic redraws.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct LineageControls {
    lineages: BTreeMap<CanonicalUuid, LineageControl>,
    empty_mode: LineageMode,
    closed: BTreeSet<CanonicalUuid>,
}

pub(crate) enum LineageControlAction {
    SetMode {
        lineage: Option<CanonicalUuid>,
        mode: LineageMode,
    },
    ToggleMenu {
        lineage: CanonicalUuid,
    },
    ToggleImport {
        lineage: CanonicalUuid,
    },
    ToggleArrangement {
        lineage: CanonicalUuid,
    },
    CloseMenus,
    CloseLineage {
        lineage: CanonicalUuid,
    },
    OpenLineage {
        lineage: CanonicalUuid,
    },
    SaveSplit {
        lineage: CanonicalUuid,
        topology: Vec<LineageArrangement>,
    },
    ClearRestore {
        lineage: CanonicalUuid,
    },
    SetSpeech {
        lineage: CanonicalUuid,
        authoritative_life: wire::intent::LineageLife,
        speech: LineageSpeech,
    },
    RequestLife {
        lineage: CanonicalUuid,
        authoritative_life: wire::intent::LineageLife,
        life: LineageLife,
    },
    SyncLanes {
        answer: wire::intent::LaneAnswer,
        settle: Option<(CanonicalUuid, LineageLife)>,
    },
    SettleIssue {
        lineage: CanonicalUuid,
        life: Option<LineageLife>,
        issue: String,
    },
    SettleStaleLife {
        lineage: CanonicalUuid,
        life: LineageLife,
    },
}

impl Reducible for LineageControls {
    type Action = LineageControlAction;

    fn reduce(self: Rc<Self>, action: Self::Action) -> Rc<Self> {
        let mut next = (*self).clone();
        match action {
            LineageControlAction::SetMode { lineage, mode } => next.set_mode(lineage, mode),
            LineageControlAction::ToggleMenu { lineage } => next.toggle_menu(lineage),
            LineageControlAction::ToggleImport { lineage } => next.toggle_import(lineage),
            LineageControlAction::ToggleArrangement { lineage } => {
                next.toggle_arrangement(lineage);
            }
            LineageControlAction::CloseMenus => next.close_menus(),
            LineageControlAction::CloseLineage { lineage } => next.close_lineage(lineage),
            LineageControlAction::OpenLineage { lineage } => next.open_lineage(lineage),
            LineageControlAction::SaveSplit { lineage, topology } => {
                next.save_split(lineage, topology);
            }
            LineageControlAction::ClearRestore { lineage } => {
                next.lineages.entry(lineage).or_default().previous_topology = None;
            }
            LineageControlAction::SetSpeech {
                lineage,
                authoritative_life,
                speech,
            } => next.set_speech(lineage, authoritative_life, speech),
            LineageControlAction::RequestLife {
                lineage,
                authoritative_life,
                life,
            } => next.request_life(lineage, authoritative_life, life),
            LineageControlAction::SyncLanes { answer, settle } => {
                next.sync_lanes(&answer, settle);
            }
            LineageControlAction::SettleIssue {
                lineage,
                life,
                issue,
            } => {
                next.settle_issue(lineage, life, issue);
            }
            LineageControlAction::SettleStaleLife { lineage, life } => {
                next.settle_stale_life(lineage, life);
            }
        }
        Rc::new(next)
    }
}

impl LineageControls {
    pub(in crate::browser) fn restore() -> Self {
        let mut controls = Self::default();
        let Some(storage) =
            web_sys::window().and_then(|window| window.local_storage().ok().flatten())
        else {
            return controls;
        };
        let Some(document) = storage.get_item(LINEAGE_SPEECH_STORAGE).ok().flatten() else {
            return controls;
        };
        let Ok(rows) = serde_json::from_str::<Vec<(String, u8)>>(&document) else {
            return controls;
        };
        for (lineage, value) in rows {
            let (Ok(lineage), Some(speech)) = (lineage.parse(), LineageSpeech::from_storage(value))
            else {
                continue;
            };
            controls.lineages.entry(lineage).or_default().speech = speech;
        }
        controls
    }

    pub(super) fn current(
        &self,
        lineage: CanonicalUuid,
        authoritative_life: wire::intent::LineageLife,
    ) -> LineageControl {
        self.lineages
            .get(&lineage)
            .cloned()
            .unwrap_or_else(|| LineageControl {
                life: authoritative_life.into(),
                authoritative_life: authoritative_life.into(),
                ..LineageControl::default()
            })
    }

    pub(super) const fn empty_mode(&self) -> LineageMode {
        self.empty_mode
    }

    pub(in crate::browser) fn is_closed(&self, lineage: CanonicalUuid) -> bool {
        self.closed.contains(&lineage)
    }

    fn set_mode(&mut self, lineage: Option<CanonicalUuid>, mode: LineageMode) {
        if let Some(lineage) = lineage {
            self.lineages.entry(lineage).or_default().mode = mode;
        } else {
            self.empty_mode = mode;
        }
    }

    fn toggle_menu(&mut self, lineage: CanonicalUuid) {
        let state = self.lineages.entry(lineage).or_default();
        state.arrangement_open = false;
        if state.import_open {
            state.import_open = false;
            state.menu_open = false;
            return;
        }
        state.menu_open = !state.menu_open;
    }

    fn toggle_import(&mut self, lineage: CanonicalUuid) {
        let state = self.lineages.entry(lineage).or_default();
        state.menu_open = false;
        state.arrangement_open = false;
        state.import_open = !state.import_open;
    }

    fn toggle_arrangement(&mut self, lineage: CanonicalUuid) {
        let open = !self
            .lineages
            .get(&lineage)
            .is_some_and(|state| state.arrangement_open);
        self.close_menus();
        self.lineages.entry(lineage).or_default().arrangement_open = open;
    }

    fn close_menus(&mut self) {
        for state in self.lineages.values_mut() {
            state.menu_open = false;
            state.import_open = false;
            state.arrangement_open = false;
        }
    }

    fn close_arrangements(&mut self) {
        for state in self.lineages.values_mut() {
            state.arrangement_open = false;
        }
    }

    fn close_lineage(&mut self, lineage: CanonicalUuid) {
        let state = self.lineages.entry(lineage).or_default();
        state.menu_open = false;
        state.import_open = false;
        state.arrangement_open = false;
        self.closed.insert(lineage);
    }

    fn open_lineage(&mut self, lineage: CanonicalUuid) {
        self.closed.remove(&lineage);
    }

    fn save_split(&mut self, lineage: CanonicalUuid, topology: Vec<LineageArrangement>) {
        let state = self.lineages.entry(lineage).or_default();
        state.menu_open = false;
        state.import_open = false;
        state.arrangement_open = false;
        state.previous_topology = Some(topology);
    }

    fn set_speech(
        &mut self,
        lineage: CanonicalUuid,
        authoritative_life: wire::intent::LineageLife,
        speech: LineageSpeech,
    ) {
        self.lineages
            .entry(lineage)
            .or_insert_with(|| LineageControl {
                life: authoritative_life.into(),
                authoritative_life: authoritative_life.into(),
                ..LineageControl::default()
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
            .lineages
            .iter()
            .map(|(lineage, control)| (lineage.to_string(), control.speech.storage_value()))
            .collect::<Vec<_>>();
        if let Ok(document) = serde_json::to_string(&rows) {
            let _ = storage.set_item(LINEAGE_SPEECH_STORAGE, &document);
        }
    }

    fn request_life(
        &mut self,
        lineage: CanonicalUuid,
        authoritative_life: wire::intent::LineageLife,
        life: LineageLife,
    ) {
        let state = self
            .lineages
            .entry(lineage)
            .or_insert_with(|| LineageControl {
                life: authoritative_life.into(),
                authoritative_life: authoritative_life.into(),
                ..LineageControl::default()
            });
        state.life = life;
        state.pending_life = Some(life);
        state.issue = None;
    }

    /// Reconcile a lane table. Ordinary reads cannot overtake an in-flight
    /// selection; the command's own authoritative reply can.
    fn sync_lanes(
        &mut self,
        answer: &wire::intent::LaneAnswer,
        settle: Option<(CanonicalUuid, LineageLife)>,
    ) {
        self.close_arrangements();
        // A lineage that left the authoritative subscription set is gone, not
        // hidden. Drop its browser-local controls before deciding whether a
        // newly seated first lineage should inherit the empty surface.
        let live_lineages = answer
            .subscriptions
            .iter()
            .map(|subscription| subscription.lineage_id)
            .collect::<BTreeSet<_>>();
        self.lineages
            .retain(|lineage, _| live_lineages.contains(lineage));
        self.closed
            .retain(|lineage| live_lineages.contains(lineage));

        // A seat submitted from the empty Org view must keep that view when
        // its first real surface arrives. The resident's authoritative
        // subscription order resolves the transition: its first lineage
        // inherits the empty selection exactly once, and every sibling keeps
        // the explicit Lanes default.
        if self.lineages.is_empty()
            && self.empty_mode != LineageMode::Lanes
            && let Some(subscription) = answer.subscriptions.first()
        {
            self.lineages
                .entry(subscription.lineage_id)
                .or_default()
                .mode = self.empty_mode;
            self.empty_mode = LineageMode::Lanes;
        }
        for subscription in &answer.subscriptions {
            let lineage = subscription.lineage_id;
            let life = subscription.policy.into();
            let state = self.lineages.entry(lineage).or_default();
            state.authoritative_life = life;
            let settles_pending = settle.is_some_and(|(settled_lineage, requested)| {
                settled_lineage == lineage && state.pending_life == Some(requested)
            });
            if settles_pending || state.pending_life.is_none() {
                state.life = life;
                state.pending_life = None;
                if settles_pending {
                    state.issue = None;
                }
            }
        }
    }

    fn settle_issue(&mut self, lineage: CanonicalUuid, life: Option<LineageLife>, issue: String) {
        let state = self.lineages.entry(lineage).or_default();
        if life.is_some_and(|life| state.pending_life == Some(life)) {
            state.life = state.authoritative_life;
            state.pending_life = None;
        }
        state.issue = Some(issue);
    }

    fn settle_stale_life(&mut self, lineage: CanonicalUuid, life: LineageLife) {
        let Some(state) = self.lineages.get_mut(&lineage) else {
            return;
        };
        if state.pending_life == Some(life) {
            state.life = state.authoritative_life;
            state.pending_life = None;
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "watchband rendering binds organization identity and interactive control state"
)]
pub(super) fn render_lineage_controls(
    lineage: CanonicalUuid,
    authoritative_life: wire::intent::LineageLife,
    topology_revision: u64,
    control: &LineageControl,
    lineage_controls: &UseReducerHandle<LineageControls>,
    suppress_automatic_speech: &Callback<()>,
    sender: &SenderHandle,
    can_configure: bool,
    ready: bool,
) -> Html {
    let speech = control.speech;
    let update_speech = lineage_controls.clone();
    let suppress_automatic_speech = suppress_automatic_speech.clone();
    let selected = control.life;
    let update_life = lineage_controls.clone();
    let life_sender = sender.clone();
    html! {
        <aside
            class="organization-composer-controls"
            data-speech-mode={speech.label()}
            data-lifetime={selected.label()}
            data-lifetime-pending={control.pending_life.is_some().to_string()}
        >
            <label class={classes!(
                "organization-slider",
                "organization-speech",
                (speech == LineageSpeech::Quiet).then_some("armed"),
                (speech == LineageSpeech::Narrate).then_some("maxed"),
            )}>
                <input
                    type="range"
                    min="0"
                    max="2"
                    step="1"
                    value={speech.index().to_string()}
                    aria-label={format!("Speech mode: {}", speech.label())}
                    oninput={Callback::from(move |event: InputEvent| {
                        let input: HtmlInputElement = event.target_unchecked_into();
                        let index = input.value().parse::<usize>().unwrap_or(1).min(2);
                        let speech = LineageSpeech::ALL[index];
                        if speech == LineageSpeech::Quiet {
                            suppress_automatic_speech.emit(());
                        }
                        update_speech.dispatch(LineageControlAction::SetSpeech {
                            lineage,
                            authoritative_life,
                            speech,
                        });
                    })}
                />
                <span>{speech.label()}</span>
            </label>
            <label class={classes!(
                "organization-slider",
                "organization-lifetime",
                (selected == LineageLife::Steer).then_some("armed"),
                (selected == LineageLife::Drain).then_some("maxed"),
            )}>
                <input
                    type="range"
                    min="0"
                    max="2"
                    step="1"
                    value={selected.index().to_string()}
                    aria-label={format!("Task subscription policy: {}", selected.help())}
                    title={selected.help()}
                    disabled={!can_configure || control.pending_life.is_some()}
                    oninput={Callback::from(move |event: InputEvent| {
                        let input: HtmlInputElement = event.target_unchecked_into();
                        let index = input.value().parse::<usize>().unwrap_or(0).min(2);
                        let life = LineageLife::ALL[index];
                        update_life.dispatch(LineageControlAction::RequestLife {
                            lineage,
                            authoritative_life,
                            life,
                        });
                        send_action(
                            &life_sender,
                            ClientAction::ConfigureLineage {
                                topology_revision,
                                lineage,
                                member: None,
                                life: Some(life.wire()),
                            },
                        );
                    })}
                />
                <span>{selected.label()}</span>
            </label>
            <button
                class={classes!(
                    "organization-send",
                    (selected == LineageLife::Steer).then_some("armed"),
                    (selected == LineageLife::Drain).then_some("maxed"),
                )}
                type="submit"
                disabled={!ready}
                title={format!("Send with {}: {}", selected.label(), selected.help())}
            >{selected.label()}</button>
            if let Some(issue) = &control.issue {
                <p class="organization-control-issue" role="status">{issue.clone()}</p>
            }
        </aside>
    }
}

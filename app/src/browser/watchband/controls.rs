use super::*;

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
            wire::intent::TaskSubscriptionScope::Manual => "Manual task filters",
            wire::intent::TaskSubscriptionScope::ManualAndAutoProject => {
                "Auto-subscribe to projects this team creates or claims"
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

/// Browser-local speech posture for one Canvas team.
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
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct LineageControl {
    pub(super) mode: LineageMode,
    pub(super) speech: LineageSpeech,
    pub(super) life: LineageLife,
    pub(super) pending_life: bool,
    authoritative_life: LineageLife,
    issue: Option<String>,
}

impl Default for LineageControl {
    fn default() -> Self {
        Self {
            mode: LineageMode::Compose,
            speech: LineageSpeech::Speak,
            life: LineageLife::Steer,
            pending_life: false,
            authoritative_life: LineageLife::Steer,
            issue: None,
        }
    }
}

/// Per-team control state retained across Mosaic redraws.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct LineageControls {
    lineages: BTreeMap<CanonicalUuid, LineageControl>,
    empty_mode: LineageMode,
}

pub(crate) enum LineageControlAction {
    SetMode {
        lineage: Option<CanonicalUuid>,
        mode: LineageMode,
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
        settle: bool,
    },
    SettleIssue {
        lineage: CanonicalUuid,
        issue: String,
    },
}

impl Reducible for LineageControls {
    type Action = LineageControlAction;

    fn reduce(self: Rc<Self>, action: Self::Action) -> Rc<Self> {
        let mut next = (*self).clone();
        match action {
            LineageControlAction::SetMode { lineage, mode } => next.set_mode(lineage, mode),
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
            LineageControlAction::SettleIssue { lineage, issue } => {
                next.settle_issue(lineage, issue);
            }
        }
        Rc::new(next)
    }
}

impl LineageControls {
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

    fn set_mode(&mut self, lineage: Option<CanonicalUuid>, mode: LineageMode) {
        if let Some(lineage) = lineage {
            self.lineages.entry(lineage).or_default().mode = mode;
        } else {
            self.empty_mode = mode;
        }
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
        state.pending_life = true;
        state.issue = None;
    }

    /// Reconcile a lane table. Ordinary reads cannot overtake an in-flight
    /// selection; the command's own authoritative reply can.
    fn sync_lanes(&mut self, answer: &wire::intent::LaneAnswer, settle: bool) {
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

        // A seat submitted from the empty Info view must keep that view when
        // its first real surface arrives. The resident's authoritative
        // subscription order resolves the transition: its first lineage
        // inherits the empty selection exactly once, and every sibling keeps
        // the explicit Compose default.
        if self.lineages.is_empty()
            && self.empty_mode != LineageMode::Compose
            && let Some(subscription) = answer.subscriptions.first()
        {
            self.lineages
                .entry(subscription.lineage_id)
                .or_default()
                .mode = self.empty_mode;
            self.empty_mode = LineageMode::Compose;
        }
        for subscription in &answer.subscriptions {
            let lineage = subscription.lineage_id;
            let life = subscription.policy.into();
            let state = self.lineages.entry(lineage).or_default();
            state.authoritative_life = life;
            if settle || !state.pending_life {
                state.life = life;
                state.pending_life = false;
                if settle {
                    state.issue = None;
                }
            }
        }
    }

    fn settle_issue(&mut self, lineage: CanonicalUuid, issue: String) {
        let state = self.lineages.entry(lineage).or_default();
        state.life = state.authoritative_life;
        state.pending_life = false;
        state.issue = Some(issue);
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "watchband rendering binds team identity and interactive control state"
)]
pub(super) fn render_lineage_controls(
    lineage: CanonicalUuid,
    authoritative_life: wire::intent::LineageLife,
    control: &LineageControl,
    lineage_controls: &UseReducerHandle<LineageControls>,
    sender: &SenderHandle,
    can_configure: bool,
    ready: bool,
    wake_ready: bool,
) -> Html {
    let speech = control.speech;
    let update_speech = lineage_controls.clone();
    let selected = control.life;
    let update_life = lineage_controls.clone();
    let life_sender = sender.clone();
    html! {
        <aside
            class="team-composer-controls"
            data-speech-mode={speech.label()}
            data-lifetime={selected.label()}
            data-lifetime-pending={control.pending_life.to_string()}
        >
            <label class={classes!(
                "team-slider",
                "team-speech",
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
                        update_speech.dispatch(LineageControlAction::SetSpeech {
                            lineage,
                            authoritative_life,
                            speech: LineageSpeech::ALL[index],
                        });
                    })}
                />
                <span>{speech.label()}</span>
            </label>
            <label class={classes!(
                "team-slider",
                "team-lifetime",
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
                    disabled={!can_configure || control.pending_life}
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
                                lineage,
                                member: None,
                                life: life.wire(),
                            },
                        );
                    })}
                />
                <span>{selected.label()}</span>
            </label>
            <button
                class={classes!(
                    "team-send",
                    (selected == LineageLife::Steer).then_some("armed"),
                    (selected == LineageLife::Drain).then_some("maxed"),
                )}
                type="submit"
                disabled={!ready}
                title={if wake_ready {
                    "Wake vacant agents, confirm fresh residency, then steer their drafts".to_owned()
                } else {
                    format!("Send with {}: {}", selected.label(), selected.help())
                }}
            >{if wake_ready { "Wake & Steer" } else { selected.label() }}</button>
            if let Some(issue) = &control.issue {
                <p class="team-control-issue" role="status">{issue.clone()}</p>
            }
        </aside>
    }
}

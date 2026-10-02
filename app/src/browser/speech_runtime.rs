//! Canvas speech playback: the queue and controller one page drives, the
//! controls agent panels speak and narrate through, and the page lifecycle
//! hooks that stop or abandon playback.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use gloo_timers::future::TimeoutFuture;
use wasm_bindgen::JsCast as _;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

use super::connection::request_external_speech;
use super::watchband::TeamSpeech;
use super::{SurfaceData, toast};
use crate::AppRuntime;
use crate::speech::{SystemObservers, SystemStart};
use crate::speech_queue::{
    Advance, Enqueue, ExternalPause, NarrationCursors, NarrationMessage, SpeechEntry, SpeechQueue,
    SpeechRequest, SuppressAutomatic,
};

const NARRATION_CURSOR_STORAGE: &str = "spice:narration-cursors:v1";

/// How long a system request waits for the browser to publish its voices.
///
/// Engines enumerate voices once per page and announce the list with
/// `voiceschanged`; the wait covers that first-load window. An engine with no
/// voices never announces anything, so the wait needs an end.
const VOICE_PUBLICATION_DEADLINE_MILLIS: u32 = 5_000;

#[derive(Clone, Default, PartialEq)]
pub(super) enum SpeechStatus {
    #[default]
    Idle,
    Failed(String),
}

#[derive(Clone, PartialEq)]
pub(super) struct NarrationBatch {
    pub(super) team: String,
    pub(super) materialization: bool,
    pub(super) mode: TeamSpeech,
    pub(super) messages: Vec<NarrationMessage>,
}

#[derive(Clone)]
pub(super) struct AgentSpeechControls {
    pub(super) speak: Callback<SpeechRequest>,
    pub(super) narrate: Callback<NarrationBatch>,
    pub(super) suppress_automatic: Callback<()>,
    pub(super) message: UseStateHandle<Option<String>>,
}

#[derive(Clone)]
pub(super) struct SpeechRuntime {
    pub(super) queue: Rc<RefCell<SpeechQueue>>,
    pub(super) controller: Rc<RefCell<crate::speech::Controller>>,
    pub(super) status: UseStateHandle<SpeechStatus>,
    pub(super) message: UseStateHandle<Option<String>>,
}

impl SpeechRuntime {
    fn enqueue(&self, request: SpeechRequest) {
        let decision = self.queue.borrow_mut().enqueue(request);
        if let Enqueue::Start(entry) = decision {
            self.start(entry);
        }
    }

    fn toggle_message(&self, request: SpeechRequest) {
        let was_playing = self.queue.borrow().active_message_key() == request.message_key();
        self.stop();
        if !was_playing {
            self.enqueue(request);
        }
    }

    fn start(&self, entry: SpeechEntry) {
        let generation = self.controller.borrow_mut().begin();
        self.message.set(entry.message_key().map(str::to_owned));
        self.status.set(SpeechStatus::Idle);
        match entry.playback().clone() {
            wire::SpeechPlayback::Disabled {} => self.finish(entry.id(), None),
            wire::SpeechPlayback::System {
                voice,
                words_per_minute,
            } => self.start_system(entry, generation, &voice, words_per_minute),
            wire::SpeechPlayback::External {
                content_type,
                available,
                ..
            } => {
                if available {
                    self.start_external(entry, generation, content_type);
                } else {
                    self.finish(
                        entry.id(),
                        Some("external speech backend is unavailable".to_owned()),
                    );
                }
            }
        }
    }

    fn start_system(
        &self,
        entry: SpeechEntry,
        generation: u64,
        voice: &str,
        words_per_minute: u32,
    ) {
        let ended = self.clone();
        let failed = self.clone();
        let awaiting = self.clone();
        let id = entry.id();
        let resumed = entry.clone();
        let resumed_voice = voice.to_owned();
        let result = self.controller.borrow_mut().start_system(
            generation,
            entry.text(),
            voice,
            words_per_minute,
            SystemObservers {
                voices: move || {
                    awaiting.resume_system(
                        resumed.clone(),
                        generation,
                        resumed_voice.clone(),
                        words_per_minute,
                    );
                },
                ended: move || ended.defer_finish(id, None),
                failed: move |issue| {
                    failed.defer_finish(id, Some(issue));
                },
            },
        );
        match result {
            Ok(SystemStart::Speaking) => {}
            // The browser has published no voices yet. This entry stays the
            // loading queue head until `voiceschanged` resumes it, so the
            // first utterance of a page load speaks instead of refusing a
            // voice the browser had not enumerated.
            Ok(SystemStart::AwaitingVoices) => self.refuse_unpublished_voices(id, generation),
            Err(error) => self.finish(id, Some(error)),
        }
    }

    /// Retry system playback once the browser publishes its voice list.
    ///
    /// The resume runs one task after the event: the observing closure is
    /// owned by the controller, and re-entering [`Self::start_system`]
    /// directly would drop that closure while it is still running. Stopping
    /// or superseding the request in the meantime both detaches the observer
    /// and fails this ownership check, so nothing stale ever starts.
    fn resume_system(
        &self,
        entry: SpeechEntry,
        generation: u64,
        voice: String,
        words_per_minute: u32,
    ) {
        let runtime = self.clone();
        spawn_local(async move {
            TimeoutFuture::new(0).await;
            if !runtime.is_current(entry.id(), generation) {
                return;
            }
            runtime.start_system(entry, generation, &voice, words_per_minute);
        });
    }

    /// Refuse a request whose browser never publishes any voice.
    ///
    /// Waiting on `voiceschanged` is what lets a configured voice arrive
    /// late, but an engine carrying no voices at all never fires it. Without
    /// this deadline such a request would hold the queue head silently until
    /// a global stop, which reads as speech that vanished rather than speech
    /// this browser cannot give.
    fn refuse_unpublished_voices(&self, id: u64, generation: u64) {
        let runtime = self.clone();
        spawn_local(async move {
            TimeoutFuture::new(VOICE_PUBLICATION_DEADLINE_MILLIS).await;
            let awaiting = runtime.controller.borrow().is_awaiting_voices(generation);
            if !awaiting || !runtime.is_current(id, generation) {
                return;
            }
            runtime.finish(
                id,
                Some("this browser published no system voices".to_owned()),
            );
        });
    }

    fn start_external(&self, entry: SpeechEntry, generation: u64, content_type: String) {
        let runtime = self.clone();
        spawn_local(async move {
            let rendered =
                request_external_speech(entry.worktree(), entry.text(), &content_type).await;
            if !runtime.is_current(entry.id(), generation) {
                return;
            }
            let bytes = match rendered {
                Ok(bytes) => bytes,
                Err(error) => {
                    runtime.finish(entry.id(), Some(error));
                    return;
                }
            };
            let ended = runtime.clone();
            let failed = runtime.clone();
            let paused = runtime.clone();
            let id = entry.id();
            let playing = runtime.controller.borrow_mut().start_external(
                generation,
                &bytes,
                &content_type,
                move || ended.defer_finish(id, None),
                move || {
                    failed.defer_finish(id, Some("rendered speech playback failed".to_owned()));
                },
                move || paused.external_pause(id),
            );
            let playing = match playing {
                Ok(playing) => playing,
                Err(error) => {
                    runtime.finish(id, Some(error));
                    return;
                }
            };
            if let Err(error) = crate::speech::await_play(playing).await
                && runtime.is_current(id, generation)
            {
                runtime.finish(id, Some(error));
            }
        });
    }

    fn defer_finish(&self, id: u64, failure: Option<String>) {
        let runtime = self.clone();
        spawn_local(async move {
            TimeoutFuture::new(0).await;
            runtime.finish(id, failure);
        });
    }

    fn finish(&self, id: u64, failure: Option<String>) {
        let advance = self.queue.borrow_mut().finish(id);
        match advance {
            Advance::Ignored => {}
            Advance::Idle => {
                self.controller.borrow_mut().stop();
                self.message.set(None);
                self.status
                    .set(failure.map_or(SpeechStatus::Idle, SpeechStatus::Failed));
            }
            Advance::Start(entry) => self.start(entry),
        }
    }

    fn is_current(&self, id: u64, generation: u64) -> bool {
        self.queue.borrow().is_active(id) && self.controller.borrow().is_current(generation)
    }

    fn external_pause(&self, id: u64) {
        let pause = self.queue.borrow_mut().external_pause(id);
        match pause {
            ExternalPause::Ignored => {}
            ExternalPause::Stopped | ExternalPause::Idle => {
                self.controller.borrow_mut().stop();
                self.message.set(None);
                self.status.set(SpeechStatus::Idle);
            }
            ExternalPause::Start(entry) => self.start(entry),
        }
    }

    fn stop(&self) {
        self.queue.borrow_mut().stop();
        self.controller.borrow_mut().stop();
        self.message.set(None);
        self.status.set(SpeechStatus::Idle);
    }

    fn suppress_automatic(&self) {
        match self.queue.borrow_mut().suppress_automatic() {
            SuppressAutomatic::Preserved => {}
            SuppressAutomatic::Idle => {
                self.controller.borrow_mut().stop();
                self.message.set(None);
                self.status.set(SpeechStatus::Idle);
            }
            SuppressAutomatic::Start(entry) => self.start(entry),
        }
    }

    fn abandon_lane(&self, lane: &str) {
        self.queue.borrow_mut().abandon_lane(lane);
    }
}

#[hook]
pub(super) fn use_speech_failure_toast(speech_status: &UseStateHandle<SpeechStatus>) {
    let toaster = toast::use_toaster();
    use_effect_with((**speech_status).clone(), move |status| {
        if let SpeechStatus::Failed(issue) = status {
            toaster.settled(issue.clone());
        }
    });
}

#[hook]
pub(super) fn use_agent_speech_controls(
    runtime: Rc<RefCell<SpeechRuntime>>,
    narration_cursors: Rc<RefCell<NarrationCursors>>,
    message: UseStateHandle<Option<String>>,
) -> AgentSpeechControls {
    let enqueue = {
        let runtime = runtime.clone();
        use_memo((), move |_| {
            Callback::from(move |request: SpeechRequest| runtime.borrow().enqueue(request))
        })
    };
    let speak = {
        let runtime = runtime.clone();
        use_memo((), move |_| {
            Callback::from(move |request: SpeechRequest| runtime.borrow().toggle_message(request))
        })
    };
    let suppress_automatic = use_memo((), move |_| {
        Callback::from(move |()| runtime.borrow().suppress_automatic())
    });
    let narrate = {
        let enqueue = (*enqueue).clone();
        use_memo((), move |_| narration_callback(narration_cursors, enqueue))
    };
    AgentSpeechControls {
        speak: (*speak).clone(),
        narrate: (*narrate).clone(),
        suppress_automatic: (*suppress_automatic).clone(),
        message,
    }
}

/// Stop every clip the moment the page is hidden for navigation or close.
///
/// System synthesis runs in the operating system speech server, so an
/// utterance queued by this page keeps playing after the window that queued
/// it is gone unless the page cancels it on the way out. `pagehide` is the
/// last event every browser dispatches before a document is unloaded or
/// frozen, including tab and window close.
#[hook]
pub(super) fn use_page_hide_speech_stop(speech_runtime: SpeechRuntime) {
    use_effect_with((), move |()| {
        let registration = web_sys::window().and_then(|window| {
            let callback =
                Closure::<dyn FnMut(web_sys::Event)>::new(move |_event: web_sys::Event| {
                    speech_runtime.stop()
                });
            window
                .add_event_listener_with_callback("pagehide", callback.as_ref().unchecked_ref())
                .ok()
                .map(|()| (window, callback))
        });
        move || {
            if let Some((window, callback)) = registration {
                let _ = window.remove_event_listener_with_callback(
                    "pagehide",
                    callback.as_ref().unchecked_ref(),
                );
            }
        }
    });
}

#[hook]
pub(super) fn use_team_speech_abandonment(
    surface: &UseStateHandle<SurfaceData>,
    speech_runtime: SpeechRuntime,
) {
    let teams = surface
        .read()
        .as_ref()
        .and_then(AppRuntime::lanes)
        .map(|lanes| {
            lanes
                .lanes
                .iter()
                .map(|team| team.lane.to_string())
                .collect::<BTreeSet<_>>()
        });
    let previous = use_mut_ref(|| None::<BTreeSet<String>>);
    use_effect_with(teams, move |teams| {
        let Some(current) = teams else {
            return;
        };
        if let Some(previous) = previous.borrow().as_ref() {
            for removed in previous.difference(current) {
                speech_runtime.abandon_lane(removed);
            }
        }
        *previous.borrow_mut() = Some(current.clone());
    });
}

pub(super) fn load_narration_cursors() -> NarrationCursors {
    web_sys::window()
        .and_then(|window| window.local_storage().ok().flatten())
        .and_then(|storage| storage.get_item(NARRATION_CURSOR_STORAGE).ok().flatten())
        .and_then(|value| serde_json::from_str::<Vec<(String, String)>>(&value).ok())
        .map_or_else(NarrationCursors::default, NarrationCursors::restore)
}

fn store_narration_cursors(cursors: &NarrationCursors) {
    let Some(storage) = web_sys::window().and_then(|window| window.local_storage().ok().flatten())
    else {
        return;
    };
    let Ok(value) = serde_json::to_string(&cursors.rows()) else {
        return;
    };
    let _ = storage.set_item(NARRATION_CURSOR_STORAGE, &value);
}

fn narration_callback(
    cursors: Rc<RefCell<NarrationCursors>>,
    speak: Callback<SpeechRequest>,
) -> Callback<NarrationBatch> {
    Callback::from(move |batch: NarrationBatch| {
        let lines = {
            let mut cursors = cursors.borrow_mut();
            let lines = cursors.observe(batch.materialization, &batch.messages);
            store_narration_cursors(&cursors);
            lines
        };
        for line in lines {
            let text = match batch.mode {
                TeamSpeech::Quiet => None,
                TeamSpeech::Speak => line.automatic_speech,
                TeamSpeech::Narrate => line.narration,
            };
            if let Some(text) = text {
                speak.emit(SpeechRequest::narration(
                    line.worktree,
                    line.playback,
                    batch.team.clone(),
                    line.key,
                    text,
                ));
            }
        }
    })
}

//! System synthesis for each queued entry's admitted worktree voices.
//!
//! A page in the desktop shell speaks through the shell's own synthesizer (see
//! [`shell`]), which offers the voices WebKit withholds from
//! `speechSynthesis`; every other page speaks through the browser's.

mod shell;

use std::collections::HashMap;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{SpeechSynthesis, SpeechSynthesisUtterance, SpeechSynthesisVoice};

use crate::speech_owner::PlaybackOwnership;
use crate::speech_rate::system_rate;
use crate::voice_spread::{self, OfferedVoice};
use shell::ShellSpeech;
use wire::CanonicalUuid;

/// How many offered voice names a refusal names before eliding the rest, so
/// a browser carrying hundreds of voices still yields one readable sentence.
const MAX_OFFERED_VOICES: usize = 8;

/// What the browser did with one system-speech request.
pub enum SystemStart {
    /// The utterance is with the browser. `notice` is the premium gap an
    /// automatic spread stepped down from, carried once per page.
    Speaking { notice: Option<String> },
    /// The browser has published no voices yet, so no candidate can be
    /// matched and no utterance was started. The installed `voiceschanged`
    /// observer calls the caller's resume once the browser publishes its list.
    AwaitingVoices,
}

/// What one system utterance reports back to the playback owner.
pub struct SystemObservers<Voices, Ended, Failed> {
    /// The browser published its voice list, so the request can resume.
    pub voices: Voices,
    /// The utterance finished speaking.
    pub ended: Ended,
    /// The utterance failed, carrying the browser's own error code.
    pub failed: Failed,
}

/// Which voice one system request asks for.
#[derive(Clone, Copy)]
pub enum VoiceRequest<'a> {
    /// Configured voice names in preference order.
    Listed(&'a [String]),
    /// No names are configured: this roster member's spread voice. A Mac is
    /// expected to publish premium voices and is told when none applies.
    Spread {
        teams: &'a [(CanonicalUuid, Vec<CanonicalUuid>)],
        member: CanonicalUuid,
        premium_expected: bool,
    },
    /// No names are configured and the speaker is not on the roster.
    Default,
}

/// The synthesizer a page speaks through.
#[derive(Clone)]
enum Engine {
    Browser(SpeechSynthesis),
    Shell(ShellSpeech),
}

impl Engine {
    /// The desktop shell's synthesizer when it installed one, else the
    /// browser's.
    fn current() -> Result<Self, String> {
        let window = web_sys::window().ok_or_else(|| "browser window is unavailable".to_owned())?;
        if let Some(shell) = ShellSpeech::installed(&window) {
            return Ok(Self::Shell(shell));
        }
        window
            .speech_synthesis()
            .map(Self::Browser)
            .map_err(|_| "system speech is unavailable".to_owned())
    }

    fn voices(&self) -> Vec<Voice> {
        match self {
            Self::Browser(synthesis) => synthesis
                .get_voices()
                .iter()
                .filter_map(|value| value.dyn_into::<SpeechSynthesisVoice>().ok())
                .map(Voice::browser)
                .collect(),
            Self::Shell(shell) => shell.voices().into_iter().map(Voice::shell).collect(),
        }
    }

    fn observe_voices(&self, observer: Option<&js_sys::Function>) {
        match self {
            Self::Browser(synthesis) => synthesis.set_onvoiceschanged(observer),
            Self::Shell(shell) => shell.observe_voices(observer),
        }
    }

    fn cancel(&self) {
        match self {
            Self::Browser(synthesis) => synthesis.cancel(),
            Self::Shell(shell) => shell.cancel(),
        }
    }
}

/// One published voice, whichever synthesizer published it.
#[derive(Clone)]
struct Voice {
    name: String,
    /// The voice URI, or the shell's voice identifier.
    uri: String,
    lang: String,
    local: bool,
    default: bool,
    /// The browser's own record, which a browser utterance must be given.
    browser: Option<SpeechSynthesisVoice>,
}

impl Voice {
    fn browser(voice: SpeechSynthesisVoice) -> Self {
        Self {
            name: voice.name(),
            uri: voice.voice_uri(),
            lang: voice.lang(),
            local: voice.local_service(),
            default: voice.default(),
            browser: Some(voice),
        }
    }

    /// The shell's synthesizer serves every voice it lists on the device.
    fn shell(voice: shell::ShellVoice) -> Self {
        Self {
            name: voice.name,
            uri: voice.identifier,
            lang: voice.language,
            local: true,
            default: voice.default,
            browser: None,
        }
    }

    const fn offered(&self) -> OfferedVoice<'_> {
        OfferedVoice {
            name: self.name.as_str(),
            uri: self.uri.as_str(),
            lang: self.lang.as_str(),
            local: self.local,
            default: self.default,
        }
    }
}

/// One playback owner. Replacements always stop the prior utterance first.
#[derive(Default)]
pub struct Controller {
    ownership: PlaybackOwnership,
    engine: Option<Engine>,
    utterance: Option<SpeechSynthesisUtterance>,
    on_end: Option<Closure<dyn FnMut(JsValue)>>,
    on_error: Option<Closure<dyn FnMut(JsValue)>>,
    on_voices: Option<Closure<dyn FnMut(JsValue)>>,
    /// The voice each agent settled on; its search runs once per page.
    chosen: HashMap<String, Voice>,
    /// Whether the premium gap of an automatic spread was already surfaced.
    gap_noticed: bool,
}

impl Controller {
    /// Stop the current utterance, invalidate pending work, and begin one replacement.
    pub fn begin(&mut self) -> u64 {
        let generation = self.ownership.begin();
        self.release_media();
        generation
    }

    /// Whether an asynchronous result still belongs to this owner.
    pub const fn is_current(&self, generation: u64) -> bool {
        self.ownership.is_current(generation)
    }

    /// Whether this generation's request still waits on the browser's voice
    /// list. The installed observer is the wait: speaking detaches it, and
    /// so does every stop.
    pub const fn is_awaiting_voices(&self, generation: u64) -> bool {
        self.on_voices.is_some() && self.is_current(generation)
    }

    /// Stop current playback and invalidate pending work.
    pub fn stop(&mut self) {
        self.ownership.stop();
        self.release_media();
    }

    /// Release synthesizer resources without changing playback ownership.
    fn release_media(&mut self) {
        if let Some(utterance) = &self.utterance {
            utterance.set_onend(None);
            utterance.set_onerror(None);
        }
        if let Some(engine) = &self.engine {
            engine.observe_voices(None);
            engine.cancel();
        }
        self.engine = None;
        self.utterance = None;
        self.on_end = None;
        self.on_error = None;
        self.on_voices = None;
    }

    /// Speak `text` for `agent` through the page's system synthesis.
    ///
    /// A [`VoiceRequest::Listed`] request speaks with the first candidate this
    /// page publishes, which stays the agent's voice for the rest of the
    /// page. A [`VoiceRequest::Spread`] request speaks with the member's slot
    /// among the published voices sharing the chosen language, premium first,
    /// so desktop teammates sound different; the first step down from premium
    /// on a page is reported as the start's notice. Apple mobile callers and
    /// [`VoiceRequest::Default`] speak with the device's default voice.
    ///
    /// A synthesizer publishes its voice list asynchronously: on a fresh page
    /// the list is empty until the engine has enumerated the installed voices
    /// and fired `voiceschanged`. An empty list is therefore no verdict on the
    /// candidates; it installs the observer that resumes this request and
    /// reports [`SystemStart::AwaitingVoices`]. Only a published list carrying
    /// no listed candidate is refused.
    pub fn start_system<Voices, Ended, Failed>(
        &mut self,
        generation: u64,
        agent: &str,
        text: &str,
        request: VoiceRequest<'_>,
        words_per_minute: u32,
        observers: SystemObservers<Voices, Ended, Failed>,
    ) -> Result<SystemStart, String>
    where
        Voices: Fn() + 'static,
        Ended: Fn() + 'static,
        Failed: Fn(String) + 'static,
    {
        let SystemObservers {
            voices: on_voices,
            ended: on_end,
            failed: on_error,
        } = observers;
        if !self.is_current(generation) {
            return Err("speech request was superseded".to_owned());
        }
        let engine = Engine::current()?;
        let mut notice = None;
        let voice = match request {
            VoiceRequest::Default => None,
            VoiceRequest::Listed(voices) => match self.chosen.get(agent) {
                Some(voice) if voices.contains(&voice.name) => Some(voice.clone()),
                _ => {
                    let Some(offered) = self.published(&engine, on_voices) else {
                        return Ok(SystemStart::AwaitingVoices);
                    };
                    let voice = voices
                        .iter()
                        .find_map(|name| offered.iter().find(|voice| voice.name == *name))
                        .ok_or_else(|| unavailable_voices(voices, &offered))?;
                    self.chosen.insert(agent.to_owned(), voice.clone());
                    Some(voice.clone())
                }
            },
            VoiceRequest::Spread {
                teams,
                member,
                premium_expected,
            } => {
                let Some(offered) = self.published(&engine, on_voices) else {
                    return Ok(SystemStart::AwaitingVoices);
                };
                let (voice, gap) = spread_voice(&offered, teams, member, premium_expected);
                if let Some(gap) = gap.filter(|_| !self.gap_noticed) {
                    self.gap_noticed = true;
                    notice = Some(gap);
                }
                voice
            }
        };
        let rate = system_rate(words_per_minute);
        let ended = Closure::wrap(Box::new(move |_event: JsValue| on_end()) as Box<dyn FnMut(_)>);
        let failed = Closure::wrap(
            Box::new(move |event: JsValue| on_error(system_failure(&event))) as Box<dyn FnMut(_)>,
        );
        engine.observe_voices(None);
        match &engine {
            Engine::Browser(synthesis) => {
                let utterance = SpeechSynthesisUtterance::new_with_text(text)
                    .map_err(|_| "system speech utterance could not be created".to_owned())?;
                utterance.set_voice(voice.as_ref().and_then(|voice| voice.browser.as_ref()));
                utterance.set_rate(rate);
                utterance.set_onend(Some(ended.as_ref().unchecked_ref()));
                utterance.set_onerror(Some(failed.as_ref().unchecked_ref()));
                synthesis.speak(&utterance);
                self.utterance = Some(utterance);
            }
            Engine::Shell(shell) => {
                let handlers = js_sys::Object::new();
                for (name, handler) in [("onend", &ended), ("onerror", &failed)] {
                    js_sys::Reflect::set(&handlers, &JsValue::from_str(name), handler.as_ref())
                        .map_err(|_| "shell speech handlers could not be attached".to_owned())?;
                }
                let identifier = voice.as_ref().map_or("", |voice| voice.uri.as_str());
                shell.speak(text, identifier, rate, &handlers);
            }
        }
        self.engine = Some(engine);
        self.on_end = Some(ended);
        self.on_error = Some(failed);
        self.on_voices = None;
        Ok(SystemStart::Speaking { notice })
    }

    /// The voices `engine` publishes, or `None` once the observer that
    /// resumes the request on `voiceschanged` is installed.
    fn published(&mut self, engine: &Engine, on_voices: impl Fn() + 'static) -> Option<Vec<Voice>> {
        let offered = engine.voices();
        if !offered.is_empty() {
            return Some(offered);
        }
        let observed =
            Closure::wrap(Box::new(move |_event: JsValue| on_voices()) as Box<dyn FnMut(_)>);
        engine.observe_voices(Some(observed.as_ref().unchecked_ref()));
        self.engine = Some(engine.clone());
        self.on_voices = Some(observed);
        None
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The member's slot among the published voices sharing the chosen language,
/// premium first, with the gap when the pool sits below premium. A language
/// with no published voice yields `None`: the device default speaks.
fn spread_voice(
    offered: &[Voice],
    teams: &[(CanonicalUuid, Vec<CanonicalUuid>)],
    member: CanonicalUuid,
    premium_expected: bool,
) -> (Option<Voice>, Option<String>) {
    let described: Vec<OfferedVoice<'_>> = offered.iter().map(Voice::offered).collect();
    let language = web_sys::window().and_then(|window| window.navigator().language());
    let spread = voice_spread::pool(&described, language.as_deref(), premium_expected);
    let voice = voice_spread::voice_slot(teams, member, spread.pool.len())
        .map(|slot| offered[spread.pool[slot]].clone());
    (voice, spread.gap.map(|gap| gap.to_string()))
}

/// Refuse a voice list this device carries none of, naming what it offers.
///
/// Naming the published voices is what makes a mismatch fixable: the next
/// `say.voices` entry is one of these names. The list is bounded because a
/// browser can offer hundreds of voices and this sentence is shown whole.
fn unavailable_voices(voices: &[String], offered: &[Voice]) -> String {
    let mut names: Vec<String> = offered
        .iter()
        .take(MAX_OFFERED_VOICES)
        .map(|voice| voice.name.clone())
        .collect();
    if let Some(elided) = offered
        .len()
        .checked_sub(MAX_OFFERED_VOICES)
        .filter(|elided| *elided > 0)
    {
        names.push(format!("and {elided} more"));
    }
    let wanted: Vec<String> = voices.iter().map(|voice| format!("{voice:?}")).collect();
    format!(
        "none of the voices {} is on this device; this browser offers {}",
        wanted.join(", "),
        names.join(", "),
    )
}

/// Report a failed utterance with the synthesizer's own error code.
///
/// `SpeechSynthesisErrorEvent.error`, and the shell's `error` member alike,
/// distinguish a refused start from an interrupted one and from a voice the
/// engine could not load, which one generic sentence hides. The member is read
/// as a string rather than through the typed `SpeechSynthesisErrorCode`: an
/// engine reporting a code outside the enumerated set panics that conversion,
/// and an unenumerated code is exactly the one worth showing.
fn system_failure(event: &JsValue) -> String {
    let Some(code) = js_sys::Reflect::get(event, &JsValue::from_str("error"))
        .ok()
        .and_then(|code| code.as_string())
    else {
        return "system speech playback failed".to_owned();
    };
    format!("system speech playback failed: {code}")
}

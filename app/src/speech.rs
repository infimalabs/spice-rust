//! Browser system synthesis for each queued entry's admitted worktree voices.

use std::collections::HashMap;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Event, SpeechSynthesis, SpeechSynthesisUtterance, SpeechSynthesisVoice};

use crate::speech_owner::PlaybackOwnership;
use crate::speech_rate::system_rate;
use crate::voice_spread::{self, OfferedVoice};
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

/// One browser playback owner. Replacements always stop the prior utterance first.
#[derive(Default)]
pub struct Controller {
    ownership: PlaybackOwnership,
    synthesis: Option<SpeechSynthesis>,
    utterance: Option<SpeechSynthesisUtterance>,
    on_end: Option<Closure<dyn FnMut(Event)>>,
    on_error: Option<Closure<dyn FnMut(Event)>>,
    on_voices: Option<Closure<dyn FnMut(Event)>>,
    /// The voice each agent settled on; its search runs once per page.
    chosen: HashMap<String, SpeechSynthesisVoice>,
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

    /// Release browser resources without changing playback ownership.
    fn release_media(&mut self) {
        if let Some(utterance) = &self.utterance {
            utterance.set_onend(None);
            utterance.set_onerror(None);
        }
        if let Some(synthesis) = &self.synthesis {
            synthesis.set_onvoiceschanged(None);
            synthesis.cancel();
        }
        self.synthesis = None;
        self.utterance = None;
        self.on_end = None;
        self.on_error = None;
        self.on_voices = None;
    }

    /// Speak `text` for `agent` through the browser's system synthesis.
    ///
    /// A [`VoiceRequest::Listed`] request speaks with the first candidate this
    /// browser publishes, which stays the agent's voice for the rest of the
    /// page. A [`VoiceRequest::Spread`] request speaks with the member's slot
    /// among the published voices sharing the chosen language, premium first,
    /// so desktop teammates sound different; the first step down from premium
    /// on a page is reported as the start's notice. Apple mobile callers and
    /// [`VoiceRequest::Default`] speak with the device's default voice.
    ///
    /// A browser publishes its voice list asynchronously: on a fresh page
    /// `get_voices` answers an empty list until the engine has enumerated the
    /// installed voices and fired `voiceschanged`. An empty list is therefore
    /// no verdict on the candidates; it installs the observer that resumes this
    /// request and reports [`SystemStart::AwaitingVoices`]. Only a published
    /// list carrying no listed candidate is refused.
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
        let synthesis = web_sys::window()
            .ok_or_else(|| "browser window is unavailable".to_owned())?
            .speech_synthesis()
            .map_err(|_| "system speech is unavailable".to_owned())?;
        let mut notice = None;
        let voice = match request {
            VoiceRequest::Default => None,
            VoiceRequest::Listed(voices) => match self.chosen.get(agent) {
                Some(voice) if voices.contains(&voice.name()) => Some(voice.clone()),
                _ => {
                    let Some(offered) = self.published(&synthesis, on_voices) else {
                        return Ok(SystemStart::AwaitingVoices);
                    };
                    let voice = voices
                        .iter()
                        .find_map(|name| offered.iter().find(|voice| voice.name() == *name))
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
                let Some(offered) = self.published(&synthesis, on_voices) else {
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
        let utterance = SpeechSynthesisUtterance::new_with_text(text)
            .map_err(|_| "system speech utterance could not be created".to_owned())?;
        utterance.set_voice(voice.as_ref());
        utterance.set_rate(system_rate(words_per_minute));
        let ended = Closure::wrap(Box::new(move |_event: Event| on_end()) as Box<dyn FnMut(_)>);
        let failed = Closure::wrap(
            Box::new(move |event: Event| on_error(system_failure(&event))) as Box<dyn FnMut(_)>,
        );
        utterance.set_onend(Some(ended.as_ref().unchecked_ref()));
        utterance.set_onerror(Some(failed.as_ref().unchecked_ref()));
        synthesis.set_onvoiceschanged(None);
        synthesis.speak(&utterance);
        self.synthesis = Some(synthesis);
        self.utterance = Some(utterance);
        self.on_end = Some(ended);
        self.on_error = Some(failed);
        self.on_voices = None;
        Ok(SystemStart::Speaking { notice })
    }

    /// The voices this browser publishes, or `None` once the observer that
    /// resumes the request on `voiceschanged` is installed.
    fn published(
        &mut self,
        synthesis: &SpeechSynthesis,
        on_voices: impl Fn() + 'static,
    ) -> Option<Vec<SpeechSynthesisVoice>> {
        let offered: Vec<SpeechSynthesisVoice> = synthesis
            .get_voices()
            .iter()
            .filter_map(|value| value.dyn_into::<SpeechSynthesisVoice>().ok())
            .collect();
        if !offered.is_empty() {
            return Some(offered);
        }
        let observed =
            Closure::wrap(Box::new(move |_event: Event| on_voices()) as Box<dyn FnMut(_)>);
        synthesis.set_onvoiceschanged(Some(observed.as_ref().unchecked_ref()));
        self.synthesis = Some(synthesis.clone());
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
    offered: &[SpeechSynthesisVoice],
    teams: &[(CanonicalUuid, Vec<CanonicalUuid>)],
    member: CanonicalUuid,
    premium_expected: bool,
) -> (Option<SpeechSynthesisVoice>, Option<String>) {
    let traits: Vec<(String, String, String, bool, bool)> = offered
        .iter()
        .map(|voice| {
            (
                voice.name(),
                voice.voice_uri(),
                voice.lang(),
                voice.local_service(),
                voice.default(),
            )
        })
        .collect();
    let described: Vec<OfferedVoice<'_>> = traits
        .iter()
        .map(|(name, uri, lang, local, default)| OfferedVoice {
            name,
            uri,
            lang,
            local: *local,
            default: *default,
        })
        .collect();
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
fn unavailable_voices(voices: &[String], offered: &[SpeechSynthesisVoice]) -> String {
    let mut names: Vec<String> = offered
        .iter()
        .take(MAX_OFFERED_VOICES)
        .map(SpeechSynthesisVoice::name)
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

/// Report a failed utterance with the browser's own error code.
///
/// `SpeechSynthesisErrorEvent.error` distinguishes a refused start from an
/// interrupted one and from a voice the engine could not load, which one
/// generic sentence hides. The member is read as a string rather than through
/// the typed `SpeechSynthesisErrorCode`: an engine reporting a code outside
/// the enumerated set panics that conversion, and an unenumerated code is
/// exactly the one worth showing.
fn system_failure(event: &Event) -> String {
    let Some(code) = js_sys::Reflect::get(event, &JsValue::from_str("error"))
        .ok()
        .and_then(|code| code.as_string())
    else {
        return "system speech playback failed".to_owned();
    };
    format!("system speech playback failed: {code}")
}

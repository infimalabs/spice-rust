//! Browser-owned playback for each queued entry's admitted worktree speech contract.

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Blob, BlobPropertyBag, Event, HtmlAudioElement, SpeechSynthesis, SpeechSynthesisUtterance,
    SpeechSynthesisVoice, Url,
};

use crate::speech_owner::PlaybackOwnership;

const REFERENCE_WORDS_PER_MINUTE: f32 = 175.0;
const MIN_SYSTEM_RATE: f32 = 0.5;
const MAX_SYSTEM_RATE: f32 = 2.0;

/// How many offered voice names a refusal names before eliding the rest, so
/// a browser carrying hundreds of voices still yields one readable sentence.
const MAX_OFFERED_VOICES: usize = 8;

/// What the browser did with one system-speech request.
pub enum SystemStart {
    /// The utterance is with the browser.
    Speaking,
    /// The browser has published no voices yet, so no voice can be selected
    /// and no utterance was started. The installed `voiceschanged` observer
    /// calls the caller's resume once the browser publishes its list.
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

/// One browser playback owner. Replacements always stop the prior media first.
#[derive(Default)]
pub struct Controller {
    ownership: PlaybackOwnership,
    synthesis: Option<SpeechSynthesis>,
    utterance: Option<SpeechSynthesisUtterance>,
    audio: Option<HtmlAudioElement>,
    audio_url: Option<String>,
    on_end: Option<Closure<dyn FnMut(Event)>>,
    on_error: Option<Closure<dyn FnMut(Event)>>,
    on_pause: Option<Closure<dyn FnMut(Event)>>,
    on_voices: Option<Closure<dyn FnMut(Event)>>,
}

impl Controller {
    /// Stop current media, invalidate pending fetches, and begin one replacement.
    pub fn begin(&mut self) -> u64 {
        let generation = self.ownership.begin();
        self.release_media();
        generation
    }

    /// Whether an asynchronous renderer result still belongs to this owner.
    pub const fn is_current(&self, generation: u64) -> bool {
        self.ownership.is_current(generation)
    }

    /// Whether this generation's request still waits on the browser's voice
    /// list. The installed observer is the wait: speaking detaches it, and
    /// so does every stop.
    pub const fn is_awaiting_voices(&self, generation: u64) -> bool {
        self.on_voices.is_some() && self.is_current(generation)
    }

    /// Stop current system or external playback and invalidate pending work.
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
        if let Some(audio) = &self.audio {
            audio.set_onended(None);
            audio.set_onerror(None);
            audio.set_onpause(None);
            let _ = audio.pause();
            audio.set_current_time(0.0);
        }
        if let Some(url) = self.audio_url.take() {
            let _ = Url::revoke_object_url(&url);
        }
        self.synthesis = None;
        self.utterance = None;
        self.audio = None;
        self.on_end = None;
        self.on_error = None;
        self.on_pause = None;
        self.on_voices = None;
    }

    /// Start exact-voice system synthesis for the current generation.
    ///
    /// A browser publishes its voice list asynchronously: on a fresh page
    /// `get_voices` answers an empty list until the engine has enumerated the
    /// installed voices and fired `voiceschanged`. An empty list is therefore
    /// no verdict on the configured voice, and refusing on it is what makes a
    /// present voice look intermittently unavailable. Only a published list
    /// that omits the configured name is a naming mismatch; an empty one
    /// installs the observer that resumes this request and reports
    /// [`SystemStart::AwaitingVoices`], leaving the entry the queue head.
    pub fn start_system<Voices, Ended, Failed>(
        &mut self,
        generation: u64,
        text: &str,
        voice_name: &str,
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
            .map_err(|_| "system speech backend is unavailable".to_owned())?;
        let offered: Vec<SpeechSynthesisVoice> = synthesis
            .get_voices()
            .iter()
            .filter_map(|value| value.dyn_into::<SpeechSynthesisVoice>().ok())
            .collect();
        let Some(voice) = offered.iter().find(|voice| voice.name() == voice_name) else {
            if !offered.is_empty() {
                return Err(unavailable_voice(voice_name, &offered));
            }
            let observed =
                Closure::wrap(Box::new(move |_event: Event| on_voices()) as Box<dyn FnMut(_)>);
            synthesis.set_onvoiceschanged(Some(observed.as_ref().unchecked_ref()));
            self.synthesis = Some(synthesis);
            self.on_voices = Some(observed);
            return Ok(SystemStart::AwaitingVoices);
        };
        let utterance = SpeechSynthesisUtterance::new_with_text(text)
            .map_err(|_| "system speech utterance could not be created".to_owned())?;
        utterance.set_voice(Some(voice));
        utterance.set_rate(
            (words_per_minute as f32 / REFERENCE_WORDS_PER_MINUTE)
                .clamp(MIN_SYSTEM_RATE, MAX_SYSTEM_RATE),
        );
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
        Ok(SystemStart::Speaking)
    }

    /// Install externally rendered bytes and ask the browser to play them.
    pub fn start_external(
        &mut self,
        generation: u64,
        bytes: &[u8],
        content_type: &str,
        on_end: impl Fn() + 'static,
        on_error: impl Fn() + 'static,
        on_pause: impl Fn() + 'static,
    ) -> Result<js_sys::Promise, String> {
        if !self.is_current(generation) {
            return Err("speech request was superseded".to_owned());
        }
        let parts = js_sys::Array::new();
        parts.push(&js_sys::Uint8Array::from(bytes));
        let options = BlobPropertyBag::new();
        options.set_type(content_type);
        let blob = Blob::new_with_u8_array_sequence_and_options(&parts, &options)
            .map_err(|_| "rendered speech bytes were not browser-playable".to_owned())?;
        let url = Url::create_object_url_with_blob(&blob)
            .map_err(|_| "rendered speech URL could not be created".to_owned())?;
        let audio = HtmlAudioElement::new_with_src(&url)
            .map_err(|_| "browser audio backend is unavailable".to_owned())?;
        let ended = Closure::wrap(Box::new(move |_event: Event| on_end()) as Box<dyn FnMut(_)>);
        let failed = Closure::wrap(Box::new(move |_event: Event| on_error()) as Box<dyn FnMut(_)>);
        let paused_audio = audio.clone();
        let paused = Closure::wrap(Box::new(move |_event: Event| {
            if !paused_audio.ended() {
                on_pause();
            }
        }) as Box<dyn FnMut(_)>);
        audio.set_onended(Some(ended.as_ref().unchecked_ref()));
        audio.set_onerror(Some(failed.as_ref().unchecked_ref()));
        audio.set_onpause(Some(paused.as_ref().unchecked_ref()));
        let playing = audio
            .play()
            .map_err(|_| "browser refused rendered speech playback".to_owned())?;
        self.audio = Some(audio);
        self.audio_url = Some(url);
        self.on_end = Some(ended);
        self.on_error = Some(failed);
        self.on_pause = Some(paused);
        Ok(playing)
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Refuse an absent configured voice while naming what the browser published.
///
/// The operator configured one exact voice and gets that voice or silence:
/// naming the published alternatives is what makes a mismatch diagnosable
/// without a substitution nobody asked for. System synthesis runs on the
/// viewing device, so a phone offers its own voices rather than the
/// resident's; the refusal says so and names the backend that renders on
/// the resident instead. The list is bounded because a browser can offer
/// hundreds of voices and this sentence is shown whole.
fn unavailable_voice(voice_name: &str, offered: &[SpeechSynthesisVoice]) -> String {
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
    format!(
        "system voice {voice_name:?} is not on this device; system speech uses the viewing \
         device's own voices, and this browser offers {}. To hear the resident's voices on \
         every device, set say.backend = \"external\" with the say.command renderer in \
         docs/product/interfaces.md (Speech)",
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

/// Await the browser's asynchronous media-play admission.
pub async fn await_play(promise: js_sys::Promise) -> Result<(), String> {
    JsFuture::from(promise)
        .await
        .map(|_| ())
        .map_err(|_| "browser refused rendered speech playback".to_owned())
}

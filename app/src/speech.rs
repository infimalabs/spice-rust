//! Browser-owned playback for the effective resident speech contract.

use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Blob, BlobPropertyBag, Event, HtmlAudioElement, HtmlElement, SpeechSynthesis,
    SpeechSynthesisUtterance, Url,
};

const REFERENCE_WORDS_PER_MINUTE: f32 = 175.0;
const MIN_SYSTEM_RATE: f32 = 0.5;
const MAX_SYSTEM_RATE: f32 = 2.0;
const MAX_RENDERED_TEXT_BYTES: usize = 32 * 1_024;

/// One browser playback owner. Replacements always stop the prior media first.
#[derive(Default)]
pub struct Controller {
    generation: u64,
    synthesis: Option<SpeechSynthesis>,
    utterance: Option<SpeechSynthesisUtterance>,
    audio: Option<HtmlAudioElement>,
    audio_url: Option<String>,
    on_end: Option<Closure<dyn FnMut(Event)>>,
    on_error: Option<Closure<dyn FnMut(Event)>>,
}

impl Controller {
    /// Stop current media, invalidate pending fetches, and begin one replacement.
    pub fn begin(&mut self) -> u64 {
        self.stop();
        self.generation
    }

    /// Whether an asynchronous renderer result still belongs to this owner.
    pub const fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
    }

    /// Stop current system or external playback and invalidate pending work.
    pub fn stop(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(utterance) = &self.utterance {
            utterance.set_onend(None);
            utterance.set_onerror(None);
        }
        if let Some(synthesis) = &self.synthesis {
            synthesis.cancel();
        }
        if let Some(audio) = &self.audio {
            audio.set_onended(None);
            audio.set_onerror(None);
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
    }

    /// Start exact-voice system synthesis for the current generation.
    pub fn start_system(
        &mut self,
        generation: u64,
        text: &str,
        voice_name: &str,
        words_per_minute: u32,
        on_end: impl Fn() + 'static,
        on_error: impl Fn() + 'static,
    ) -> Result<(), String> {
        if !self.is_current(generation) {
            return Err("speech request was superseded".to_owned());
        }
        let synthesis = web_sys::window()
            .ok_or_else(|| "browser window is unavailable".to_owned())?
            .speech_synthesis()
            .map_err(|_| "system speech backend is unavailable".to_owned())?;
        let voice = synthesis
            .get_voices()
            .iter()
            .filter_map(|value| value.dyn_into::<web_sys::SpeechSynthesisVoice>().ok())
            .find(|voice| voice.name() == voice_name)
            .ok_or_else(|| format!("system voice {voice_name:?} is unavailable"))?;
        let utterance = SpeechSynthesisUtterance::new_with_text(text)
            .map_err(|_| "system speech utterance could not be created".to_owned())?;
        utterance.set_voice(Some(&voice));
        utterance.set_rate(
            (words_per_minute as f32 / REFERENCE_WORDS_PER_MINUTE)
                .clamp(MIN_SYSTEM_RATE, MAX_SYSTEM_RATE),
        );
        let ended = Closure::wrap(Box::new(move |_event: Event| on_end()) as Box<dyn FnMut(_)>);
        let failed = Closure::wrap(Box::new(move |_event: Event| on_error()) as Box<dyn FnMut(_)>);
        utterance.set_onend(Some(ended.as_ref().unchecked_ref()));
        utterance.set_onerror(Some(failed.as_ref().unchecked_ref()));
        synthesis.cancel();
        synthesis.speak(&utterance);
        self.synthesis = Some(synthesis);
        self.utterance = Some(utterance);
        self.on_end = Some(ended);
        self.on_error = Some(failed);
        Ok(())
    }

    /// Install externally rendered bytes and ask the browser to play them.
    pub fn start_external(
        &mut self,
        generation: u64,
        bytes: &[u8],
        content_type: &str,
        on_end: impl Fn() + 'static,
        on_error: impl Fn() + 'static,
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
        audio.set_onended(Some(ended.as_ref().unchecked_ref()));
        audio.set_onerror(Some(failed.as_ref().unchecked_ref()));
        let playing = audio
            .play()
            .map_err(|_| "browser refused rendered speech playback".to_owned())?;
        self.audio = Some(audio);
        self.audio_url = Some(url);
        self.on_end = Some(ended);
        self.on_error = Some(failed);
        Ok(playing)
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Plain text currently visible in the canvas content region.
pub fn rendered_text() -> Result<String, String> {
    let element = web_sys::window()
        .and_then(|window| window.document())
        .ok_or_else(|| "canvas document is unavailable".to_owned())?
        .query_selector("main.content")
        .map_err(|_| "canvas content could not be inspected".to_owned())?
        .ok_or_else(|| "canvas content is unavailable".to_owned())?
        .dyn_into::<HtmlElement>()
        .map_err(|_| "canvas content is not readable".to_owned())?;
    let text = element.inner_text();
    let text = text.trim();
    if text.is_empty() {
        return Err("canvas content is empty".to_owned());
    }
    if text.len() > MAX_RENDERED_TEXT_BYTES {
        return Err("canvas content exceeds the speech limit".to_owned());
    }
    Ok(text.to_owned())
}

/// Await the browser's asynchronous media-play admission.
pub async fn await_play(promise: js_sys::Promise) -> Result<(), String> {
    JsFuture::from(promise)
        .await
        .map(|_| ())
        .map_err(|_| "browser refused rendered speech playback".to_owned())
}

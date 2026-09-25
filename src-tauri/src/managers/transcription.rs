//! Transcription orchestration — backed by the online Google speech engine
//! (see [`crate::stt`]). No local models are loaded, so "model loading"
//! degenerates to marking the single built-in engine ready.

use crate::audio_toolkit::{
    apply_custom_words, detect_output_language, normalize_transcription_output,
    remove_filler_words, OutputLanguageEvidence,
};
use crate::stt::engine::{self, SessionFeed, StreamingSession};
use crate::stt::InterimText;
use crate::managers::model::{ModelManager, ONLINE_MODEL_ID};
use crate::settings::{get_settings, AppSettings};
use anyhow::Result;
use log::{debug, error, info, warn};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex, MutexGuard};
use tauri::{AppHandle, Emitter};

#[derive(Clone, Debug, Serialize)]
pub struct ModelStateEvent {
    pub event_type: String,
    pub model_id: Option<String>,
    pub model_name: Option<String>,
    pub error: Option<String>,
}

/// Live transcription snapshot emitted to the overlay during a streaming run.
#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct StreamTextEvent {
    pub committed: String,
    pub tentative: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum StreamPhase {
    Listening,
    Working,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "lowercase")]
pub enum StreamWorkKind {
    Transcribing,
    Polishing,
}

#[derive(Clone, Debug, Serialize, Deserialize, Type, tauri_specta::Event)]
pub struct StreamPhaseEvent {
    pub phase: StreamPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<StreamWorkKind>,
}

/// Routes real-time audio frames to the live speech session while recording.
/// The recorder holds this unconditionally; with no session attached every
/// frame costs a single relaxed check.
pub struct StreamRouter {
    feed: Mutex<Option<SessionFeed>>,
}

impl StreamRouter {
    fn new() -> Self {
        Self {
            feed: Mutex::new(None),
        }
    }

    fn attach(&self, feed: SessionFeed) {
        *self.feed.lock().unwrap() = Some(feed);
    }

    fn detach(&self) {
        *self.feed.lock().unwrap() = None;
    }

    /// Forward a 16 kHz frame to the live session, if one is recording.
    pub fn feed(&self, frame: &[f32]) {
        if let Some(feed) = self.feed.lock().unwrap().as_ref() {
            feed.push(frame);
        }
    }

    pub fn is_open(&self) -> bool {
        self.feed.lock().unwrap().is_some()
    }
}

/// RAII guard kept for call-site compatibility; loading is instantaneous.
pub struct LoadingGuard;

#[derive(Clone)]
pub struct TranscriptionManager {
    model_manager: Arc<ModelManager>,
    app_handle: AppHandle,
    current_model_id: Arc<Mutex<Option<String>>>,
    router: Arc<StreamRouter>,
    session: Arc<Mutex<Option<StreamingSession>>>,
}

impl TranscriptionManager {
    pub fn new(app_handle: &AppHandle, model_manager: Arc<ModelManager>) -> Result<Self> {
        let manager = Self {
            model_manager,
            app_handle: app_handle.clone(),
            current_model_id: Arc::new(Mutex::new(None)),
            router: Arc::new(StreamRouter::new()),
            session: Arc::new(Mutex::new(None)),
        };
        // The online engine is always available; mark it ready at startup so
        // the UI and tray see a loaded "model" immediately.
        manager.load_model(ONLINE_MODEL_ID)?;
        Ok(manager)
    }

    fn lock_model(&self) -> MutexGuard<'_, Option<String>> {
        self.current_model_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn is_model_loaded(&self) -> bool {
        true
    }

    pub fn reload_model_on_next_use(&self) {}

    pub fn try_start_loading(&self) -> Option<LoadingGuard> {
        Some(LoadingGuard)
    }

    pub fn unload_model(&self) -> Result<()> {
        // Nothing to free; keep the built-in engine marked as loaded.
        Ok(())
    }

    pub fn maybe_unload_immediately(&self, _context: &str) {}

    pub fn load_model(&self, model_id: &str) -> Result<()> {
        self.load_model_with_device(model_id, None)
    }

    pub fn load_model_with_device(
        &self,
        model_id: &str,
        _device_index: Option<usize>,
    ) -> Result<()> {
        if model_id != ONLINE_MODEL_ID {
            return Err(anyhow::anyhow!(
                "Unknown model: {model_id}. Sokhan ships only the built-in online engine."
            ));
        }
        let info = self
            .model_manager
            .get_model_info(ONLINE_MODEL_ID)
            .ok_or_else(|| anyhow::anyhow!("Online model is not registered"))?;
        *self.lock_model() = Some(model_id.to_string());
        let _ = self.app_handle.emit(
            "model-state-changed",
            ModelStateEvent {
                event_type: "loaded".to_string(),
                model_id: Some(model_id.to_string()),
                model_name: Some(info.name.clone()),
                error: None,
            },
        );
        info!("Online Google speech engine ready (model: {})", info.name);
        Ok(())
    }

    pub fn initiate_model_load(&self) {
        if self.lock_model().is_none() {
            let _ = self.load_model(ONLINE_MODEL_ID);
        }
    }

    pub fn get_current_model(&self) -> Option<String> {
        self.lock_model().clone()
    }

    pub fn current_backend(&self) -> Option<String> {
        Some("google-online".to_string())
    }

    pub fn is_streaming(&self) -> bool {
        self.session.lock().unwrap().is_some()
    }

    pub fn stream_router(&self) -> Arc<StreamRouter> {
        Arc::clone(&self.router)
    }

    /// Open a live Google session for the recording that just started. Audio
    /// frames flow through the [`StreamRouter`]; interim text is pushed to
    /// the overlay as [`StreamTextEvent`]s (the live subtitles).
    pub fn start_stream(&self) {
        let settings = get_settings(&self.app_handle);
        let language = effective_language_for_model(
            &settings,
            &self.model_manager,
            &settings.selected_model,
        );
        let app = self.app_handle.clone();
        let interim = Some(Box::new(move |text: InterimText| {
            let _ = app.emit(
                "stream-text-event",
                StreamTextEvent {
                    committed: text.committed,
                    tentative: text.tentative,
                },
            );
        }) as crate::stt::engine::InterimCb);
        let session = engine::start(&language, interim);
        match session.feed_handle() {
            Some(feed) => {
                self.router.attach(feed);
                *self.session.lock().unwrap() = Some(session);
                debug!("Live Google speech session started (lang={language})");
            }
            None => warn!("speech session failed to open; falling back to batch"),
        }
    }

    /// Close the live session and return its transcript. `Ok(None)` means
    /// the caller should batch-transcribe the recorded audio instead.
    pub fn finalize_stream(&self) -> Result<Option<String>> {
        self.router.detach();
        let session = self.session.lock().unwrap().take();
        let Some(session) = session else {
            return Ok(None);
        };
        match session.finish() {
            Ok(text) if !text.trim().is_empty() => Ok(Some(text)),
            Ok(_) => {
                debug!("Live session produced no text; falling back to batch");
                Ok(None)
            }
            Err(e) => {
                error!("Live session failed ({e}); falling back to batch");
                Ok(None)
            }
        }
    }

    pub fn cancel_stream(&self) {
        self.router.detach();
        // Dropping the session ends its upload and abandons results.
        let _ = self.session.lock().unwrap().take();
    }

    pub fn emit_stream_working(&self, kind: StreamWorkKind) {
        let _ = self.app_handle.emit(
            "stream-phase-event",
            StreamPhaseEvent {
                phase: StreamPhase::Working,
                kind: Some(kind),
            },
        );
    }

    pub fn transcribe(&self, audio: Vec<f32>) -> Result<String> {
        #[cfg(debug_assertions)]
        if std::env::var("SOKHAN_FORCE_TRANSCRIPTION_FAILURE").is_ok() {
            return Err(anyhow::anyhow!(
                "Simulated transcription failure (SOKHAN_FORCE_TRANSCRIPTION_FAILURE)"
            ));
        }

        let st = std::time::Instant::now();
        let audio_len = audio.len();
        debug!("Audio vector length: {}", audio_len);

        if audio.is_empty() {
            debug!("Empty audio vector");
            return Ok(String::new());
        }

        let settings = get_settings(&self.app_handle);
        let active_model = self
            .get_current_model()
            .unwrap_or_else(|| settings.selected_model.clone());
        let validated_language =
            effective_language_for_model(&settings, self.model_manager.as_ref(), &active_model);
        let model_languages = self
            .model_manager
            .get_model_info(&active_model)
            .map(|info| info.supported_languages)
            .unwrap_or_default();

        let raw = engine::transcribe(&audio, &validated_language)?;

        let output_language = resolve_output_language_evidence(
            &settings,
            Some(validated_language.as_str()),
            &model_languages,
            false,
        );
        debug!("Output language evidence: {:?}", output_language);

        let filtered_result = post_process_transcription_text(
            raw,
            &settings,
            false,
            &output_language,
            &model_languages,
        );

        let elapsed_secs = st.elapsed().as_secs_f64();
        let audio_secs = audio_len as f64 / 16_000.0;
        info!(
            "Online transcription completed in {:.2}s for {:.2}s of audio",
            elapsed_secs, audio_secs
        );

        let filtered_result = if settings.fa_halfspace_enabled
            && validated_language.starts_with("fa")
        {
            crate::audio_toolkit::apply_persian_halfspace(&filtered_result)
        } else {
            filtered_result
        };

        if filtered_result.is_empty() {
            info!("Transcription result is empty");
        } else {
            info!(
                "Transcription result: {}",
                crate::utils::redact_text(&filtered_result)
            );
        }

        Ok(filtered_result)
    }
}

fn normalize_cjk_language(language: &str) -> &str {
    match language {
        "zh-Hans" | "zh-Hant" => "zh",
        other => other,
    }
}

/// Resolve the persisted language intent into the BCP-47 tag the Google
/// engine receives.
pub(crate) fn effective_language_for_model(
    settings: &AppSettings,
    model_manager: &ModelManager,
    model_id: &str,
) -> String {
    let resolved = match model_manager.get_model_info(model_id) {
        Some(info) => crate::managers::model::effective_language(
            &settings.selected_language,
            &info.supported_languages,
            info.supports_language_detection,
        ),
        None => settings.selected_language.clone(),
    };
    match resolved.as_str() {
        // The endpoint expects full tags; a bare "zh" collapses to Simplified.
        "zh" => "zh-CN".to_string(),
        other => normalize_cjk_language(other).to_string(),
    }
}

fn resolve_output_language_evidence(
    settings: &AppSettings,
    applied_language_hint: Option<&str>,
    supported_languages: &[String],
    translated_to_english: bool,
) -> OutputLanguageEvidence {
    if translated_to_english {
        return OutputLanguageEvidence::TranslatedToEnglish;
    }

    if let Some(language) = applied_language_hint.filter(|lang| !lang.is_empty() && *lang != "auto")
    {
        if settings.selected_language != "auto"
            && crate::managers::model::canonical_language_code(&settings.selected_language)
                == crate::managers::model::canonical_language_code(language)
        {
            return OutputLanguageEvidence::UserSelected(language.to_string());
        }
        return OutputLanguageEvidence::ModelConstrained(language.to_string());
    }

    if let [language] = supported_languages {
        return OutputLanguageEvidence::ModelConstrained(language.clone());
    }

    OutputLanguageEvidence::Unknown
}

fn post_process_transcription_text(
    raw: String,
    settings: &AppSettings,
    custom_words_already_prompted: bool,
    output_language: &OutputLanguageEvidence,
    supported_languages: &[String],
) -> String {
    fail_open_text_transform(raw, |raw| {
        let corrected = if !settings.custom_words.is_empty() && !custom_words_already_prompted {
            apply_custom_words(
                &raw,
                &settings.custom_words,
                settings.word_correction_threshold,
            )
        } else {
            raw
        };

        let output_language = match output_language {
            OutputLanguageEvidence::Unknown
                if settings.filler_word_removal_enabled
                    && settings.custom_filler_words.is_none() =>
            {
                match detect_output_language(&corrected, supported_languages) {
                    Some(language) => {
                        debug!("Text-based language detection resolved '{}'", language);
                        OutputLanguageEvidence::TextDetected(language)
                    }
                    None => OutputLanguageEvidence::Unknown,
                }
            }
            other => other.clone(),
        };

        let without_fillers = remove_filler_words(
            &corrected,
            &output_language,
            &settings.custom_filler_words,
            settings.filler_word_removal_enabled,
        );

        normalize_transcription_output(&without_fillers)
    })
}

fn fail_open_text_transform<F>(raw: String, transform: F) -> String
where
    F: FnOnce(String) -> String,
{
    let fallback = raw.clone();
    match catch_unwind(AssertUnwindSafe(|| transform(raw))) {
        Ok(processed) => processed,
        Err(payload) => {
            error!(
                "Optional transcription text post-processing panicked: {}; using the raw transcription",
                panic_payload_message(payload.as_ref())
            );
            fallback
        }
    }
}

fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    }
}

// --------------- Compatibility shims for removed GPU/model machinery -------

pub fn init_transcribe_backend() {}

pub fn describe_compute_devices() -> Vec<String> {
    Vec::new()
}

pub fn apply_accelerator_settings(_app: &tauri::AppHandle) {}

#[derive(Serialize, Clone, Debug, Type)]
pub struct GpuDeviceOption {
    pub id: String,
    pub name: String,
    pub total_vram_mb: usize,
}

#[derive(Serialize, Clone, Debug, Type)]
pub struct AvailableAccelerators {
    pub transcribe: Vec<String>,
    pub ort: Vec<String>,
    pub gpu_devices: Vec<GpuDeviceOption>,
}

pub fn get_available_accelerators() -> AvailableAccelerators {
    AvailableAccelerators {
        transcribe: vec!["auto".to_string()],
        ort: vec!["auto".to_string()],
        gpu_devices: Vec::new(),
    }
}

//! Model registry for Sokhan. There are no downloadable models: the app
//! ships exactly one built-in engine — Google's free online speech
//! recognition (see [`crate::google_speech`]). This manager exists to keep
//! the command/event surface (and the frontend stores) unchanged.

use crate::settings::get_settings;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use specta::Type;
use std::collections::HashSet;
use tauri::AppHandle;

pub const ONLINE_MODEL_ID: &str = "google-online";

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Eq)]
pub enum EngineType {
    /// Google's online speech engine (Chromium Web Speech API backend).
    GoogleOnline,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub enum ModelSource {
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub filename: String,
    pub source: ModelSource,
    pub size_mb: u64,
    pub is_downloaded: bool,
    pub is_downloading: bool,
    pub partial_size: u64,
    pub is_directory: bool,
    pub engine_type: EngineType,
    pub accuracy_score: f32,
    pub speed_score: f32,
    pub supports_translation: bool,
    pub is_recommended: bool,
    pub supported_languages: Vec<String>,
    pub supports_language_selection: bool,
    pub is_custom: bool,
    pub supports_streaming: bool,
    pub supports_language_detection: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct DownloadProgress {
    pub model_id: String,
    pub downloaded: u64,
    pub total: u64,
    pub percentage: f64,
}

const CHINESE_LANGUAGE_CODE: &str = "zh";

fn recognition_language(language: &str) -> &str {
    match language {
        "zh-Hans" | "zh-Hant" => CHINESE_LANGUAGE_CODE,
        other => other,
    }
}

/// A tag's primary language subtag, with any BCP-47 region/script suffix
/// dropped (`en-US` → `en`, `zh-Hant` → `zh`).
fn base_language(language: &str) -> &str {
    language.split(&['-', '_'][..]).next().unwrap_or(language)
}

pub(crate) fn canonical_language_code(language: &str) -> &str {
    match base_language(language) {
        "nb" => "no",
        "fil" => "tl",
        base => base,
    }
}

/// Resolve the user's persisted language intent (`"auto"` or a language
/// code) into the language the engine will actually use. Computed at the
/// point of use and never written back to settings.
pub fn effective_language(
    intent: &str,
    supported_languages: &[String],
    supports_language_detection: bool,
) -> String {
    if supported_languages.is_empty() {
        return intent.to_string();
    }

    if intent != "auto" {
        let exact_base_match = supported_languages
            .iter()
            .find(|language| base_language(language) == base_language(intent));
        let equivalent_match = || {
            supported_languages.iter().find(|language| {
                canonical_language_code(language) == canonical_language_code(intent)
            })
        };

        if let Some(code) = exact_base_match.or_else(equivalent_match) {
            if intent == "zh-Hans" || intent == "zh-Hant" {
                return intent.to_string();
            }
            return code.clone();
        }
    }

    if supports_language_detection {
        return "auto".to_string();
    }

    if let Some(en) = supported_languages
        .iter()
        .find(|language| base_language(language) == "en")
    {
        return en.clone();
    }
    recognition_language(&supported_languages[0]).to_string()
}

fn online_model_info() -> ModelInfo {
    let languages: Vec<String> = [
        "en-US", "fa-IR", "ar-SA", "de-DE", "es-ES", "es-MX", "fr-FR", "fr-CA", "hi-IN", "it-IT",
        "ja-JP", "ko-KR", "nl-NL", "pl-PL", "pt-BR", "pt-PT", "ru-RU", "tr-TR", "id-ID", "th-TH",
        "vi-VN", "uk-UA", "el-GR", "he-IL", "bn-BD", "ta-IN", "te-IN", "mr-IN", "ms-MY", "sv-SE",
        "nb-NO", "da-DK", "fi-FI", "cs-CZ", "ro-RO", "hu-HU", "zh-CN", "zh-TW",
    ]
    .iter()
    .map(|code| code.to_string())
    .collect();
    let seen: HashSet<&str> = languages.iter().map(|l| base_language(l)).collect();
    debug_assert!(!seen.is_empty());

    ModelInfo {
        id: ONLINE_MODEL_ID.to_string(),
        name: "Google Online".to_string(),
        description: "Online Google speech recognition — the same engine Chrome's voice input \
                      uses. No download needed; requires an internet connection."
            .to_string(),
        filename: String::new(),
        source: ModelSource::Local,
        size_mb: 0,
        is_downloaded: true,
        is_downloading: false,
        partial_size: 0,
        is_directory: false,
        engine_type: EngineType::GoogleOnline,
        accuracy_score: 0.90,
        speed_score: 0.85,
        supports_translation: false,
        is_recommended: true,
        supported_languages: languages,
        supports_language_selection: true,
        is_custom: false,
        supports_streaming: true,
        supports_language_detection: false,
    }
}

pub struct ModelManager {
    app_handle: AppHandle,
}

impl ModelManager {
    pub fn new(app_handle: &AppHandle) -> Result<Self> {
        Ok(Self {
            app_handle: app_handle.clone(),
        })
    }

    pub fn get_available_models(&self) -> Vec<ModelInfo> {
        vec![online_model_info()]
    }

    pub fn rescan_local_models(&self) -> Result<()> {
        Ok(())
    }

    pub fn get_model_info(&self, model_id: &str) -> Option<ModelInfo> {
        if model_id == ONLINE_MODEL_ID {
            Some(online_model_info())
        } else {
            // Tolerate stale persisted selections from old installs by
            // resolving the configured default instead.
            let selected = get_settings(&self.app_handle).selected_model;
            (selected == model_id).then(online_model_info)
        }
    }

    pub async fn download_model(&self, _model_id: &str) -> Result<()> {
        Err(anyhow::anyhow!(
            "Sokhan uses the built-in online engine; there is nothing to download."
        ))
    }

    pub fn delete_model(&self, _model_id: &str) -> Result<()> {
        Err(anyhow::anyhow!(
            "The built-in online engine cannot be deleted."
        ))
    }

    pub fn cancel_download(&self, _model_id: &str) -> Result<()> {
        Ok(())
    }
}

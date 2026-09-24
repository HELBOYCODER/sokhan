//! Apple Intelligence (FoundationModels) post-processing was removed together
//! with the local-model stack. The call sites (optional LLM polish provider)
//! stay alive through these stubs; the feature reports as unavailable.

#![allow(dead_code)]

pub fn check_apple_intelligence_availability() -> bool {
    false
}

pub fn process_text_with_system_prompt(
    _system_prompt: &str,
    _user_content: &str,
    _max_tokens: i32,
) -> Result<String, String> {
    Err("Apple Intelligence is not available in this build".to_string())
}

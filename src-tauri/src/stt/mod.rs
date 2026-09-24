//! Speech-to-text engine layer: Google's online (SDA) protocol, a streaming
//! FLAC encoder, and the session API used by the transcription manager.

pub mod engine;
pub mod flac_stream;
pub mod protocol;

pub use engine::InterimText;

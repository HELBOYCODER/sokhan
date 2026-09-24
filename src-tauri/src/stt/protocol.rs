//! Google "Sound Drive Audio" full-duplex wire protocol — the transport
//! behind Chromium's Web Speech API.
//!
//! Two paired HTTP/2 requests share a `pair` id: a long-lived GET on `/down`
//! streams 4-byte length-prefixed `SpeechRecognitionEvent` protobufs while a
//! POST on `/up` carries the FLAC-encoded 16 kHz mono audio.
//! Reference: chromium `content/browser/speech/network_speech_recognition_engine_impl.cc`.

use anyhow::{anyhow, Result};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub const BASE_URL: &str = "https://www.google.com/speech-api/full-duplex/v1";
/// The API key Chromium embeds for the speech service (public in every Chrome
/// binary; the service rejects other keys with 403).
pub const SPEECH_API_KEY: &str = "AIzaSyDr2UxVnv_U85AbhhY8XSHSIavUW0DC-sY";
pub const CHROME_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/153.0.0.0 Safari/537.36";
pub const FLAC_CONTENT_TYPE: &str = "audio/x-flac; rate=16000";

pub struct Alternative {
    pub transcript: String,
    #[allow(dead_code)]
    pub confidence: Option<f32>,
}

pub struct RecognitionResult {
    pub alternatives: Vec<Alternative>,
    pub is_final: bool,
    #[allow(dead_code)]
    pub stability: Option<f32>,
}

pub struct RecognitionEvent {
    /// 0 = SUCCESS; 1 = NO_SPEECH; others are fatal per the Web Speech spec.
    pub status: Option<u32>,
    /// 0 SOS, 1 EOS, 2 END_OF_AUDIO, 3 END_OF_UTTERANCE.
    pub endpoint: Option<u32>,
    pub results: Vec<RecognitionResult>,
}

pub const END_OF_UTTERANCE: u32 = 3;

pub fn pair_key() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let now_micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0);
    // Mirror Chromium: high 32 bits random, low 32 bits timestamp, hex-encoded.
    let seed = now_micros ^ (COUNTER.fetch_add(1, Ordering::Relaxed) << 40);
    let mut rnd = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    rnd ^= rnd >> 31;
    let key: u64 = ((rnd & 0xFFFF_FFFF_0000_0000) | (now_micros & 0xFFFF_FFFF))
        .wrapping_mul(0x9E37_79B9_7F4A_7C15);
    key.to_be_bytes()
        .iter()
        .map(|b| format!("{:02X}", b))
        .collect::<String>()
}

pub fn urlencoding(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{:02X}", b),
        })
        .collect()
}

// ------------------------- minimal proto2 decoder --------------------------
// SpeechRecognitionEvent { status=1(varint), result=2(rep msg), endpoint=4(varint) }
// SpeechRecognitionResult { alternative=1(rep msg), final=2(bool), stability=3(float) }
// SpeechRecognitionAlternative { transcript=1(string), confidence=2(float) }

fn read_varint(buf: &[u8], pos: &mut usize) -> Result<u64> {
    let (mut value, mut shift) = (0u64, 0u32);
    loop {
        let byte = *buf
            .get(*pos)
            .ok_or_else(|| anyhow!("truncated protobuf varint"))?;
        *pos += 1;
        value |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
        if shift > 63 {
            return Err(anyhow!("protobuf varint too long"));
        }
    }
}

fn skip_field(buf: &[u8], pos: &mut usize, wire: u32) -> Result<()> {
    match wire {
        0 => {
            read_varint(buf, pos)?;
        }
        1 => *pos += 8,
        2 => {
            let len = read_varint(buf, pos)? as usize;
            *pos += len;
        }
        5 => *pos += 4,
        other => return Err(anyhow!("bad protobuf wire type {other}")),
    }
    if *pos > buf.len() {
        return Err(anyhow!("protobuf field out of bounds"));
    }
    Ok(())
}

fn parse_alternative(buf: &[u8]) -> Result<Alternative> {
    let (mut pos, mut transcript, mut confidence) = (0usize, String::new(), None);
    while pos < buf.len() {
        let key = read_varint(buf, &mut pos)?;
        let (field, wire) = (key >> 3, (key & 7) as u32);
        match (field, wire) {
            (1, 2) => {
                let len = read_varint(buf, &mut pos)? as usize;
                let end = pos + len;
                if end > buf.len() {
                    return Err(anyhow!("truncated transcript"));
                }
                transcript = String::from_utf8_lossy(&buf[pos..end]).into_owned();
                pos = end;
            }
            (2, 5) => {
                if pos + 4 > buf.len() {
                    return Err(anyhow!("truncated confidence"));
                }
                confidence = Some(f32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap()));
                pos += 4;
            }
            (_, w) => skip_field(buf, &mut pos, w)?,
        }
    }
    Ok(Alternative {
        transcript,
        confidence,
    })
}

fn parse_result(buf: &[u8]) -> Result<RecognitionResult> {
    let (mut pos, mut alternatives, mut is_final, mut stability) =
        (0usize, Vec::new(), false, None);
    while pos < buf.len() {
        let key = read_varint(buf, &mut pos)?;
        let (field, wire) = (key >> 3, (key & 7) as u32);
        match (field, wire) {
            (1, 2) => {
                let len = read_varint(buf, &mut pos)? as usize;
                let end = pos + len;
                if end > buf.len() {
                    return Err(anyhow!("truncated alternative"));
                }
                alternatives.push(parse_alternative(&buf[pos..end])?);
                pos = end;
            }
            (2, 0) => is_final = read_varint(buf, &mut pos)? != 0,
            (3, 5) => {
                if pos + 4 > buf.len() {
                    return Err(anyhow!("truncated stability"));
                }
                stability = Some(f32::from_le_bytes(buf[pos..pos + 4].try_into().unwrap()));
                pos += 4;
            }
            (_, w) => skip_field(buf, &mut pos, w)?,
        }
    }
    Ok(RecognitionResult {
        alternatives,
        is_final,
        stability,
    })
}

pub fn parse_event(buf: &[u8]) -> Result<RecognitionEvent> {
    let (mut pos, mut status, mut endpoint, mut results) = (0usize, None, None, Vec::new());
    while pos < buf.len() {
        let key = read_varint(buf, &mut pos)?;
        let (field, wire) = (key >> 3, (key & 7) as u32);
        match (field, wire) {
            (1, 0) => status = Some(read_varint(buf, &mut pos)? as u32),
            (2, 2) => {
                let len = read_varint(buf, &mut pos)? as usize;
                let end = pos + len;
                if end > buf.len() {
                    return Err(anyhow!("truncated result"));
                }
                results.push(parse_result(&buf[pos..end])?);
                pos = end;
            }
            (4, 0) => endpoint = Some(read_varint(buf, &mut pos)? as u32),
            (_, w) => skip_field(buf, &mut pos, w)?,
        }
    }
    Ok(RecognitionEvent {
        status,
        endpoint,
        results,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_handcrafted_event() {
        let mut alt = vec![0x0A, 0x02]; // transcript "hi"
        alt.extend(b"hi");
        let mut result = vec![0x0A, alt.len() as u8];
        result.extend(&alt);
        result.extend([0x10, 0x01]); // final = true
        let mut buf = vec![0x12, result.len() as u8];
        buf.extend(&result);

        let event = parse_event(&buf).unwrap();
        assert_eq!(event.status, None);
        assert_eq!(event.results.len(), 1);
        assert!(event.results[0].is_final);
        assert_eq!(event.results[0].alternatives[0].transcript, "hi");
    }

    #[test]
    fn pair_key_shape() {
        let key = pair_key();
        assert_eq!(key.len(), 16);
        assert!(key.chars().all(|c| c.is_ascii_hexdigit()));
    }
}

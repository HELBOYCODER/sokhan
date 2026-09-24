//! Online Google speech recognition — the same free, keyless backend that
//! Chromium's Web Speech API uses ("Sound Drive Audio" full-duplex protocol).
//!
//! Protocol reference: chromium `content/browser/speech/
//! network_speech_recognition_engine_impl.cc`. Two paired HTTP/2 requests
//! share a `pair` id: a long-lived GET on `/down` streams
//! length-prefixed `SpeechRecognitionEvent` protobufs while a POST on `/up`
//! carries the FLAC-encoded 16 kHz mono audio.

use anyhow::{anyhow, Result};
use log::{debug, error, info};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc;

const BASE_URL: &str = "https://www.google.com/speech-api/full-duplex/v1";
/// The API key Chromium embeds for the speech service (public in every Chrome
/// binary; the service rejects other keys with 403).
const SPEECH_API_KEY: &str = "AIzaSyDr2UxVnv_U85AbhhY8XSHSIavUW0DC-sY";
const CHROME_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/153.0.0.0 Safari/537.36";
const MAX_WAIT: Duration = Duration::from_secs(45);
// The server may split an utterance into several final results; keep reading
// until the downstream goes quiet for this long.
const QUIET_PERIOD: Duration = Duration::from_millis(2500);

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

fn pair_key() -> String {
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

fn encode_flac(samples: &[f32]) -> Result<Vec<u8>> {
    let i16s: Vec<i32> = samples
        .iter()
        .map(|s| (s.clamp(-1.0, 1.0) * 32_767.0).round() as i32)
        .collect();
    let audio = flac_io::FlacAudio {
        sample_rate: 16_000,
        channels: 1,
        bits_per_sample: 16,
        samples: vec![i16s],
    };
    flac_io::encode(&audio).map_err(|e| anyhow!("FLAC encode failed: {e}"))
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

fn parse_event(buf: &[u8]) -> Result<RecognitionEvent> {
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

// -------------------------------------------------------------------------

/// Transcribe a full 16 kHz mono clip with Google's online engine.
///
/// Runs the async protocol on a dedicated thread: this is called from inside
/// Tauri's tokio runtime, and nesting a second `block_on` there aborts the
/// task silently — which would leave the UI stuck on "transcribing" forever.
pub fn transcribe(samples: &[f32], language: &str) -> Result<String> {
    if samples.is_empty() {
        return Ok(String::new());
    }
    let samples = samples.to_vec();
    let language = language.to_string();
    let worker = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| anyhow!("failed to start speech runtime: {e}"))?;
        runtime.block_on(transcribe_async(&samples, &language))
    });
    match worker.join() {
        Ok(result) => result,
        Err(_) => Err(anyhow!("speech worker panicked")),
    }
}

async fn transcribe_async(samples: &[f32], language: &str) -> Result<String> {
    let flac = encode_flac(samples)?;
    debug!(
        "Google online STT: {} samples -> {} FLAC bytes, lang={}",
        samples.len(),
        flac.len(),
        language
    );

    let pair = pair_key();
    let common = format!("key={}&pair={}&output=pb", SPEECH_API_KEY, pair);
    let down_url = format!("{}/down?{}", BASE_URL, common);
    let up_url = format!(
        "{}/up?{}&lang={}&pFilter=0&maxAlternatives=1&app=chromium&continuous&interim",
        BASE_URL,
        common,
        urlencoding(language)
    );

    let client = reqwest::Client::builder()
        .user_agent(CHROME_USER_AGENT)
        .build()
        .map_err(|e| anyhow!("http client error: {e}"))?;

    let (event_tx, mut event_rx) = mpsc::unbounded_channel::<RecognitionEvent>();

    // Downstream reader: 4-byte big-endian length-prefixed protobuf events.
    let down_client = client.clone();
    let down_handle = tokio::spawn(async move {
        let Ok(response) = down_client.get(&down_url).send().await else {
            return;
        };
        if !response.status().is_success() {
            error!("speech down channel HTTP {}", response.status());
            return;
        }
        let mut stream = response.bytes_stream();
        let mut buffer: Vec<u8> = Vec::new();
        use futures_util::StreamExt;
        while let Some(Ok(chunk)) = stream.next().await {
            buffer.extend_from_slice(&chunk);
            loop {
                if buffer.len() < 4 {
                    break;
                }
                let len = u32::from_be_bytes(buffer[..4].try_into().unwrap()) as usize;
                if len > 1_000_000 {
                    return; // framing corrupted; bail out
                }
                if buffer.len() < 4 + len {
                    break;
                }
                let message = buffer[4..4 + len].to_vec();
                buffer.drain(..4 + len);
                match parse_event(&message) {
                    Ok(event) => {
                        if event_tx.send(event).is_err() {
                            return;
                        }
                    }
                    Err(e) => debug!("skipping undecodable speech event: {e}"),
                }
            }
        }
    });

    // Upstream: the whole FLAC clip in one POST.
    let up_response = client
        .post(&up_url)
        .header(reqwest::header::CONTENT_TYPE, "audio/x-flac; rate=16000")
        .body(flac)
        .send()
        .await
        .map_err(|e| anyhow!("speech upload failed: {e}"))?;
    let up_status = up_response.status();
    if !up_status.is_success() {
        let body = up_response.text().await.unwrap_or_default();
        let snippet: String = body.chars().take(160).collect();
        down_handle.abort();
        return Err(anyhow!(
            "speech API rejected the request (HTTP {up_status}): {snippet}"
        ));
    }
    debug!("speech up channel accepted (HTTP {up_status})");

    // Collect final transcripts until the stream ends or goes quiet.
    let deadline = tokio::time::Instant::now() + MAX_WAIT;
    let mut finals: Vec<String> = Vec::new();
    let mut saw_status_error: Option<u32> = None;

    fn push_final(finals: &mut Vec<String>, event: &RecognitionEvent) {
        for result in &event.results {
            if result.is_final {
                if let Some(alt) = result.alternatives.first() {
                    let text = alt.transcript.trim();
                    if !text.is_empty() {
                        finals.push(text.to_string());
                    }
                }
            }
        }
    }

    loop {
        let event = tokio::select! {
            maybe_event = event_rx.recv() => maybe_event,
            _ = tokio::time::sleep_until(deadline) => None,
        };
        let Some(event) = event else {
            break;
        };
        if let Some(status) = event.status {
            if status != 0 {
                saw_status_error = Some(status);
            }
        }
        push_final(&mut finals, &event);
        // END_OF_UTTERANCE (3) means no more results are expected.
        let utterance_over = event.endpoint == Some(3);
        if !finals.is_empty() {
            if utterance_over {
                break;
            }
            // Grace period for stragglers after the first final answer.
            loop {
                tokio::select! {
                    maybe_event = event_rx.recv() => match maybe_event {
                        Some(event) => {
                            let done = event.endpoint == Some(3);
                            push_final(&mut finals, &event);
                            if done {
                                break;
                            }
                        }
                        None => break,
                    },
                    _ = tokio::time::sleep(QUIET_PERIOD) => break,
                }
            }
            break;
        }
    }

    down_handle.abort();

    let text = finals.join(" ").trim().to_string();
    if text.is_empty() {
        match saw_status_error {
            // NO_SPEECH is a normal outcome for silence-only captures.
            Some(1) | None => {
                info!("Google online STT returned no speech");
                Ok(String::new())
            }
            Some(status) => Err(anyhow!("Google speech service error (status {status})")),
        }
    } else {
        Ok(text)
    }
}

fn urlencoding(value: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_handcrafted_event() {
        // SpeechRecognitionEvent { result { alternative { transcript: "hi" }, final: true } }
        let mut buf = Vec::new();
        let mut alt = Vec::new();
        alt.extend([0x0A, 0x02]); // field 1 (transcript), wire 2, len 2
        alt.extend(b"hi");
        let mut result = Vec::new();
        result.push(0x0A); // field 1 (alternative), wire 2
        result.push(alt.len() as u8);
        result.extend(&alt);
        result.extend([0x10, 0x01]); // field 2 (final) varint 1
        buf.push(0x12); // field 2 (result), wire 2
        buf.push(result.len() as u8);
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

    #[test]
    fn flac_encode_roundtrip_size() {
        let samples = vec![0.1f32; 16_000];
        let encoded = encode_flac(&samples).unwrap();
        assert!(!encoded.is_empty());
        assert_eq!(&encoded[0..4], b"fLaC");
    }
}

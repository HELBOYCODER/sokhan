//! Streaming + batch transcription sessions on top of the SDA protocol.
//!
//! A session uploads FLAC incrementally while dictation is in progress and
//! receives interim/final results live, powering the on-screen subtitles.
//! Batch transcription is the same machinery with the audio fed up-front,
//! plus splitting of very long clips at low-energy points.

use super::flac_stream::FlacStreamWriter;
use super::protocol::{
    pair_key, urlencoding, BASE_URL, CHROME_USER_AGENT, FLAC_CONTENT_TYPE, SPEECH_API_KEY,
};
use anyhow::{anyhow, Result};
use bytes::Bytes;
use futures_util::StreamExt;
use log::{debug, error, info};
use std::sync::mpsc as std_mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Interim subtitle snapshot pushed to the overlay while speaking.
pub struct InterimText {
    pub committed: String,
    pub tentative: String,
}

pub type InterimCb = Box<dyn Fn(InterimText) + Send + Sync + 'static>;

const MAX_WAIT: Duration = Duration::from_secs(45);
/// After the last final result, give the server this long to deliver
/// stragglers before closing the session.
const QUIET_PERIOD: Duration = Duration::from_millis(1_500);
/// Google tolerates long streams, but keep single requests modest so a
/// network hiccup can't sink a whole dictation.
const MAX_CHUNK_SAMPLES: usize = 50 * 16_000;

enum Cmd {
    Audio(Vec<f32>),
    Finish,
}

#[derive(Default)]
struct SessionState {
    committed: String,
    tentative: String,
    last_final_at: Option<Instant>,
    down_closed: bool,
    status_error: Option<u32>,
    up_error: Option<String>,
}

pub struct StreamingSession {
    cmd_tx: Option<mpsc::UnboundedSender<Cmd>>,
    thread: Option<std::thread::JoinHandle<()>>,
    done_rx: std_mpsc::Receiver<Result<String>>,
}

impl Drop for StreamingSession {
    fn drop(&mut self) {
        // Closing the command channel ends the upload body; the worker
        // thread unwinds on its own.
        self.cmd_tx.take();
    }
}

/// Cheap cloneable handle used by the audio callback to feed live frames.
#[derive(Clone)]
pub struct SessionFeed {
    tx: mpsc::UnboundedSender<Cmd>,
}

impl SessionFeed {
    pub fn push(&self, samples: &[f32]) {
        let _ = self.tx.send(Cmd::Audio(samples.to_vec()));
    }
}

impl StreamingSession {
    pub fn feed_handle(&self) -> Option<SessionFeed> {
        self.cmd_tx.clone().map(|tx| SessionFeed { tx })
    }

    /// Feed more 16 kHz mono audio. Cheap; buffered by the encoder.
    pub fn feed(&self, samples: &[f32]) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(Cmd::Audio(samples.to_vec()));
        }
    }

    /// Flush the audio, wait for the final transcript.
    pub fn finish(mut self) -> Result<String> {
        if let Some(tx) = self.cmd_tx.take() {
            let _ = tx.send(Cmd::Finish);
        }
        match self.done_rx.recv_timeout(MAX_WAIT + Duration::from_secs(10)) {
            Ok(result) => result,
            Err(_) => Err(anyhow!("speech session timed out waiting for results")),
        }
    }
}

/// Open a live session: audio is uploaded as it is fed, results stream back.
pub fn start(language: &str, interim: Option<InterimCb>) -> StreamingSession {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel::<Cmd>();
    let (done_tx, done_rx) = std_mpsc::channel();
    let language = language.to_string();
    let thread = std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                let _ = done_tx.send(Err(anyhow!("failed to start speech runtime: {e}")));
                return;
            }
        };
        runtime.block_on(run_session(cmd_rx, &language, interim, done_tx));
    });
    StreamingSession {
        cmd_tx: Some(cmd_tx),
        thread: Some(thread),
        done_rx,
    }
}

async fn run_session(
    mut cmd_rx: mpsc::UnboundedReceiver<Cmd>,
    language: &str,
    interim: Option<InterimCb>,
    done_tx: std_mpsc::Sender<Result<String>>,
) {
    let state = Arc::new(Mutex::new(SessionState::default()));

    let pair = pair_key();
    let common = format!("key={}&pair={}&output=pb", SPEECH_API_KEY, pair);
    let down_url = format!("{}/down?{}", BASE_URL, common);
    let up_url = format!(
        "{}/up?{}&lang={}&pFilter=0&maxAlternatives=1&app=chromium&continuous&interim",
        BASE_URL,
        common,
        urlencoding(language)
    );

    let client = match reqwest::Client::builder()
        .user_agent(CHROME_USER_AGENT)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = done_tx.send(Err(anyhow!("http client error: {e}")));
            return;
        }
    };

    let (byte_tx, byte_rx) = mpsc::unbounded_channel::<Bytes>();

    // Audio pump: encode incoming PCM into the FLAC stream and forward the
    // encoded bytes to the upload body.
    let _pump = tokio::spawn(async move {
        let mut writer = FlacStreamWriter::new();
        loop {
            let cmd = match cmd_rx.recv().await {
                Some(cmd) => cmd,
                None => break, // session dropped without finish: cancel
            };
            let finish = matches!(cmd, Cmd::Finish);
            if let Cmd::Audio(samples) = cmd {
                writer.push(
                    &samples
                        .iter()
                        .map(|s| (s.clamp(-1.0, 1.0) * 32_767.0).round() as i16)
                        .collect::<Vec<i16>>(),
                );
            }
            if finish {
                writer.finish();
            }
            let encoded = writer.take();
            if !encoded.is_empty() && byte_tx.send(Bytes::from(encoded)).is_err() {
                break;
            }
            if finish {
                break;
            }
        }
        // byte_tx drops here, ending the request body.
    });

    // Downstream: length-prefixed protobuf events.
    let down_client = client.clone();
    let down_state = Arc::clone(&state);
    let down_task = tokio::spawn(async move {
        let Ok(response) = down_client.get(&down_url).send().await else {
            down_state.lock().unwrap().down_closed = true;
            return;
        };
        if !response.status().is_success() {
            error!("speech down channel HTTP {}", response.status());
            down_state.lock().unwrap().down_closed = true;
            return;
        }
        let mut stream = response.bytes_stream();
        let mut buffer: Vec<u8> = Vec::new();
        while let Some(Ok(chunk)) = stream.next().await {
            buffer.extend_from_slice(&chunk);
            loop {
                if buffer.len() < 4 {
                    break;
                }
                let len = u32::from_be_bytes(buffer[..4].try_into().unwrap()) as usize;
                if len > 1_000_000 {
                    down_state.lock().unwrap().down_closed = true;
                    return;
                }
                if buffer.len() < 4 + len {
                    break;
                }
                let message = buffer[4..4 + len].to_vec();
                buffer.drain(..4 + len);
                let Ok(event) = super::protocol::parse_event(&message) else {
                    continue;
                };
                let mut guard = down_state.lock().unwrap();
                if let Some(status) = event.status {
                    if status != 0 {
                        guard.status_error = Some(status);
                    }
                }
                let mut changed = false;
                for result in &event.results {
                    let Some(top) = result.alternatives.first() else {
                        continue;
                    };
                    let text = top.transcript.trim();
                    if text.is_empty() {
                        continue;
                    }
                    if result.is_final {
                        if !guard.committed.is_empty() {
                            guard.committed.push(' ');
                        }
                        guard.committed.push_str(text);
                        guard.tentative.clear();
                        guard.last_final_at = Some(Instant::now());
                    } else {
                        guard.tentative = text.to_string();
                    }
                    changed = true;
                }
                let snapshot = if changed {
                    Some(InterimText {
                        committed: guard.committed.clone(),
                        tentative: guard.tentative.clone(),
                    })
                } else {
                    None
                };
                drop(guard);
                if let (Some(text), Some(cb)) = (snapshot, interim.as_ref()) {
                    cb(text);
                }
            }
        }
        down_state.lock().unwrap().down_closed = true;
    });

    // Upstream: stream the encoded FLAC as the request body.
    let body_stream = futures_util::stream::unfold(byte_rx, |mut rx| async move {
        rx.recv().await.map(|b| (Ok::<_, std::io::Error>(b), rx))
    });
    let up_result = client
        .post(&up_url)
        .header(reqwest::header::CONTENT_TYPE, FLAC_CONTENT_TYPE)
        .body(reqwest::Body::wrap_stream(body_stream))
        .send()
        .await;
    match up_result {
        Ok(response) => {
            let status = response.status();
            if !status.is_success() {
                let body = response.text().await.unwrap_or_default();
                let snippet: String = body.chars().take(160).collect();
                let mut guard = state.lock().unwrap();
                guard.up_error = Some(format!("HTTP {status}: {snippet}"));
                guard.down_closed = true;
            } else {
                debug!("speech up channel accepted (HTTP {status})");
            }
        }
        Err(e) => {
            let mut guard = state.lock().unwrap();
            guard.up_error = Some(format!("upload failed: {e}"));
        }
    }

    // Wait for the transcript: final answer + (stream closed or quiet).
    let deadline = Instant::now() + MAX_WAIT;
    let text;
    let error;
    loop {
        tokio::time::sleep(Duration::from_millis(150)).await;
        let guard = state.lock().unwrap();
        if let Some(msg) = &guard.up_error {
            error = Some(msg.clone());
            text = None;
            break;
        }
        let has_final = !guard.committed.is_empty();
        let quiet = guard
            .last_final_at
            .is_some_and(|t| t.elapsed() > QUIET_PERIOD);
        if (has_final && (guard.down_closed || quiet)) || guard.down_closed {
            text = Some(guard.committed.clone());
            error = guard
                .status_error
                .filter(|s| *s != 1)
                .map(|s| format!("service status {s}"));
            break;
        }
        if Instant::now() > deadline {
            text = Some(guard.committed.clone());
            error = None;
            break;
        }
    }
    down_task.abort();

    let result = match error {
        Some(msg) => Err(anyhow!("Google speech service error: {msg}")),
        None => Ok(text.unwrap_or_default()),
    };
    match &result {
        Ok(text) => info!("speech session produced {} chars", text.len()),
        Err(e) => error!("speech session failed: {e}"),
    }
    let _ = done_tx.send(result);
}

/// Batch-transcribe a full 16 kHz mono clip, splitting very long recordings
/// into <=50s chunks at their quietest points.
pub fn transcribe(samples: &[f32], language: &str) -> Result<String> {
    if samples.is_empty() {
        return Ok(String::new());
    }
    let mut pieces: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    while start < samples.len() {
        let mut end = (start + MAX_CHUNK_SAMPLES).min(samples.len());
        if end < samples.len() {
            end = quietest_split(samples, start, end);
        }
        pieces.push((start, end));
        start = end;
    }
    let mut texts: Vec<String> = Vec::new();
    for (index, (from, to)) in pieces.iter().enumerate() {
        debug!(
            "Batch chunk {}/{}: {} samples -> Google",
            index + 1,
            pieces.len(),
            to - from
        );
        let session = start_session(language);
        session.feed(&samples[*from..*to]);
        let text = session.finish()?;
        if !text.trim().is_empty() {
            texts.push(text.trim().to_string());
        }
    }
    Ok(texts.join(" "))
}

fn start_session(language: &str) -> StreamingSession {
    start(language, None)
}

/// Search the last 20% of [start, end) for the quietest 200 ms window and
/// split there, so words are never cut in half.
fn quietest_split(samples: &[f32], start: usize, end: usize) -> usize {
    let window = 200 * 16; // 200 ms of samples
    let search_from = start + (end - start) * 4 / 5;
    let mut best = end;
    let mut best_energy = f32::INFINITY;
    let mut pos = search_from;
    while pos + window < end {
        let energy: f32 = samples[pos..pos + window]
            .iter()
            .map(|s| s * s)
            .sum::<f32>()
            / window as f32;
        if energy < best_energy {
            best_energy = energy;
            best = pos + window / 2;
        }
        pos += window;
    }
    best
}

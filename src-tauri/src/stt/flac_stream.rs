//! Incremental FLAC stream encoder for 16 kHz mono 16-bit audio.
//!
//! Emits a valid FLAC byte stream frame-by-frame so encoded bytes can be
//! uploaded while recording is still in progress (live dictation). Uses
//! fixed 4096-sample blocks with VERBATIM (uncompressed) subframes —
//! lossless, ~PCM size, and simple enough to trust. `total_samples` stays
//! 0 (unknown) exactly like Chrome's streaming encoder; the final partial
//! block is zero-padded, which the spec allows.

const BLOCK_SAMPLES: usize = 4_096;
const SAMPLE_RATE: u32 = 16_000;

fn crc8(data: &[u8]) -> u8 {
    let mut crc: u8 = 0;
    for byte in data {
        crc ^= byte;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x07 } else { crc << 1 };
        }
    }
    crc
}

fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0;
    for byte in data {
        crc ^= (*byte as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x8005 } else { crc << 1 };
        }
    }
    crc
}

struct BitWriter {
    buf: Vec<u8>,
    byte: u8,
    bits: u32,
}

impl BitWriter {
    fn new() -> Self {
        Self { buf: Vec::new(), byte: 0, bits: 0 }
    }

    fn write(&mut self, value: u64, count: u32) {
        for i in (0..count).rev() {
            self.byte = (self.byte << 1) | (((value >> i) & 1) as u8);
            self.bits += 1;
            if self.bits == 8 {
                self.buf.push(self.byte);
                self.byte = 0;
                self.bits = 0;
            }
        }
    }

    /// Pad the pending bits with zeros and return the bytes written so far.
    fn flush_bytes(&mut self) {
        if self.bits > 0 {
            self.byte <<= 8 - self.bits;
            self.buf.push(self.byte);
            self.byte = 0;
            self.bits = 0;
        }
    }
}

/// UTF-8 coding of a sample/frame number as required by the FLAC frame header.
fn write_utf8_number(w: &mut BitWriter, value: u64) {
    let bytes: Vec<u8> = if value < 0x80 {
        vec![value as u8]
    } else if value < 0x800 {
        vec![0xC0 | (value >> 6) as u8, 0x80 | (value & 0x3F) as u8]
    } else if value < 0x1_0000 {
        vec![
            0xE0 | (value >> 12) as u8,
            0x80 | ((value >> 6) & 0x3F) as u8,
            0x80 | (value & 0x3F) as u8,
        ]
    } else if value < 0x20_0000 {
        vec![
            0xF0 | (value >> 18) as u8,
            0x80 | ((value >> 12) & 0x3F) as u8,
            0x80 | ((value >> 6) & 0x3F) as u8,
            0x80 | (value & 0x3F) as u8,
        ]
    } else if value < 0x400_0000 {
        vec![
            0xF8 | (value >> 24) as u8,
            0x80 | ((value >> 18) & 0x3F) as u8,
            0x80 | ((value >> 12) & 0x3F) as u8,
            0x80 | ((value >> 6) & 0x3F) as u8,
            0x80 | (value & 0x3F) as u8,
        ]
    } else if value < 0x8000_0000 {
        vec![
            0xFC | (value >> 30) as u8,
            0x80 | ((value >> 24) & 0x3F) as u8,
            0x80 | ((value >> 18) & 0x3F) as u8,
            0x80 | ((value >> 12) & 0x3F) as u8,
            0x80 | ((value >> 6) & 0x3F) as u8,
            0x80 | (value & 0x3F) as u8,
        ]
    } else {
        vec![
            0xFE,
            0x80 | ((value >> 36) & 0x3F) as u8,
            0x80 | ((value >> 30) & 0x3F) as u8,
            0x80 | ((value >> 24) & 0x3F) as u8,
            0x80 | ((value >> 18) & 0x3F) as u8,
            0x80 | ((value >> 12) & 0x3F) as u8,
            0x80 | ((value >> 6) & 0x3F) as u8,
            0x80 | (value & 0x3F) as u8,
        ]
    };
    for b in bytes {
        w.write(b as u64, 8);
    }
}

fn encode_frame(samples: &[i16], frame_number: u64) -> Vec<u8> {
    debug_assert_eq!(samples.len(), BLOCK_SAMPLES);
    let mut w = BitWriter::new();
    w.write(0b11_1111_1111_1110, 14); // frame sync
    w.write(0, 1); // reserved
    w.write(0, 1); // blocking strategy: fixed block size
    w.write(0b1100, 4); // block size code: 4096
    w.write(0b0101, 4); // sample rate code: 16000 Hz
    w.write(0b0000, 4); // channel assignment: mono
    w.write(0b100, 3); // bits per sample: 16
    w.write(0, 1); // reserved
    write_utf8_number(&mut w, frame_number);
    debug_assert_eq!(w.bits, 0); // header is byte-aligned before the CRC8
    let header_crc = crc8(&w.buf);
    w.write(header_crc as u64, 8);

    // VERBATIM subframe: 0 pad bit + 6-bit type + wasted=0. The spec's binary
    // constants are written LSB-first; on the wire verbatim is 000001.
    w.write(0, 1);
    w.write(0b000001, 6);
    w.write(0, 1);
    for s in samples {
        w.write((*s as u16) as u64, 16);
    }
    w.flush_bytes();

    let frame_crc = crc16(&w.buf);
    w.write((frame_crc >> 8) as u64, 8);
    w.write((frame_crc & 0xFF) as u64, 8);
    w.flush_bytes();
    w.buf
}

pub struct FlacStreamWriter {
    pending: Vec<u8>,
    frame: Vec<i16>,
    frame_number: u64,
    finished: bool,
}

impl FlacStreamWriter {
    /// `total_samples: None` = unknown length (live streaming, like Chrome).
    pub fn new() -> Self {
        Self::new_with_total(None)
    }

    pub fn new_with_total(total_samples: Option<u64>) -> Self {
        let mut pending = Vec::with_capacity(64 * 1024);
        pending.extend_from_slice(b"fLaC");
        // STREAMINFO, marked as the last metadata block.
        let mut info = Vec::with_capacity(34);
        info.extend_from_slice(&(BLOCK_SAMPLES as u32).to_be_bytes()[2..]); // min blocksize
        info.extend_from_slice(&(BLOCK_SAMPLES as u32).to_be_bytes()[2..]); // max blocksize
        info.extend_from_slice(&[0, 0, 0]); // min framesize unknown
        info.extend_from_slice(&[0, 0, 0]); // max framesize unknown
        // 20-bit rate | 3-bit channels-1 | 5-bit depth-1 | 36-bit total (0 = unknown)
        let total = total_samples.unwrap_or(0);
        let tail: u64 = (SAMPLE_RATE as u64) << 44 | 15u64 << 36 | (total & 0xF_FFFF_FFFF);
        info.extend_from_slice(&tail.to_be_bytes());
        info.extend_from_slice(&[0u8; 16]); // unverified MD5
        pending.push(0x80);
        pending.extend_from_slice(&[0u8, 0u8, info.len() as u8]);
        pending.extend_from_slice(&info);
        Self {
            pending,
            frame: Vec::with_capacity(BLOCK_SAMPLES),
            frame_number: 0,
            finished: false,
        }
    }

    /// Feed 16-bit samples; complete frames move into the output buffer.
    pub fn push(&mut self, samples: &[i16]) {
        assert!(!self.finished, "push after finish");
        let mut offset = 0;
        while offset < samples.len() {
            let take = (BLOCK_SAMPLES - self.frame.len()).min(samples.len() - offset);
            self.frame.extend_from_slice(&samples[offset..offset + take]);
            offset += take;
            if self.frame.len() == BLOCK_SAMPLES {
                let frame = encode_frame(&self.frame, self.frame_number);
                self.pending.extend_from_slice(&frame);
                self.frame_number += 1;
                self.frame.clear();
            }
        }
    }

    /// Zero-pad and emit the trailing block. Idempotent.
    pub fn finish(&mut self) {
        if self.finished {
            return;
        }
        if !self.frame.is_empty() {
            let pad = BLOCK_SAMPLES - self.frame.len();
            self.frame.extend(std::iter::repeat(0i16).take(pad));
            let frame = encode_frame(&self.frame, self.frame_number);
            self.pending.extend_from_slice(&frame);
            self.frame.clear();
        }
        self.finished = true;
    }

    /// Drain everything encoded so far.
    pub fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.pending)
    }

    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

/// One-shot helper: f32 samples ([-1,1]) -> complete FLAC stream.
pub fn encode_f32(samples: &[f32]) -> Vec<u8> {
    // one-shot knows its length; strict decoders rely on it.
    let i16s: Vec<i16> = samples
        .iter()
        .map(|s| (s.clamp(-1.0, 1.0) * 32_767.0).round() as i16)
        .collect();
    let mut writer = FlacStreamWriter::new_with_total(Some(samples.len() as u64));
    writer.push(&i16s);
    writer.finish();
    writer.take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streaminfo_header() {
        let out = encode_f32(&[0.0f32; 100]);
        assert_eq!(&out[0..4], b"fLaC");
        assert_eq!(out[4], 0x80); // last metadata block, type STREAMINFO
        assert_eq!(&out[5..8], &[0, 0, 34]);
    }

    #[test]
    fn incremental_equals_one_shot() {
        let samples: Vec<f32> = (0..10_000).map(|i| ((i as f32) * 0.01).sin() * 0.5).collect();
        let one_shot = encode_f32(&samples);

        let i16s: Vec<i16> = samples
            .iter()
            .map(|s| (s.clamp(-1.0, 1.0) * 32_767.0).round() as i16)
            .collect();
        let mut writer = FlacStreamWriter::new_with_total(Some(samples.len() as u64));
        let mut streamed = Vec::new();
        for chunk in i16s.chunks(777) {
            writer.push(chunk);
            streamed.extend_from_slice(&writer.take());
        }
        writer.finish();
        streamed.extend_from_slice(&writer.take());
        assert_eq!(one_shot, streamed);
    }

    #[test]
    fn claxon_roundtrips_streamed_frames() {
        let i16s: Vec<i16> = (0..10_000).map(|i| (i % 300 - 150) * 100).collect();
        let mut writer = FlacStreamWriter::new_with_total(Some(i16s.len() as u64));
        let mut encoded = Vec::new();
        for chunk in i16s.chunks(777) {
            writer.push(chunk);
            encoded.extend_from_slice(&writer.take());
        }
        writer.finish();
        encoded.extend_from_slice(&writer.take());

        let mut decoder = claxon::FlacReader::new(std::io::Cursor::new(&encoded))
            .expect("claxon must accept our stream");
        let got: Vec<i32> = decoder.samples().map(|s| s.expect("sample decode")).collect();
        // Padded tail blocks are part of the frame stream; compare the real range.
        assert!(got.len() >= i16s.len());
        assert_eq!(&got[..i16s.len()], &i16s.iter().map(|s| *s as i32).collect::<Vec<i32>>()[..]);
    }
}

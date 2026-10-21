//! Two-way audio: Opus (pure-Rust libopus) ⇄ speakers / microphone (cpal).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Linear resampler for mono `f32` audio, keeping state between chunks.
pub struct Resampler {
    from: f64,
    to: f64,
    pos: f64,
    last: f32,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Self {
        Self { from: from as f64, to: to as f64, pos: 0.0, last: 0.0 }
    }

    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if self.from == self.to {
            out.extend_from_slice(input);
            return;
        }
        let step = self.from / self.to;
        // `pos` indexes into [last, input...]; index 0 is the previous chunk's tail.
        let len = input.len() as f64;
        while self.pos < len {
            let i = self.pos.floor() as usize;
            let frac = (self.pos - i as f64) as f32;
            let a = if i == 0 { self.last } else { input[i - 1] };
            let b = input[i.min(input.len() - 1)];
            out.push(a + (b - a) * frac);
            self.pos += step;
        }
        self.pos -= len;
        if let Some(&l) = input.last() {
            self.last = l;
        }
    }
}

pub const RATE: u32 = 48_000;
/// 20 ms at 48 kHz.
pub const FRAME: usize = 960;

pub struct OpusDecoder(*mut unsafe_libopus::OpusDecoder);
// The decoder is only ever used from one thread at a time.
unsafe impl Send for OpusDecoder {}

impl OpusDecoder {
    pub fn new() -> Option<Self> {
        let mut err = 0;
        let p = unsafe { unsafe_libopus::opus_decoder_create(RATE as i32, 2, &mut err) };
        (!p.is_null() && err == 0).then_some(Self(p))
    }

    /// Decodes one packet to interleaved stereo `i16`.
    pub fn decode(&mut self, packet: &[u8]) -> Vec<i16> {
        let mut pcm = vec![0i16; 5760 * 2];
        let n = unsafe { unsafe_libopus::opus_decode(self.0, packet.as_ptr(), packet.len() as i32, pcm.as_mut_ptr(), 5760, 0) };
        pcm.truncate(n.max(0) as usize * 2);
        pcm
    }
}

impl Drop for OpusDecoder {
    fn drop(&mut self) {
        unsafe { unsafe_libopus::opus_decoder_destroy(self.0) }
    }
}

pub struct OpusEncoder(*mut unsafe_libopus::OpusEncoder);
unsafe impl Send for OpusEncoder {}

impl OpusEncoder {
    pub fn new() -> Option<Self> {
        let mut err = 0;
        let p = unsafe { unsafe_libopus::opus_encoder_create(RATE as i32, 2, unsafe_libopus::OPUS_APPLICATION_VOIP, &mut err) };
        (!p.is_null() && err == 0).then_some(Self(p))
    }

    /// Encodes one 20 ms interleaved stereo frame (`FRAME * 2` samples).
    pub fn encode(&mut self, pcm: &[i16]) -> Vec<u8> {
        let mut out = vec![0u8; 1500];
        let n = unsafe { unsafe_libopus::opus_encode(self.0, pcm.as_ptr(), FRAME as i32, out.as_mut_ptr(), out.len() as i32) };
        out.truncate(n.max(0) as usize);
        out
    }
}

impl Drop for OpusEncoder {
    fn drop(&mut self) {
        unsafe { unsafe_libopus::opus_encoder_destroy(self.0) }
    }
}

type Buffer = Arc<Mutex<VecDeque<f32>>>;

/// Plays the external unit and captures the microphone for one call.
/// cpal streams are not `Send` everywhere, so they live on their own thread.
pub struct Audio {
    speaker: Buffer,
    mic: Buffer,
    pub muted: Arc<AtomicBool>,
    pub talking: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    decoder: Mutex<Option<OpusDecoder>>,
    encoder: Mutex<Option<OpusEncoder>>,
    out_rate: Arc<Mutex<u32>>,
}

impl Audio {
    pub fn start() -> Arc<Self> {
        let a = Arc::new(Self {
            speaker: Arc::default(),
            mic: Arc::default(),
            muted: Arc::new(AtomicBool::new(false)),
            talking: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(false)),
            decoder: Mutex::new(OpusDecoder::new()),
            encoder: Mutex::new(OpusEncoder::new()),
            out_rate: Arc::new(Mutex::new(RATE)),
        });
        let t = a.clone();
        std::thread::Builder::new()
            .name("easykit-audio".into())
            .spawn(move || {
                let streams = open_streams(&t);
                if let Err(e) = &streams {
                    log::warn!("audio unavailable: {e}");
                }
                while !t.stop.load(Ordering::Relaxed) {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                drop(streams);
            })
            .ok();
        a
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Opus payload from the external unit → speakers.
    pub fn play(&self, payload: &[u8]) {
        if self.muted.load(Ordering::Relaxed) {
            return;
        }
        let Some(pcm) = self.decoder.lock().unwrap().as_mut().map(|d| d.decode(payload)) else { return };
        let mono: Vec<f32> = pcm.chunks_exact(2).map(|s| (s[0] as f32 + s[1] as f32) / 65536.0).collect();
        let rate = *self.out_rate.lock().unwrap();
        let mut out = Vec::with_capacity(mono.len() * rate as usize / RATE as usize + 1);
        Resampler::new(RATE, rate).process(&mono, &mut out);
        let mut buf = self.speaker.lock().unwrap();
        // Keep latency bounded (~300 ms).
        let cap = rate as usize * 3 / 10;
        buf.extend(out);
        let excess = buf.len().saturating_sub(cap);
        buf.drain(..excess);
    }

    /// Next 20 ms Opus frame from the microphone while talking, else `None`
    /// (the caller then sends silence).
    pub fn next_mic_frame(&self) -> Option<Vec<u8>> {
        let mut buf = self.mic.lock().unwrap();
        if !self.talking.load(Ordering::Relaxed) {
            buf.clear();
            return None;
        }
        if buf.len() < FRAME {
            return None;
        }
        let pcm: Vec<i16> =
            buf.drain(..FRAME).flat_map(|s| { let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16; [v, v] }).collect();
        drop(buf);
        self.encoder.lock().unwrap().as_mut().map(|e| e.encode(&pcm))
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.stop();
    }
}

fn open_streams(a: &Arc<Audio>) -> Result<(cpal::Stream, Option<cpal::Stream>), String> {
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    let host = cpal::default_host();
    let out = host.default_output_device().ok_or("no output device")?;
    let ocfg = out.default_output_config().map_err(|e| e.to_string())?;
    *a.out_rate.lock().unwrap() = ocfg.sample_rate().0;
    let channels = ocfg.channels() as usize;
    let speaker = a.speaker.clone();
    let err = |e| log::warn!("audio stream: {e}");
    let fill = move |data: &mut [f32]| {
        let mut buf = speaker.lock().unwrap();
        for frame in data.chunks_mut(channels) {
            let s = buf.pop_front().unwrap_or(0.0);
            frame.iter_mut().for_each(|x| *x = s);
        }
    };
    let ostream = match ocfg.sample_format() {
        cpal::SampleFormat::F32 => out.build_output_stream(&ocfg.into(), move |d: &mut [f32], _| fill(d), err, None),
        cpal::SampleFormat::I16 => {
            let mut tmp = Vec::new();
            out.build_output_stream(
                &ocfg.into(),
                move |d: &mut [i16], _| {
                    tmp.resize(d.len(), 0.0);
                    fill(&mut tmp);
                    d.iter_mut().zip(&tmp).for_each(|(o, s)| *o = (s * 32767.0) as i16);
                },
                err,
                None,
            )
        }
        f => return Err(format!("unsupported output format {f:?}")),
    }
    .map_err(|e| e.to_string())?;
    ostream.play().map_err(|e| e.to_string())?;

    // The microphone is optional: listening still works without one.
    let istream = (|| {
        let inp = host.default_input_device()?;
        let icfg = inp.default_input_config().ok()?;
        let ch = icfg.channels() as usize;
        let mut rs = Resampler::new(icfg.sample_rate().0, RATE);
        let mic = a.mic.clone();
        let talking = a.talking.clone();
        let mut push = move |mono: &[f32]| {
            if !talking.load(Ordering::Relaxed) {
                return;
            }
            let mut out = Vec::with_capacity(mono.len());
            rs.process(mono, &mut out);
            let mut b = mic.lock().unwrap();
            b.extend(out);
            let excess = b.len().saturating_sub(RATE as usize / 2);
            b.drain(..excess);
        };
        let s = match icfg.sample_format() {
            cpal::SampleFormat::F32 => inp.build_input_stream(
                &icfg.into(),
                move |d: &[f32], _| push(&d.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32).collect::<Vec<_>>()),
                err,
                None,
            ),
            cpal::SampleFormat::I16 => inp.build_input_stream(
                &icfg.into(),
                move |d: &[i16], _| {
                    push(&d.chunks(ch).map(|f| f.iter().map(|&x| x as f32 / 32768.0).sum::<f32>() / ch as f32).collect::<Vec<_>>())
                },
                err,
                None,
            ),
            _ => return None,
        }
        .ok()?;
        s.play().ok()?;
        Some(s)
    })();
    Ok((ostream, istream))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_keeps_length_ratio_across_chunks() {
        let mut r = Resampler::new(44_100, 48_000);
        let mut out = Vec::new();
        for _ in 0..100 {
            r.process(&[0.5; 441], &mut out);
        }
        // 44100 samples in → 48000 out (± one sample).
        assert!((out.len() as i64 - 48_000).abs() <= 1, "{}", out.len());
        // The first two samples ramp up from the initial silence.
        assert!(out.iter().skip(2).all(|&s| (s - 0.5).abs() < 1e-6));
    }

    #[test]
    fn opus_roundtrip() {
        let mut enc = OpusEncoder::new().unwrap();
        let mut dec = OpusDecoder::new().unwrap();
        let pcm: Vec<i16> = (0..FRAME).flat_map(|i| {
            let v = ((i as f32 / 48.0).sin() * 8000.0) as i16;
            [v, v]
        }).collect();
        let pkt = enc.encode(&pcm);
        assert!(!pkt.is_empty() && pkt.len() < 400);
        assert_eq!(dec.decode(&pkt).len(), FRAME * 2);
    }
}

use ruffle_core::backend::audio::{
    AudioBackend, AudioMixer, DecodeError, RegisterError, SoundHandle, SoundInstanceHandle,
    SoundStreamInfo, SoundTransform, swf,
};
use ruffle_core::impl_audio_mixer_backend;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicI32, Ordering},
};
use std::thread::{Builder, JoinHandle};
use std::time::Duration;

const SAMPLE_RATE: u32 = 48_000;
const FRAMES: usize = 1024;

#[repr(align(64))]
struct PcmBuffer([i16; FRAMES * 2]);

unsafe extern "C" {
    fn lonewolf_vita_audio_open(frames: i32, sample_rate: i32) -> i32;
    fn lonewolf_vita_audio_output(port: i32, samples: *const i16) -> i32;
    fn lonewolf_vita_audio_drain(port: i32) -> i32;
    fn lonewolf_vita_audio_close(port: i32) -> i32;
}

pub struct VitaAudio {
    mixer: AudioMixer,
    running: Arc<AtomicBool>,
    playing: Arc<AtomicBool>,
    error: Arc<AtomicI32>,
    thread: Option<JoinHandle<()>>,
}

impl VitaAudio {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let mixer = AudioMixer::new(2, SAMPLE_RATE);
        let proxy = mixer.proxy();
        let running = Arc::new(AtomicBool::new(true));
        let playing = Arc::new(AtomicBool::new(true));
        let error = Arc::new(AtomicI32::new(0));
        let port = unsafe { lonewolf_vita_audio_open(FRAMES as i32, SAMPLE_RATE as i32) };
        if port < 0 {
            return Err(format!("Cannot open Vita audio output: {port:#x}").into());
        }
        let thread_running = running.clone();
        let thread_playing = playing.clone();
        let thread_error = error.clone();
        let thread = Builder::new()
            .name("Lonewolf audio".into())
            // Kernel thread stacks live outside the reserved newlib heap.
            .stack_size(512 * 1024)
            .spawn(move || {
                let mut buffers = [PcmBuffer([0; FRAMES * 2]), PcmBuffer([0; FRAMES * 2])];
                let mut next_buffer = 0;
                let mut heard_sound = false;
                while thread_running.load(Ordering::Relaxed) {
                    // Keep the submitted block intact until audioOut accepts the next one.
                    let samples = &mut buffers[next_buffer].0;
                    if thread_playing.load(Ordering::Relaxed) {
                        proxy.mix(samples);
                    } else {
                        // Pausing must freeze decoder positions, not just mute the output.
                        samples.fill(0);
                    }
                    if !heard_sound && samples.iter().any(|&sample| sample != 0) {
                        crate::vita::log("[audio] first non-silent block mixed\n");
                        heard_sound = true;
                    }
                    let result = unsafe { lonewolf_vita_audio_output(port, samples.as_ptr()) };
                    if result < 0 {
                        thread_error.store(result, Ordering::Relaxed);
                        break;
                    }
                    next_buffer ^= 1;
                }
                // Drain while the aligned stack buffers are still alive.
                let result = unsafe { lonewolf_vita_audio_drain(port) };
                if result < 0 {
                    let _ = thread_error.compare_exchange(
                        0,
                        result,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    );
                }
                let result = unsafe { lonewolf_vita_audio_close(port) };
                if result < 0 {
                    crate::vita::log(&format!("[fatal] audio port release failed: {result:#x}\n"));
                    // Stop the process before buffers can be freed with DMA still active.
                    crate::vita::exit(1);
                }
            });
        let thread = match thread {
            Ok(thread) => thread,
            Err(error) => {
                unsafe { lonewolf_vita_audio_close(port) };
                return Err(error.into());
            }
        };
        crate::vita::log("[audio] stereo PCM output ready: 48000 Hz, 1024 frames\n");
        Ok(Self {
            mixer,
            running,
            playing,
            error,
            thread: Some(thread),
        })
    }
}

impl AudioBackend for VitaAudio {
    impl_audio_mixer_backend!(mixer);

    fn play(&mut self) {
        self.playing.store(true, Ordering::Relaxed);
    }

    fn pause(&mut self) {
        self.playing.store(false, Ordering::Relaxed);
    }

    fn tick(&mut self) {
        let code = self.error.swap(0, Ordering::Relaxed);
        if code < 0 {
            tracing::error!("Vita audio output failed: {code:#x}");
        }
    }

    fn position_resolution(&self) -> Option<Duration> {
        Some(Duration::from_secs_f64(FRAMES as f64 / SAMPLE_RATE as f64))
    }
}

impl Drop for VitaAudio {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

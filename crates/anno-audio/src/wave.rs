//! Wave (sound effect) slot management.
//!
//! Corresponds to the MaxwaveXxx functions in Maxsound.dll.
//! Manages up to 256 sound effect slots with spatial positioning
//! and clone-on-play polyphony.

use rodio::source::Source;
use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

/// Maximum number of wave slots (matching original engine).
pub const MAX_WAVE_SLOTS: usize = 256;

/// Maximum simultaneous instances for one-shot sounds.
pub const MAX_ONESHOT_INSTANCES: usize = 4;

/// Maximum simultaneous instances for looping sounds.
pub const MAX_LOOP_INSTANCES: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaveStatus {
    Empty,
    Stopped,
    Playing,
    Looping,
}

/// A loaded wave sound effect.
pub struct WaveSlot {
    pub status: WaveStatus,
    pub filename: PathBuf,
    pub data: Vec<u8>,
    pub sink: Option<Sink>,
    /// Screen position for spatial audio
    pub position: (f32, f32),
    /// Number of active clone instances
    pub instance_count: u32,
}

impl WaveSlot {
    fn empty() -> Self {
        Self {
            status: WaveStatus::Empty,
            filename: PathBuf::new(),
            data: Vec::new(),
            sink: None,
            position: (0.0, 0.0),
            instance_count: 0,
        }
    }
}

/// Wave sound effect manager.
pub struct WaveManager {
    slots: Vec<WaveSlot>,
    base_dirs: Vec<PathBuf>,
    master_volume: f32,
    play_enabled: bool,
    screen_half_width: f32,
    screen_half_height: f32,
}

impl WaveManager {
    pub fn new(base_dirs: Vec<PathBuf>) -> Self {
        let mut slots = Vec::with_capacity(MAX_WAVE_SLOTS);
        for _ in 0..MAX_WAVE_SLOTS {
            slots.push(WaveSlot::empty());
        }

        Self {
            slots,
            base_dirs,
            master_volume: 1.0,
            play_enabled: true,
            screen_half_width: 400.0,
            screen_half_height: 300.0,
        }
    }

    pub fn set_screen_size(&mut self, width: u32, height: u32) {
        self.screen_half_width = width as f32 / 2.0;
        self.screen_half_height = height as f32 / 2.0;
    }

    /// Load a WAV file into the next free slot. Returns slot index or None.
    pub fn load(&mut self, filename: &str) -> Option<usize> {
        // Find free slot
        let slot_idx = self
            .slots
            .iter()
            .position(|s| s.status == WaveStatus::Empty)?;

        // Search base directories for the file
        let mut full_path = None;
        for dir in &self.base_dirs {
            let path = dir.join(filename);
            if path.exists() {
                full_path = Some(path);
                break;
            }
        }

        let path = full_path?;
        let data = std::fs::read(&path).ok()?;

        self.slots[slot_idx] = WaveSlot {
            status: WaveStatus::Stopped,
            filename: path,
            data,
            sink: None,
            position: (0.0, 0.0),
            instance_count: 0,
        };

        Some(slot_idx)
    }

    /// Play a sound once at a screen position.
    pub fn play_once(
        &mut self,
        slot: usize,
        x: i32,
        y: i32,
        stream_handle: &OutputStreamHandle,
    ) -> bool {
        if !self.play_enabled || slot >= MAX_WAVE_SLOTS {
            return false;
        }

        let wave = &mut self.slots[slot];
        if wave.status == WaveStatus::Empty {
            return false;
        }

        // Check if position is within screen bounds
        let fx = x as f32 - self.screen_half_width;
        let fy = y as f32 - self.screen_half_height;
        if fx.abs() > self.screen_half_width || fy.abs() > self.screen_half_height {
            return false;
        }

        if wave.instance_count >= MAX_ONESHOT_INSTANCES as u32 {
            return false;
        }

        wave.position = (fx, fy);

        // Create a new sink and play
        if let Ok(sink) = Sink::try_new(stream_handle) {
            let cursor = std::io::Cursor::new(wave.data.clone());
            if let Ok(source) = Decoder::new(cursor) {
                // Apply simple panning based on x position
                let pan = (fx / self.screen_half_width).clamp(-1.0, 1.0);
                // rodio doesn't have built-in panning, so we just adjust volume
                let distance = (fx * fx + fy * fy).sqrt()
                    / (self.screen_half_width * self.screen_half_width
                        + self.screen_half_height * self.screen_half_height)
                        .sqrt();
                let volume = (1.0 - distance * 0.5) * self.master_volume;
                sink.set_volume(volume.max(0.0));
                sink.append(source);
                wave.sink = Some(sink);
                wave.status = WaveStatus::Playing;
                wave.instance_count += 1;
                return true;
            }
        }
        false
    }

    /// Play a sound in a loop at a screen position.
    pub fn play_loop(
        &mut self,
        slot: usize,
        x: i32,
        y: i32,
        stream_handle: &OutputStreamHandle,
    ) -> bool {
        if !self.play_enabled || slot >= MAX_WAVE_SLOTS {
            return false;
        }

        let wave = &mut self.slots[slot];
        if wave.status == WaveStatus::Empty {
            return false;
        }

        if wave.instance_count >= MAX_LOOP_INSTANCES as u32 {
            return false;
        }

        wave.position = (x as f32, y as f32);

        if let Ok(sink) = Sink::try_new(stream_handle) {
            let cursor = std::io::Cursor::new(wave.data.clone());
            if let Ok(source) = Decoder::new(cursor) {
                sink.set_volume(self.master_volume);
                sink.append(source.repeat_infinite());
                wave.sink = Some(sink);
                wave.status = WaveStatus::Looping;
                wave.instance_count += 1;
                return true;
            }
        }
        false
    }

    pub fn stop(&mut self, slot: usize) -> bool {
        if slot >= MAX_WAVE_SLOTS {
            return false;
        }
        let wave = &mut self.slots[slot];
        if let Some(ref sink) = wave.sink {
            sink.pause();
        }
        wave.status = WaveStatus::Stopped;
        true
    }

    pub fn resume(&mut self, slot: usize) -> bool {
        if slot >= MAX_WAVE_SLOTS {
            return false;
        }
        let wave = &mut self.slots[slot];
        if let Some(ref sink) = wave.sink {
            sink.play();
            wave.status = WaveStatus::Playing;
            return true;
        }
        false
    }

    pub fn status(&self, slot: usize) -> WaveStatus {
        if slot >= MAX_WAVE_SLOTS {
            return WaveStatus::Empty;
        }
        self.slots[slot].status
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.master_volume = volume;
        for slot in &self.slots {
            if let Some(ref sink) = slot.sink {
                sink.set_volume(volume);
            }
        }
    }

    pub fn get_volume(&self) -> f32 {
        self.master_volume
    }

    pub fn set_play_enabled(&mut self, enabled: bool) {
        self.play_enabled = enabled;
    }

    pub fn destroy(&mut self, slot: usize) {
        if slot >= MAX_WAVE_SLOTS {
            return;
        }
        if let Some(sink) = self.slots[slot].sink.take() {
            sink.stop();
        }
        self.slots[slot] = WaveSlot::empty();
    }

    pub fn destroy_all(&mut self) {
        for slot in &mut self.slots {
            if let Some(sink) = slot.sink.take() {
                sink.stop();
            }
            *slot = WaveSlot::empty();
        }
    }

    /// Per-frame cleanup: remove finished one-shot sounds.
    pub fn work_events(&mut self) {
        for slot in &mut self.slots {
            if slot.status == WaveStatus::Playing {
                if let Some(ref sink) = slot.sink {
                    if sink.empty() {
                        slot.status = WaveStatus::Stopped;
                        slot.instance_count = slot.instance_count.saturating_sub(1);
                    }
                }
            }
        }
    }
}

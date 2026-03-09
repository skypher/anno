//! Streaming audio (music) playback.
//!
//! Corresponds to the MaxstreamXxx functions in Maxsound.dll.
//! Supports up to 8 simultaneous streams with optional IMA ADPCM decoding.

use crate::adpcm::{self, AdpcmState};
use rodio::source::Source;
use rodio::{Decoder, OutputStreamHandle, Sink};
use std::path::{Path, PathBuf};

/// Maximum number of stream slots (matching original engine).
pub const MAX_STREAM_SLOTS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamStatus {
    Empty,
    Stopped,
    Playing,
}

pub struct StreamSlot {
    pub status: StreamStatus,
    pub filename: PathBuf,
    pub volume: f32,
    pub sink: Option<Sink>,
    pub is_adpcm: bool,
}

impl StreamSlot {
    fn empty() -> Self {
        Self {
            status: StreamStatus::Empty,
            filename: PathBuf::new(),
            volume: 1.0,
            sink: None,
            is_adpcm: false,
        }
    }
}

/// Music stream manager.
pub struct StreamManager {
    slots: Vec<StreamSlot>,
    base_dirs: Vec<PathBuf>,
}

impl StreamManager {
    pub fn new(base_dirs: Vec<PathBuf>) -> Self {
        let mut slots = Vec::with_capacity(MAX_STREAM_SLOTS);
        for _ in 0..MAX_STREAM_SLOTS {
            slots.push(StreamSlot::empty());
        }
        Self { slots, base_dirs }
    }

    /// Create a stream from a file. Returns slot index or None.
    pub fn create(&mut self, filename: &str, dir_index: usize) -> Option<usize> {
        let slot_idx = self
            .slots
            .iter()
            .position(|s| s.status == StreamStatus::Empty)?;

        let dir = self.base_dirs.get(dir_index)?;
        let path = dir.join(filename);
        if !path.exists() {
            return None;
        }

        self.slots[slot_idx] = StreamSlot {
            status: StreamStatus::Stopped,
            filename: path,
            volume: 1.0,
            sink: None,
            is_adpcm: false,
        };

        Some(slot_idx)
    }

    /// Start playing a stream.
    pub fn play(&mut self, slot: usize, volume: f32, handle: &OutputStreamHandle) -> bool {
        if slot >= MAX_STREAM_SLOTS {
            return false;
        }

        let stream = &mut self.slots[slot];
        if stream.status == StreamStatus::Empty {
            return false;
        }

        if stream.status == StreamStatus::Playing {
            return true;
        }

        let file = match std::fs::File::open(&stream.filename) {
            Ok(f) => f,
            Err(_) => return false,
        };

        let reader = std::io::BufReader::new(file);
        match Decoder::new(reader) {
            Ok(source) => {
                if let Ok(sink) = Sink::try_new(handle) {
                    sink.set_volume(volume);
                    sink.append(source);
                    stream.sink = Some(sink);
                    stream.status = StreamStatus::Playing;
                    stream.volume = volume;
                    return true;
                }
            }
            Err(_) => return false,
        }

        false
    }

    pub fn stop(&mut self, slot: usize) -> bool {
        if slot >= MAX_STREAM_SLOTS {
            return false;
        }
        let stream = &mut self.slots[slot];
        if let Some(ref sink) = stream.sink {
            sink.stop();
        }
        stream.sink = None;
        stream.status = StreamStatus::Stopped;
        true
    }

    pub fn resume(&mut self, slot: usize) -> bool {
        if slot >= MAX_STREAM_SLOTS {
            return false;
        }
        let stream = &mut self.slots[slot];
        if let Some(ref sink) = stream.sink {
            sink.play();
            stream.status = StreamStatus::Playing;
            return true;
        }
        false
    }

    pub fn status(&self, slot: usize) -> StreamStatus {
        if slot >= MAX_STREAM_SLOTS {
            return StreamStatus::Empty;
        }
        // Check if a playing stream has finished naturally
        if self.slots[slot].status == StreamStatus::Playing {
            if let Some(ref sink) = self.slots[slot].sink {
                if sink.empty() {
                    return StreamStatus::Stopped;
                }
            }
        }
        self.slots[slot].status
    }

    pub fn set_volume(&mut self, slot: usize, volume: f32) {
        if slot >= MAX_STREAM_SLOTS {
            return;
        }
        self.slots[slot].volume = volume;
        if let Some(ref sink) = self.slots[slot].sink {
            sink.set_volume(volume);
        }
    }

    pub fn get_volume(&self, slot: usize) -> f32 {
        if slot >= MAX_STREAM_SLOTS {
            return 0.0;
        }
        self.slots[slot].volume
    }

    pub fn destroy(&mut self, slot: usize) {
        if slot >= MAX_STREAM_SLOTS {
            return;
        }
        if let Some(sink) = self.slots[slot].sink.take() {
            sink.stop();
        }
        self.slots[slot] = StreamSlot::empty();
    }
}

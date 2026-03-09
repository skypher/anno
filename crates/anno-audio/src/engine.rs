//! Top-level audio engine combining wave and stream managers.
//!
//! Corresponds to MaxsoundInit/MaxsoundClr/MaxsoundSleep/MaxsoundWakeUp.

use crate::stream::StreamManager;
use crate::wave::WaveManager;
use rodio::{OutputStream, OutputStreamHandle};
use std::path::PathBuf;

/// The main audio engine, wrapping wave effects and music streams.
pub struct AudioEngine {
    pub waves: WaveManager,
    pub streams: StreamManager,
    _output_stream: Option<OutputStream>,
    pub stream_handle: Option<OutputStreamHandle>,
    sleeping: bool,
}

impl AudioEngine {
    /// Initialize the audio engine with base directories for file loading.
    pub fn new(base_dirs: Vec<PathBuf>) -> Self {
        let (output_stream, stream_handle) = match OutputStream::try_default() {
            Ok((s, h)) => (Some(s), Some(h)),
            Err(e) => {
                eprintln!("Warning: failed to initialize audio output: {e}");
                (None, None)
            }
        };

        Self {
            waves: WaveManager::new(base_dirs.clone()),
            streams: StreamManager::new(base_dirs),
            _output_stream: output_stream,
            stream_handle,
            sleeping: false,
        }
    }

    pub fn set_screen_size(&mut self, width: u32, height: u32) {
        self.waves.set_screen_size(width, height);
    }

    /// Suspend the audio engine (e.g., on focus loss).
    pub fn sleep(&mut self) {
        if self.sleeping {
            return;
        }
        self.waves.set_play_enabled(false);
        // Stop all streams but remember which were playing
        for i in 0..crate::stream::MAX_STREAM_SLOTS {
            if self.streams.status(i) == crate::stream::StreamStatus::Playing {
                self.streams.stop(i);
            }
        }
        self.sleeping = true;
    }

    /// Resume the audio engine after sleep.
    pub fn wake_up(&mut self) {
        if !self.sleeping {
            return;
        }
        self.waves.set_play_enabled(true);
        self.sleeping = false;
    }

    /// Per-frame tick: clean up finished sounds.
    pub fn work_events(&mut self) {
        self.waves.work_events();
    }

    /// Full teardown.
    pub fn clear(&mut self) {
        self.waves.destroy_all();
        for i in 0..crate::stream::MAX_STREAM_SLOTS {
            self.streams.destroy(i);
        }
    }
}

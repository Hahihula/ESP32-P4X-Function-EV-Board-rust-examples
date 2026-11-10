//! Audio level measurement: RMS and peak of a block of samples, in dBFS,
//! and a text bar to show it in a console.
//!
//! Hardware-independent: builds and is unit-tested on a PC.

use core::fmt;

/// The level of a block of 16-bit samples.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Level {
    /// Root mean square, relative to full scale (0.0..=1.0).
    pub rms: f32,
    /// Largest absolute sample, relative to full scale (0.0..=1.0).
    pub peak: f32,
}

impl Level {
    /// RMS level in dBFS (0 dB = a full-scale square wave).
    pub fn rms_db(&self) -> f32 {
        dbfs(self.rms)
    }

    /// Peak level in dBFS.
    pub fn peak_db(&self) -> f32 {
        dbfs(self.peak)
    }
}

/// Collects samples and reports their [`Level`] once enough have been seen.
#[derive(Clone, Debug)]
pub struct LevelMeter {
    block: u32,
    count: u32,
    sum_squares: u64,
    peak: u16,
}

impl LevelMeter {
    /// A meter that reports every `block` samples (e.g. the sample rate / 10
    /// for ten reports per second).
    pub const fn new(block: u32) -> Self {
        Self {
            block,
            count: 0,
            sum_squares: 0,
            peak: 0,
        }
    }

    /// Adds a sample. Returns the level of the block when this sample
    /// completes it, and starts a new block.
    pub fn push(&mut self, sample: i16) -> Option<Level> {
        let magnitude = sample.unsigned_abs();
        self.sum_squares += u64::from(magnitude) * u64::from(magnitude);
        self.peak = self.peak.max(magnitude);
        self.count += 1;
        if self.count < self.block {
            return None;
        }
        let mean_square = self.sum_squares as f32 / self.count as f32;
        let level = Level {
            rms: libm::sqrtf(mean_square) / FULL_SCALE,
            peak: f32::from(self.peak) / FULL_SCALE,
        };
        self.count = 0;
        self.sum_squares = 0;
        self.peak = 0;
        Some(level)
    }
}

const FULL_SCALE: f32 = 32768.0;

/// The quietest level reported, in dBFS; silence reads as this.
pub const FLOOR_DB: f32 = -96.0;

/// Converts a level relative to full scale into dBFS, at least [`FLOOR_DB`].
pub fn dbfs(ratio: f32) -> f32 {
    if ratio <= 0.0 {
        FLOOR_DB
    } else {
        (20.0 * libm::log10f(ratio)).max(FLOOR_DB)
    }
}

/// A text bar for a level: `#` for every 3 dB above `floor_db`, filled up to
/// `width` characters with `.`.
///
/// ```
/// use esp32_p4_function_ev::level::Bar;
/// assert_eq!(Bar::new(-30.0, -60.0, 12).to_string(), "##########..");
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Bar {
    filled: usize,
    width: usize,
}

impl Bar {
    /// dB per bar character.
    pub const DB_PER_CHAR: f32 = 3.0;

    /// A bar of `width` characters for `db`, starting at `floor_db`.
    pub fn new(db: f32, floor_db: f32, width: usize) -> Self {
        let chars = ((db - floor_db) / Self::DB_PER_CHAR).max(0.0) as usize;
        Self {
            filled: chars.min(width),
            width,
        }
    }
}

impl fmt::Display for Bar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for i in 0..self.width {
            f.write_str(if i < self.filled { "#" } else { "." })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::string::ToString;

    fn measure(samples: impl IntoIterator<Item = i16>, block: u32) -> Level {
        let mut meter = LevelMeter::new(block);
        samples
            .into_iter()
            .find_map(|s| meter.push(s))
            .expect("block completed")
    }

    #[test]
    fn silence_is_at_the_floor() {
        let level = measure([0; 100], 100);
        assert_eq!(level.rms_db(), FLOOR_DB);
        assert_eq!(level.peak_db(), FLOOR_DB);
    }

    #[test]
    fn full_scale_square_wave_is_0_db() {
        let level = measure(
            (0..100).map(|i| if i % 2 == 0 { i16::MAX } else { i16::MIN }),
            100,
        );
        assert!(level.rms_db().abs() < 0.01, "{}", level.rms_db());
        assert!(level.peak_db().abs() < 0.01, "{}", level.peak_db());
    }

    #[test]
    fn sine_rms_is_3_db_below_its_peak() {
        let level = measure(
            (0..4800).map(|i| (libm::sinf(i as f32 * 0.1) * 16384.0) as i16),
            4800,
        );
        assert!((level.peak_db() - -6.02).abs() < 0.1, "{}", level.peak_db());
        assert!((level.peak_db() - level.rms_db() - 3.01).abs() < 0.1);
    }

    #[test]
    fn reports_once_per_block_and_restarts() {
        let mut meter = LevelMeter::new(4);
        let reports: std::vec::Vec<_> = [1000, 0, 0, 0, 0, 0, 0, 0]
            .into_iter()
            .filter_map(|s| meter.push(s))
            .collect();
        assert_eq!(reports.len(), 2);
        assert!(reports[0].peak > 0.0);
        assert_eq!(reports[1].peak, 0.0);
    }

    #[test]
    fn bar_is_clamped_to_its_width() {
        assert_eq!(Bar::new(-100.0, -60.0, 5).to_string(), ".....");
        assert_eq!(Bar::new(-54.0, -60.0, 5).to_string(), "##...");
        assert_eq!(Bar::new(10.0, -60.0, 5).to_string(), "#####");
    }
}

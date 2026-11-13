//! Demo 06: the speaker (I2S, ES8311 DAC, NS4150B amplifier, 44.1 kHz).
//!
//! Starts the I2S clocks, configures the codec's DAC, switches the amplifier
//! on and plays a chime. Each BOOT press then plays the next tune (scale, Ode
//! to Joy, chime).

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use core::f32::consts::TAU;

use embassy_executor::Spawner;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp32_p4_function_ev::{
    audio::{
        self, AudioOut, Error,
        es8311::{self, Config},
    },
    board, i2s_out_pins,
};
use log::{error, info, warn};

esp_bootloader_esp_idf::esp_app_desc!();

const SAMPLE_RATE: u32 = 44_100;
/// DAC volume. -20 dB is clearly audible without being loud.
const VOLUME_DB: i8 = -20;
/// Peak sample value of the generated sine.
const AMPLITUDE: f32 = 12_000.0;

/// A note: frequency in Hz (0 = rest) and length in ms.
#[derive(Clone, Copy)]
struct Note(f32, u32);

const C4: f32 = 261.63;
const D4: f32 = 293.66;
const E4: f32 = 329.63;
const F4: f32 = 349.23;
const G4: f32 = 392.00;
const A4: f32 = 440.00;
const B4: f32 = 493.88;
const C5: f32 = 523.25;
const E5: f32 = 659.25;
const G5: f32 = 783.99;

const CHIME: &[Note] = &[Note(C5, 120), Note(E5, 120), Note(G5, 300)];
const SCALE: &[Note] = &[
    Note(C4, 250),
    Note(D4, 250),
    Note(E4, 250),
    Note(F4, 250),
    Note(G4, 250),
    Note(A4, 250),
    Note(B4, 250),
    Note(C5, 500),
];
/// Beethoven, Ode to Joy (first phrase).
const ODE_TO_JOY: &[Note] = &[
    Note(E4, 300),
    Note(E4, 300),
    Note(F4, 300),
    Note(G4, 300),
    Note(G4, 300),
    Note(F4, 300),
    Note(E4, 300),
    Note(D4, 300),
    Note(C4, 300),
    Note(C4, 300),
    Note(D4, 300),
    Note(E4, 300),
    Note(E4, 450),
    Note(D4, 150),
    Note(D4, 600),
];
const TUNES: [(&str, &[Note]); 3] = [
    ("scale", SCALE),
    ("Ode to Joy", ODE_TO_JOY),
    ("chime", CHIME),
];

static PLAY: Channel<CriticalSectionRawMutex, &'static [Note], 2> = Channel::new();

/// A sine voice with a short fade in and out per note, so notes don't click.
struct Synth {
    notes: &'static [Note],
    index: usize,
    /// Samples of the current note already rendered.
    pos: u32,
    phase: f32,
}

impl Synth {
    const FADE: u32 = SAMPLE_RATE / 100; // 10 ms

    const fn new() -> Self {
        Self {
            notes: &[],
            index: 0,
            pos: 0,
            phase: 0.0,
        }
    }

    fn play(&mut self, notes: &'static [Note]) {
        *self = Self {
            notes,
            ..Self::new()
        };
    }

    /// Fills `buf` with the next samples; silence once the tune is over.
    fn render(&mut self, buf: &mut [i16]) {
        for out in buf {
            let Some(&Note(freq, ms)) = self.notes.get(self.index) else {
                *out = 0;
                continue;
            };
            let len = SAMPLE_RATE / 1000 * ms;
            let fade = self.pos.min(len - self.pos).min(Self::FADE) as f32 / Self::FADE as f32;
            *out = if freq > 0.0 {
                (libm::sinf(self.phase) * AMPLITUDE * fade) as i16
            } else {
                0
            };
            self.phase += TAU * freq / SAMPLE_RATE as f32;
            if self.phase >= TAU {
                self.phase -= TAU;
            }
            self.pos += 1;
            if self.pos >= len {
                self.pos = 0;
                self.index += 1;
            }
        }
    }
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp32_p4_function_ev_examples::start!(p, "Demo 06: speaker (ES8311, 44.1 kHz)");

    // I2S first: the codec runs from MCLK and needs it before setup.
    match AudioOut::new(p.I2S0, p.DMA_CH0, i2s_out_pins!(p), SAMPLE_RATE) {
        Ok(out) => spawner.spawn(audio_task(out).expect("spawn audio task")),
        Err(e) => error!("I2S init failed: {e:?}"),
    }
    Timer::after(Duration::from_millis(20)).await;

    let mut i2c = board::i2c(p.I2C0, p.GPIO8, p.GPIO7).expect("I2C config is valid");
    let mut codec = audio::codec(&mut i2c);
    match codec.chip_id().await {
        Ok(es8311::CHIP_ID) => info!("ES8311 chip ID {:02X?}", es8311::CHIP_ID),
        Ok(id) => warn!("unexpected chip ID {id:02X?}"),
        Err(e) => error!("ES8311 did not answer: {e:?}"),
    }
    let config = Config {
        sample_rate: SAMPLE_RATE,
        mclk_hz: 256 * SAMPLE_RATE,
    };
    let setup = async {
        codec.init(config).await?;
        codec.set_dac_volume_db(VOLUME_DB).await?;
        codec.set_dac_mute(false).await
    };
    match setup.await {
        Ok(()) => info!("ES8311 DAC running, volume {VOLUME_DB} dB"),
        Err(e) => error!("ES8311 init failed: {e:?}"),
    }

    // Amplifier last, so the codec's power-up doesn't pop in the speaker.
    let _amp = board::speaker_amp_on(p.GPIO53);
    PLAY.send(CHIME).await;
    info!("BOOT = next tune (scale, Ode to Joy, chime)");

    let mut button = board::boot_button(p.GPIO35);
    let mut next = 0;
    loop {
        button.wait_for_falling_edge().await;
        let (name, notes) = TUNES[next];
        next = (next + 1) % TUNES.len();
        info!("playing {name}");
        PLAY.send(notes).await;
        // Ignore contact bounce, then wait for the release.
        Timer::after(Duration::from_millis(20)).await;
        button.wait_for_high().await;
        Timer::after(Duration::from_millis(20)).await;
    }
}

/// Keeps the I2S ring filled (it holds ~46 ms).
#[embassy_executor::task]
async fn audio_task(mut out: AudioOut<'static>) {
    let mut synth = Synth::new();
    loop {
        if let Ok(notes) = PLAY.try_receive() {
            synth.play(notes);
        }
        if let Err(e) = out.fill(|buf| synth.render(buf)).await {
            match e {
                Error::Dma(e) => warn!("audio ring ran dry, restarted: {e:?}"),
                e => {
                    error!("audio stream stopped: {e:?}");
                    return;
                }
            }
        }
    }
}

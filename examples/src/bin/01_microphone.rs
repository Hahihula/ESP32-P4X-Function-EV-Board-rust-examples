//! Demo 01: microphone level meter.
//!
//! Records the on-board microphone through the ES8311 codec (I2S at 32 kHz)
//! and prints its level ten times per second: RMS and peak in dBFS and a
//! bar. The microphone pre-amplifier starts at its maximum gain, 30 dB (as
//! in Espressif's C driver); each press of BOOT lowers it by 6 dB, and after
//! 0 dB it goes back to 30 dB.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use core::sync::atomic::{AtomicU8, Ordering};

use embassy_executor::Spawner;
use embassy_time::{Delay, Duration, Timer};
use esp_backtrace as _;
use esp_hal::gpio::Input;
use esp32_p4_function_ev::{
    audio::{
        self, AudioIn,
        es8311::{MicGain, Mode, SampleRate},
    },
    board,
    level::{Bar, LevelMeter},
};
use log::{info, warn};

esp_bootloader_esp_idf::esp_app_desc!();

const RATE: SampleRate = SampleRate::Hz32000;
/// Gain step per BOOT press, and the maximum (the start value).
const GAIN_STEP_DB: u8 = 6;
const GAIN_MAX_DB: u8 = 30;
/// The bar starts at this level and has one character per 3 dB.
const BAR_FLOOR_DB: f32 = -90.0;
const BAR_WIDTH: usize = 30;

/// Microphone gain in dB, set by the BOOT button task.
static GAIN_DB: AtomicU8 = AtomicU8::new(GAIN_MAX_DB);

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp32_p4_function_ev_examples::start!(p, "Demo 01: microphone");

    let mut i2c = board::i2c(p.I2C0, p.GPIO8, p.GPIO7).unwrap();
    // The codec runs from the I2S bit clock, so start the stream first.
    let mut mic = AudioIn::new(
        p.I2S0,
        p.DMA_CH0,
        esp32_p4_function_ev::i2s_in_pins!(p),
        RATE,
    )
    .unwrap();
    let mut codec = audio::codec(&mut i2c);
    codec.init(&mut Delay).await.unwrap();
    codec.set_mode(Mode::Record).await.unwrap();
    let mut gain_db = GAIN_DB.load(Ordering::Relaxed);
    codec.set_mic_gain(MicGain::from_db(gain_db)).await.unwrap();

    spawner.spawn(boot_button(board::boot_button(p.GPIO35)).unwrap());

    info!(
        "Recording at {} Hz, mic gain {gain_db} dB. Speak, clap or whistle; press BOOT for less gain.",
        RATE.hz()
    );
    let mut meter = LevelMeter::new(RATE.hz() / 10);
    loop {
        let wanted = GAIN_DB.load(Ordering::Relaxed);
        if wanted != gain_db {
            gain_db = wanted;
            match codec.set_mic_gain(MicGain::from_db(gain_db)).await {
                Ok(()) => info!("Mic gain {gain_db} dB"),
                Err(e) => warn!("Setting the mic gain failed: {e:?}"),
            }
        }

        let mut report = None;
        let result = mic
            .read(|sample| {
                if let Some(level) = meter.push(sample) {
                    report = Some(level);
                }
            })
            .await;
        if let Err(e) = result {
            warn!("Recording: {e:?}");
        }
        if let Some(level) = report {
            info!(
                "{:6.1} dBFS  peak {:6.1}  |{}|",
                level.rms_db(),
                level.peak_db(),
                Bar::new(level.rms_db(), BAR_FLOOR_DB, BAR_WIDTH)
            );
        }
    }
}

/// Lowers the microphone gain one step on every BOOT press, wrapping from
/// 0 dB back to the maximum.
#[embassy_executor::task]
async fn boot_button(mut button: Input<'static>) {
    loop {
        button.wait_for_falling_edge().await;
        let gain = GAIN_DB.load(Ordering::Relaxed);
        let next = gain.checked_sub(GAIN_STEP_DB).unwrap_or(GAIN_MAX_DB);
        GAIN_DB.store(next, Ordering::Relaxed);
        // Ignore contact bounce, then wait for the release.
        Timer::after(Duration::from_millis(20)).await;
        button.wait_for_high().await;
        Timer::after(Duration::from_millis(20)).await;
    }
}

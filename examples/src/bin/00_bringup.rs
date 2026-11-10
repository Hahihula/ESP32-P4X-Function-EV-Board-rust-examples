//! Demo 00: board bring-up.
//!
//! Prints the chip revision and MAC address from eFuse, then logs a
//! heartbeat every ten seconds. Checks that the toolchain, flashing and the
//! console work.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use esp_backtrace as _;
use esp_hal::efuse;
use log::{info, warn};

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    esp32_p4_function_ev_examples::start!(_peripherals, "ESP32-P4X-Function-EV-Board bring-up");

    print_chip_info();

    info!("Bring-up done. A heartbeat line follows every 10 s.");
    loop {
        Timer::after(Duration::from_secs(10)).await;
        info!("alive, uptime {} s", Instant::now().as_secs());
    }
}

fn print_chip_info() {
    let rev = efuse::chip_revision();
    info!("Chip revision: v{}.{}", rev.major, rev.minor);
    // The "X" board carries chip revision v3.1 or later; the older
    // ESP32-P4-Function-EV-Board has a v1.x chip.
    if rev.major < 3 {
        warn!("This is not a v3.x chip: esp-hal's ESP32-P4 support targets v3.x");
    }
    info!("Base MAC address: {}", efuse::base_mac_address());
}

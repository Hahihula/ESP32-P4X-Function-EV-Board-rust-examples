//! Demo 00: board bring-up.
//!
//! Prints the chip revision and MAC address from eFuse, tests the PSRAM and
//! adds it to the heap, scans the shared I2C bus (SCL GPIO8, SDA GPIO7) and
//! reads an ID from every device it expects there, then logs presses of the
//! BOOT button and a heartbeat every ten seconds.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

extern crate alloc;

use alloc::vec::Vec;
use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use esp_backtrace as _;
use esp_hal::{
    Async, efuse,
    gpio::Input,
    i2c::master::{AcknowledgeCheckFailedReason, Error as I2cError, I2c},
    psram::Psram,
};
use esp32_p4_function_ev::board::{self, i2c_addr};
use log::{info, warn};

esp_bootloader_esp_idf::esp_app_desc!();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp32_p4_function_ev_examples::start!(peripherals, "ESP32-P4X-Function-EV-Board bring-up");

    print_chip_info();

    let psram = board::psram(peripherals.PSRAM);
    test_psram(&psram);
    esp_alloc::psram_allocator!(&psram);
    check_heap();

    match board::i2c(peripherals.I2C0, peripherals.GPIO8, peripherals.GPIO7) {
        Ok(mut i2c) => {
            scan_i2c(&mut i2c).await;
            identify_devices(&mut i2c).await;
        }
        Err(e) => warn!("I2C init failed: {e:?}"),
    }

    spawner.spawn(boot_button(board::boot_button(peripherals.GPIO35)).unwrap());

    info!("Bring-up done. Press BOOT; a heartbeat line follows every 10 s.");
    loop {
        Timer::after(Duration::from_secs(10)).await;
        info!("alive, uptime {} s", Instant::now().as_secs());
    }
}

/// Logs every press and release of the BOOT button, with the press length.
#[embassy_executor::task]
async fn boot_button(mut button: Input<'static>) {
    let mut presses = 0u32;
    loop {
        button.wait_for_falling_edge().await;
        let pressed_at = Instant::now();
        presses += 1;
        info!("BOOT pressed ({presses})");
        // Ignore contact bounce right after the edge.
        Timer::after(Duration::from_millis(20)).await;
        button.wait_for_high().await;
        info!(
            "BOOT released after {} ms",
            pressed_at.elapsed().as_millis()
        );
        Timer::after(Duration::from_millis(20)).await;
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

/// Writes an address-derived pattern over the whole PSRAM and reads it back.
fn test_psram(psram: &Psram) {
    let (start, size) = psram.raw_parts();
    info!("PSRAM: {} KiB mapped at {:p}", size / 1024, start);
    if size == 0 {
        warn!("PSRAM not detected");
        return;
    }

    let words = size / 4;
    let base = start.cast::<u32>();
    let pattern = |i: usize| (i as u32).wrapping_mul(0x9E37_79B9) ^ 0xA5A5_5A5A;

    let t0 = Instant::now();
    for i in 0..words {
        // SAFETY: `i` stays inside the mapped PSRAM region, which nothing else uses yet.
        unsafe { base.add(i).write_volatile(pattern(i)) };
    }
    let write_ms = t0.elapsed().as_millis();

    let t0 = Instant::now();
    let errors = (0..words)
        // SAFETY: as above.
        .filter(|&i| unsafe { base.add(i).read_volatile() } != pattern(i))
        .count();
    let read_ms = t0.elapsed().as_millis();

    let mib = size as u64 / (1024 * 1024);
    info!(
        "PSRAM test: {errors} bad words, write {write_ms} ms (~{} MiB/s), read {read_ms} ms (~{} MiB/s)",
        mib * 1000 / write_ms.max(1),
        mib * 1000 / read_ms.max(1),
    );
}

/// Allocates 1 MiB, more than the internal heap holds, so it comes from PSRAM.
fn check_heap() {
    let buf: Vec<u8> = alloc::vec![0x5A; 1024 * 1024];
    let ok = buf.iter().all(|&b| b == 0x5A);
    info!(
        "Heap: 1 MiB allocation at {:p} {}",
        buf.as_ptr(),
        if ok { "OK" } else { "CORRUPT" }
    );
    drop(buf);
    info!("{}", esp_alloc::HEAP.stats());
}

#[allow(
    clippy::large_stack_frames,
    reason = "the async I2C transfer future is ~1 KiB; awaited once from main"
)]
async fn scan_i2c(i2c: &mut I2c<'_, Async>) {
    info!("Scanning I2C bus (SCL=GPIO8, SDA=GPIO7)...");
    let mut found: u128 = 0;
    for addr in 0x08u8..0x78 {
        let mut byte = [0u8; 1];
        match i2c.read_async(addr, &mut byte).await {
            Ok(()) => found |= 1 << addr,
            Err(I2cError::AcknowledgeCheckFailed(AcknowledgeCheckFailedReason::Address)) => {}
            // The address was acknowledged, only the read was refused.
            Err(I2cError::AcknowledgeCheckFailed(_)) => found |= 1 << addr,
            Err(e) => warn!("  0x{addr:02X}: error {e:?}"),
        }
    }

    for addr in (0x08u8..0x78).filter(|&a| found & (1 << a) != 0) {
        info!("  0x{addr:02X}: {}", device_name(addr));
    }
    info!("I2C scan: {} device(s)", found.count_ones());

    if found & (1 << i2c_addr::ES8311) == 0 {
        warn!(
            "  expected ES8311 at 0x{:02X} did not answer",
            i2c_addr::ES8311
        );
    }
    if found & (1 << i2c_addr::GT911 | 1 << i2c_addr::GT911_ALT) == 0 {
        warn!("  no GT911 touch controller (is the LCD connected and powered?)");
    }
}

fn device_name(addr: u8) -> &'static str {
    match addr {
        i2c_addr::ES8311 => "ES8311 audio codec",
        i2c_addr::GT911 | i2c_addr::GT911_ALT => "GT911 touch controller",
        _ => "unknown",
    }
}

#[allow(
    clippy::large_stack_frames,
    reason = "several async I2C transfer futures; awaited once from main"
)]
async fn identify_devices(i2c: &mut I2c<'_, Async>) {
    info!("Reading device IDs...");

    // ES8311: CHIP_ID1/2 at 0xFD/0xFE read 0x83 0x11, version at 0xFF.
    // The ES8311 doesn't auto-increment the register address, so each
    // register is read on its own.
    let mut id = [0u8; 3];
    let mut result = Ok(());
    for (reg, byte) in (0xFDu8..).zip(id.iter_mut()) {
        result = result.and(
            i2c.write_read_async(i2c_addr::ES8311, &[reg], core::slice::from_mut(byte))
                .await,
        );
    }
    match result {
        Ok(()) => info!(
            "  ES8311: chip ID {:02X} {:02X}, version {:02X} {}",
            id[0],
            id[1],
            id[2],
            check(id[..2] == [0x83, 0x11])
        ),
        Err(_) => warn!("  ES8311: no answer"),
    }

    // GT911: 16-bit register addresses. Product ID at 0x8140 is ASCII "911",
    // then the firmware version (0x8144, LE) and the configured X and Y
    // resolution (0x8146 / 0x8148, LE).
    for addr in [i2c_addr::GT911, i2c_addr::GT911_ALT] {
        let mut info = [0u8; 10];
        if i2c
            .write_read_async(addr, &[0x81, 0x40], &mut info)
            .await
            .is_ok()
        {
            let word = |i: usize| u16::from_le_bytes([info[i], info[i + 1]]);
            info!(
                "  GT911 at 0x{addr:02X}: product ID {:?}, firmware {:04X}, {}x{} {}",
                core::str::from_utf8(&info[..3]).unwrap_or("?"),
                word(4),
                word(6),
                word(8),
                check(&info[..3] == b"911")
            );
        }
    }
}

fn check(ok: bool) -> &'static str {
    if ok { "(OK)" } else { "(unexpected)" }
}

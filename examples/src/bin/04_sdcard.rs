//! Demo 04: microSD card.
//!
//! Brings up the card on the 4-bit SD bus and prints what it is, mounts its
//! FAT16/FAT32 file system and lists the root directory. Then it appends a
//! line to `P4BOOT.TXT` (so the count goes up with every run), and measures
//! write and read speed with a 1 MiB file, `P4BENCH.BIN`, which it checks
//! and deletes again.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

extern crate alloc;

use core::ops::ControlFlow;

use embassy_executor::Spawner;
use embassy_time::Instant;
use esp_backtrace as _;
use esp32_p4_function_ev::sdcard::{
    self, MmcError, SdCard,
    embedded_sdmmc::{self, Mode, TimeSource, Timestamp, VolumeIdx, VolumeManager},
};
use log::{info, warn};

esp_bootloader_esp_idf::esp_app_desc!();

const BOOT_LOG: &str = "P4BOOT.TXT";
const BENCH_FILE: &str = "P4BENCH.BIN";
const BENCH_BYTES: usize = 1024 * 1024;
const BENCH_CHUNK: usize = 4096;
/// Most root directory entries to list.
const LIST_MAX: usize = 20;

/// The board has no battery-backed clock, so files get this timestamp.
struct FixedTime;

impl TimeSource for FixedTime {
    fn get_timestamp(&self) -> Timestamp {
        // 2026-01-01 00:00:00
        Timestamp {
            year_since_1970: 56,
            zero_indexed_month: 0,
            zero_indexed_day: 0,
            hours: 0,
            minutes: 0,
            seconds: 0,
        }
    }
}

type Fs<'d> = VolumeManager<SdCard<'d>, FixedTime>;
type Dir<'a, 'd> = embedded_sdmmc::Directory<'a, SdCard<'d>, FixedTime, 4, 4, 1>;
type FsError = embedded_sdmmc::Error<MmcError>;

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    esp32_p4_function_ev_examples::start!(p, "Demo 04: microSD card");

    let host = sdcard::host(p.SDHOST).unwrap();
    let card = match SdCard::init(&host, esp32_p4_function_ev::sd_pins!(p)).await {
        Ok(card) => card,
        Err(e) => {
            warn!("No card: {e:?}. Insert a microSD card and press RST.");
            park().await
        }
    };
    print_card_info(&card);

    let fs: Fs<'_> = VolumeManager::new(card, FixedTime);
    match run(&fs) {
        Ok(()) => info!("Done. Press RST to run again."),
        Err(e) => warn!("File system error: {e:?} (is the card formatted FAT16 or FAT32?)"),
    }
    park().await
}

/// Waits forever (the demo has nothing left to do).
async fn park() -> ! {
    loop {
        embassy_time::Timer::after_secs(3600).await;
    }
}

fn print_card_info(card: &SdCard<'_>) {
    let info = card.card();
    let cid = info.cid;
    let (month, year) = cid.manufacturing_date();
    info!(
        "Card: manufacturer 0x{:02X}, OEM {:?}, product {:?} rev {}.{}, serial {:08X}, made {month}/{year}",
        cid.manufacturer_id(),
        cid.oem_id(),
        cid.product_name(),
        cid.product_revision() >> 4,
        cid.product_revision() & 0xF,
        cid.serial(),
    );
    let mib = card.size_bytes() / (1024 * 1024);
    info!(
        "Capacity {}.{} GiB, SD spec {:?}, 4-bit bus at {} MHz",
        mib / 1024,
        mib % 1024 * 10 / 1024,
        info.scr.version(),
        card.clock_hz() / 1_000_000
    );
}

fn run(fs: &Fs<'_>) -> Result<(), FsError> {
    let volume = fs.open_volume(VolumeIdx(0))?;
    let root = volume.open_root_dir()?;
    list(&root)?;
    append_boot_log(&root)?;
    benchmark(&root)?;
    Ok(())
}

fn list(root: &Dir<'_, '_>) -> Result<(), FsError> {
    info!("Root directory:");
    let mut count = 0;
    root.iterate_dir(|entry| {
        if count < LIST_MAX {
            if entry.attributes.is_directory() {
                info!("  {:>10}  {}", "<DIR>", entry.name);
            } else {
                info!("  {:>10}  {}", entry.size, entry.name);
            }
        }
        count += 1;
        ControlFlow::Continue(())
    })?;
    if count > LIST_MAX {
        info!("  ... and {} more", count - LIST_MAX);
    }
    Ok(())
}

/// Counts the lines already in the boot log, then appends one.
fn append_boot_log(root: &Dir<'_, '_>) -> Result<(), FsError> {
    let mut boots = 1;
    if let Ok(file) = root.open_file_in_dir(BOOT_LOG, Mode::ReadOnly) {
        let mut buf = [0u8; 128];
        while !file.is_eof() {
            let n = file.read(&mut buf)?;
            boots += buf[..n].iter().filter(|&&b| b == b'\n').count();
        }
    }
    let line = alloc::format!(
        "Boot {boots}, {} ms after reset\n",
        Instant::now().as_millis()
    );
    let file = root.open_file_in_dir(BOOT_LOG, Mode::ReadWriteCreateOrAppend)?;
    file.write(line.as_bytes())?;
    file.close()?;
    info!("Appended to {BOOT_LOG}: {}", line.trim_end());
    Ok(())
}

/// Writes a 1 MiB file with a known pattern, reads it back, checks it and
/// deletes it.
#[allow(
    clippy::large_stack_frames,
    reason = "the file system's block transfers are inlined here; called once from main"
)]
fn benchmark(root: &Dir<'_, '_>) -> Result<(), FsError> {
    let mut buf = alloc::vec![0u8; BENCH_CHUNK];
    let pattern = |offset: usize, chunk: &mut [u8]| {
        for (i, b) in chunk.iter_mut().enumerate() {
            *b = ((offset + i) as u32)
                .wrapping_mul(2_654_435_761)
                .to_le_bytes()[3];
        }
    };

    let started = Instant::now();
    let file = root.open_file_in_dir(BENCH_FILE, Mode::ReadWriteCreateOrTruncate)?;
    for offset in (0..BENCH_BYTES).step_by(BENCH_CHUNK) {
        pattern(offset, &mut buf);
        file.write(&buf)?;
    }
    file.close()?;
    let write_ms = started.elapsed().as_millis();

    let started = Instant::now();
    let file = root.open_file_in_dir(BENCH_FILE, Mode::ReadOnly)?;
    let mut expected = alloc::vec![0u8; BENCH_CHUNK];
    let (mut offset, mut bad_chunks) = (0, 0);
    while !file.is_eof() {
        let n = file.read(&mut buf)?;
        pattern(offset, &mut expected[..n]);
        if buf[..n] != expected[..n] {
            bad_chunks += 1;
        }
        offset += n;
    }
    file.close()?;
    let read_ms = started.elapsed().as_millis();
    root.delete_entry_in_dir(BENCH_FILE)?;

    let kib_per_s = |ms: u64| (BENCH_BYTES as u64 / 1024) * 1000 / ms.max(1);
    info!(
        "1 MiB file: write {write_ms} ms ({} KiB/s), read {read_ms} ms ({} KiB/s)",
        kib_per_s(write_ms),
        kib_per_s(read_ms),
    );
    if offset == BENCH_BYTES && bad_chunks == 0 {
        info!("Read back {offset} bytes, all match");
    } else {
        warn!("Read back {offset} bytes, {bad_chunks} chunks differ");
    }
    Ok(())
}

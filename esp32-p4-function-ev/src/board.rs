//! Board-level constants and common initialisation for the
//! ESP32-P4X-Function-EV-Board.
//!
//! | Function                          | GPIO                          |
//! |-----------------------------------|-------------------------------|
//! | BOOT button                       | 35 (active low, strapping pin)|
//! | Shared I2C SCL / SDA              | 8 / 7                         |
//! | I2S MCLK / BCLK / WS              | 13 / 12 / 10                  |
//! | I2S DOUT (to ES8311) / DIN (from) | 9 / 11                        |
//! | Speaker PA enable (NS4150B)       | 53 (active high)              |
//! | LCD backlight (jumper to J6 PWM)  | 26                            |
//! | LCD reset (jumper to J6 RST_LCD)  | 27                            |
//! | microSD CLK / CMD                 | 43 / 44                       |
//! | microSD D0 / D1 / D2 / D3         | 39 / 40 / 41 / 42             |
//! | UART0 TX / RX (header J1)         | 37 / 38                       |
//!
//! The touch controller on the LCD has neither its reset nor its interrupt
//! line connected, so it is polled over I2C.

use esp_hal::{
    Async, Config,
    clock::CpuClock,
    gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull},
    i2c::master::{Config as I2cConfig, ConfigError, I2c, Instance as I2cInstance},
    peripherals::{GPIO7, GPIO8, GPIO35, GPIO53, PSRAM, Peripherals},
    psram::{Psram, PsramConfig},
    time::Rate,
};

/// 7-bit addresses of the devices on the shared I2C bus (GPIO8 / GPIO7).
pub mod i2c_addr {
    /// ES8311 mono audio codec (speaker DAC and microphone ADC).
    pub const ES8311: u8 = 0x18;
    /// GT911 capacitive touch controller on the 7" LCD, at its default
    /// address. The GT911 picks 0x5D or 0x14 at reset, depending on its INT
    /// pin level.
    pub const GT911: u8 = crate::touch::ADDRESSES[0];
    /// The other address the GT911 can come up at.
    pub const GT911_ALT: u8 = crate::touch::ADDRESSES[1];
}

/// Initialises the HAL with the maximum CPU clock (400 MHz on chip
/// revision v3.x).
///
/// Logging and the heap are the application's choice:
///
/// ```ignore
/// let p = esp32_p4_function_ev::board::init();
/// esp32_p4_function_ev::board::init_logger(); // with the `log` feature
/// // Internal heap in the RAM the bootloader used, then PSRAM if wanted:
/// esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: esp32_p4_function_ev::board::RECLAIMED_HEAP);
/// esp32_p4_function_ev::board::psram_heap(p.PSRAM);
/// ```
///
/// The caller still has to start `esp_rtos` with a `TIMG0` timer and the
/// `FROM_CPU_INTR0` software interrupt.
#[must_use]
pub fn init() -> Peripherals {
    esp_hal::init(Config::default().with_cpu_clock(CpuClock::max()))
}

/// Size of the internal RAM the second-stage bootloader used and the
/// application can reclaim as heap (`#[esp_hal::ram(reclaimed)]`), in bytes.
pub const RECLAIMED_HEAP: usize = 69_632;

/// Logs through `esp-println`, at the level in the `ESP_LOG` environment
/// variable at build time. The output goes to the USB-Serial-JTAG port when
/// a host is connected to it, and to UART0 otherwise.
#[cfg(feature = "log")]
pub fn init_logger() {
    esp_println::logger::init_logger_from_env();
}

/// Waits two seconds so a serial monitor started right after reset catches
/// the first log lines. The USB-Serial-JTAG port drops off USB on every chip
/// reset, and anything printed before the host reconnects is lost.
pub async fn wait_for_console() {
    embassy_time::Timer::after(embassy_time::Duration::from_secs(2)).await;
}

/// Initialises the 32 MB in-package hex (16-line) PSRAM with esp-hal's
/// defaults: size detected from the chip, 250 MHz bus clock.
pub fn psram(psram: PSRAM<'static>) -> Psram {
    Psram::new(psram, PsramConfig::default())
}

/// Initialises PSRAM with [`psram`] and adds all of it to the heap, after any
/// internal-RAM regions: allocations that don't fit into internal RAM then
/// come from PSRAM. Call at most once. Returns the PSRAM size in bytes.
pub fn psram_heap(psram: PSRAM<'static>) -> usize {
    let psram = self::psram(psram);
    let (_, size) = psram.raw_parts();
    esp_alloc::psram_allocator!(&psram);
    size
}

/// Creates the async I2C master for the shared bus (audio codec, touch).
pub fn i2c<'d>(
    i2c: impl I2cInstance + 'd,
    scl: GPIO8<'d>,
    sda: GPIO7<'d>,
) -> Result<I2c<'d, Async>, ConfigError> {
    let config = I2cConfig::default().with_frequency(Rate::from_khz(400));
    Ok(I2c::new(i2c, config)?
        .with_scl(scl)
        .with_sda(sda)
        .into_async())
}

/// The BOOT button (GPIO35), the only user button on the board. Reads low
/// while pressed.
///
/// GPIO35 is a strapping pin: holding the button during reset enters the
/// ROM download mode instead of starting the firmware.
pub fn boot_button<'d>(pin: GPIO35<'d>) -> Input<'d> {
    Input::new(pin, InputConfig::default().with_pull(Pull::Up))
}

/// Switches the NS4150B speaker amplifier on (GPIO53, active high) for as
/// long as the returned pin is kept. Enable it after the codec's DAC is
/// running, so the codec's power-up doesn't pop in the speaker.
pub fn speaker_amp_on(pin: GPIO53<'_>) -> Output<'_> {
    Output::new(pin, Level::High, OutputConfig::default())
}

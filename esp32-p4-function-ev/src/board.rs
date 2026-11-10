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

use esp_hal::{Config, clock::CpuClock, peripherals::Peripherals};

/// 7-bit addresses of the devices on the shared I2C bus (GPIO8 / GPIO7).
pub mod i2c_addr {
    /// ES8311 mono audio codec (speaker DAC and microphone ADC).
    pub const ES8311: u8 = 0x18;
    /// GT911 capacitive touch controller on the 7" LCD, at its default
    /// address. The GT911 picks 0x5D or 0x14 at reset, depending on its INT
    /// pin level.
    pub const GT911: u8 = 0x5D;
    /// The other address the GT911 can come up at.
    pub const GT911_ALT: u8 = 0x14;
}

/// Initialises the HAL with the maximum CPU clock (400 MHz on chip
/// revision v3.x).
///
/// Logging and the heap are the application's choice:
///
/// ```ignore
/// let p = esp32_p4_function_ev::board::init();
/// esp32_p4_function_ev::board::init_logger(); // with the `log` feature
/// // Internal heap in the RAM the bootloader used:
/// esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: esp32_p4_function_ev::board::RECLAIMED_HEAP);
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

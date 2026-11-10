//! Board support for the ESP32-P4X-Function-EV-Board (ESP32-P4 chip
//! revision v3.1 or later): pin map, power switches and drivers for the
//! hardware on the board.
//!
//! Pin assignment follows Espressif's C BSP
//! (`esp-bsp/bsp/esp32_p4_function_ev_board`) and the board user guide:
//! <https://docs.espressif.com/projects/esp-dev-kits/en/latest/esp32p4/esp32-p4x-function-ev-board/user_guide.html>
//!
//! The hardware-independent modules ([`level`]) touch no peripheral and read
//! no clock, so they also build for a PC, where they are unit-tested:
//! `cargo test -p esp32-p4-function-ev --lib --target <host triple>`. The
//! drivers build only for the ESP32-P4.

#![no_std]

// Hardware-independent, tested on the host.
pub mod level;

// Drivers.
#[cfg(target_arch = "riscv32")]
pub mod audio;
#[cfg(target_arch = "riscv32")]
pub mod board;
#[cfg(target_arch = "riscv32")]
pub mod display;

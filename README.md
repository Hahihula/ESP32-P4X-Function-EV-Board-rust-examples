# ESP32-P4X-Function-EV-Board Rust examples

Bare-metal (`no_std`) Rust demos for the
[ESP32-P4X-Function-EV-Board](https://docs.espressif.com/projects/esp-dev-kits/en/latest/esp32p4/esp32-p4x-function-ev-board/user_guide.html)
(ESP32-P4 chip revision v3.1+, 32 MB PSRAM, 16 MB flash, ES8311 audio codec with
microphone and speaker amplifier, microSD, 100 Mbit Ethernet, 7" 1024×600 MIPI-DSI touch
LCD, ESP32-C6 for Wi-Fi and Bluetooth), built on [esp-hal](https://github.com/esp-rs/esp-hal)
and Embassy.

The pin map in `esp32-p4-function-ev/src/board.rs` follows Espressif's C BSP
(`esp-bsp/bsp/esp32_p4_function_ev_board`). The older ESP32-P4-Function-EV-Board (without
the "X") carries a v1.x chip, which esp-hal's ESP32-P4 support does not target. Demo 00
prints the chip revision.

## Requirements

- Stable Rust (installed with [`rustup`](https://rustup.rs)). `rust-toolchain.toml` adds
  the `riscv32imafc-unknown-none-elf` target.
- [`espflash`](https://github.com/esp-rs/espflash) 4.x.
- A USB-C data cable in the **USB Serial/JTAG** port. It is used for flashing and for the
  console.

## Running a demo

Every demo is its own binary in `examples/src/bin/`. From the repository root:

```bash
cargo run --release --bin 00_bringup
```

The cargo runner (`.cargo/config.toml`) flashes the board and then opens the serial
monitor. Press `Ctrl+R` to reset the board and `Ctrl+C` to quit. If flashing fails,
hold **BOOT**, press **RST**, then release **BOOT** to enter download mode by hand.

Log verbosity follows `ESP_LOG` (default `info`, set in `.cargo/esp-config.toml`), for
example `ESP_LOG=debug cargo run --release --bin 00_bringup`.

## Project layout

The repository is a Cargo workspace with two crates:

- **`esp32-p4-function-ev`**: the board library. Pin map, power switches and drivers for
  the hardware on the board. Use it in your own firmware (see below).
- **`esp32-p4-function-ev-examples`** (`examples/`): the demos, plus the app-level helpers
  that only the demos need.

In both, the hardware-independent modules touch no peripheral and read no clock. They
also build for a PC, where they are unit-tested; everything else builds only for the
ESP32-P4.

| Path | Contents |
|------|----------|
| `esp32-p4-function-ev/src/board.rs` | Board pin map, I2C device addresses, `init()`, logger |
| `examples/src/demo.rs` | Start-up macro shared by the demos |

## Using the library in your own firmware

```toml
[dependencies]
esp32-p4-function-ev = { git = "https://github.com/Hahihula/p4-rust-examples", features = ["log"] }
```

```rust
let p = esp32_p4_function_ev::board::init();
esp32_p4_function_ev::board::init_logger();
esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: esp32_p4_function_ev::board::RECLAIMED_HEAP);
```

Pin `esp-hal` with `~` to the same minor version as the library: the drivers use esp-hal's
`unstable` APIs, which may change in minor releases.

## Tests and CI

```bash
cargo test -p esp32-p4-function-ev -p esp32-p4-function-ev-examples --lib --target $(rustc -vV | sed -n 's/host: //p')
```

CI (`.github/workflows/ci.yml`) runs those tests, `cargo fmt --check`, clippy with
`-D warnings` (also on the library without the `log` feature) and a release build of every
demo.

## Demos

### 00 – Board bring-up

```bash
cargo run --release --bin 00_bringup
```

Prints the chip revision and the base MAC address from eFuse, then logs a heartbeat every
ten seconds.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

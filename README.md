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
| `esp32-p4-function-ev/src/board.rs` | Board pin map, I2C device addresses, `init()`, logger, console wait, PSRAM heap, shared I2C bus, BOOT button |
| `esp32-p4-function-ev/src/audio.rs` | I2S input stream from the ES8311 (DMA ring buffer that restarts itself after an overflow), ES8311 driver from `oxstack-drivers` |
| `esp32-p4-function-ev/src/level.rs` | RMS and peak levels in dBFS, text level bar (host-tested) |
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
esp32_p4_function_ev::board::psram_heap(p.PSRAM); // 32 MB more heap
let i2c = esp32_p4_function_ev::board::i2c(p.I2C0, p.GPIO8, p.GPIO7)?;
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

Checks that the board and the toolchain work:

- prints the chip revision and the base MAC address from eFuse;
- writes and reads back a pattern over the whole 32 MB PSRAM, then adds it to the heap
  and allocates 1 MiB from it;
- scans the shared I2C bus (SCL GPIO8, SDA GPIO7) and reads the ID of the ES8311 codec
  and, with the LCD connected, of the GT911 touch controller;
- logs presses of the BOOT button with their length, and a heartbeat every ten seconds.

Every demo waits two seconds after start-up before it logs anything: the USB-Serial-JTAG
port drops off USB on every chip reset, and the monitor needs that long to reconnect.

Output on a board with the LCD connected (shortened):

```text
INFO - Chip revision: v3.2
INFO - PSRAM: 32768 KiB mapped at 0x48000000
INFO - PSRAM test: 0 bad words, write 333 ms (~96 MiB/s), read 403 ms (~79 MiB/s)
INFO - Heap: 1 MiB allocation at 0x48000000 OK
INFO -   0x18: ES8311 audio codec
INFO -   0x5D: GT911 touch controller
INFO -   ES8311: chip ID 83 11, version 01 (OK)
INFO -   GT911 at 0x5D: product ID "911", firmware 1060, 1024x600 (OK)
INFO - Bring-up done. Press BOOT; a heartbeat line follows every 10 s.
```

The ES8311 doesn't auto-increment its register address, so multi-byte reads return
`0xFF` after the first byte; read its registers one at a time.

### 01 – Microphone level meter

```bash
cargo run --release --bin 01_microphone
```

Records the on-board microphone through the ES8311 codec over I2S at 32 kHz and prints
its level ten times per second: RMS and peak in dBFS and a bar with one `#` per 3 dB
above −90 dBFS. Speak, clap or whistle near the board.

The microphone pre-amplifier starts at its maximum gain, 30 dB, as in Espressif's C
driver. Each press of **BOOT** lowers it by 6 dB; after 0 dB it goes back to 30 dB.
The ES8311 adds a further 24 dB digitally.

The P4 is the I2S master. The codec runs as slave and derives its clocks from the bit
clock, so it is initialised only after the I2S stream is running. For about half a
second after that, while its ADC starts, the codec sends silence (−96 dBFS).

In a quiet room at 30 dB the meter shows about −60 dBFS; at 0 dB the noise floor drops
by those 30 dB, to about −90 dBFS:

```text
INFO - Recording at 32000 Hz, mic gain 30 dB. Speak, clap or whistle; press BOOT for less gain.
INFO -  -63.3 dBFS  peak  -53.5  |########......................|
INFO -  -57.9 dBFS  peak  -48.8  |##########....................|
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

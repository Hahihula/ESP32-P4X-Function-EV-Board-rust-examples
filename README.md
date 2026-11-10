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
| `esp32-p4-function-ev/src/display.rs` | 7" LCD: EK79007 panel init over MIPI-DSI, triple-buffered video from PSRAM, `embedded-graphics` drawing target, backlight |
| `esp32-p4-function-ev/src/ethernet.rs` | 100 Mbit Ethernet: IP101 PHY reset, EMAC over RMII, `embassy-net` driver, fix for long frames on transmit |
| `esp32-p4-function-ev/src/ntp.rs` | SNTP request and reply, Unix time to UTC date and time (host-tested) |
| `esp32-p4-function-ev/src/sdcard.rs` | microSD card: SD host in 4-bit mode at 40 MHz, card driver from `sdio`, `embedded-sdmmc` block device for FAT16/FAT32 |
| `esp32-p4-function-ev/src/touch.rs` | GT911 touch controller: polled async driver over the shared I2C bus, address detection, up to five fingers, orientation (host-tested) |
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

### 02 – Display

```bash
cargo run --release --bin 02_display
```

Needs the LCD adapter board: the ribbon cable in the MIPI-DSI connector, jumper wires
from header J1 GPIO27 to RST_LCD and GPIO26 to PWM, and power for the adapter (its USB
port, or 5 V and GND from J1).

Shows a test pattern for five seconds (eight colour bars, a 32-step grey ramp, a white
border and a caption), then bounces a ball over it. Each frame redraws only the ball and
the pattern where it was, not the whole 1.2 MB frame, and logs the frame rate:

```text
INFO - LCD on: 1024x600, RGB565
INFO - Test pattern drawn in 35 ms
INFO - 56.1 fps, 3902 us to draw a frame
```

esp-hal's MIPI-DSI driver streams a frame buffer from PSRAM to the panel by DMA, without
the CPU. Two things in it shape `display.rs`:

- **Exact timing.** The DSI host counts a line in lane byte clocks, the DPI bridge in
  pixel clocks. If the two line lengths differ even by a fraction of a pixel after
  rounding, the error adds up every line, the bridge FIFO overflows partway down the
  screen, and the picture slants from there. The timing is therefore Espressif's with a
  48 MHz pixel clock (240 MHz / 5, instead of 52) and a 150-pixel front porch (instead of
  160): a 1344-pixel line is exactly 3150 byte clocks at 900 Mbit/s. The panel refreshes
  at about 56 Hz.
- **Three frame buffers.** A committed buffer reaches the DMA only at the end of the
  next frame, so it appears two frames after `commit()`. With two buffers, the back
  buffer would still be on screen while you draw into it.

The backlight is on or off only: esp-hal has no PWM driver (LEDC or MCPWM) for the
ESP32-P4 yet.

### 03 – Touch

```bash
cargo run --release --bin 03_touch
```

Finger painting on the LCD (connected as for demo 02): every finger, up to five at once,
draws a line in its own colour. **BOOT** clears the canvas. Fingers touching down and
lifting are logged:

```text
INFO - GT911 at 0x5D: product ID 911, firmware 1060, 1024x600
INFO - Draw with up to 5 fingers; press BOOT to clear
INFO - Finger 0 down at (370, 228)
INFO - Finger 0 up at (447, 123)
```

The GT911's INT and RESET pins are not wired to the ESP32-P4, so the driver polls it, here
once per frame. Without the reset sequence the GT911 keeps whichever address it chose at
power-up, 0x5D or 0x14; `Gt911::find` tries both.

The touch panel is mounted rotated by 180° relative to the display: the GT911 reports
(0, 0) at the bottom right corner of the picture. `touch::LCD_ORIENTATION` flips both axes:

```rust
let (touch, info) = Gt911::find(i2c).await?;
let mut touch = touch.with_orientation(touch::LCD_ORIENTATION);
if let Some(touches) = touch.read().await? {
    for t in touches.iter() { /* t.id, t.x, t.y */ }
}
```

Each frame is drawn into the oldest of the three frame buffers, so the demo keeps the
strokes of the last three frames and paints all of them into every back buffer. That way
each buffer gets every stroke it missed.

### 04 – microSD card

```bash
cargo run --release --bin 04_sdcard
```

Needs a microSD card formatted FAT16 or FAT32 (cards up to 32 GB usually come that way;
larger ones come as exFAT, which `embedded-sdmmc` can't read). The demo writes to the
card, so use one without anything important on it. It:

- identifies the card and switches it to the 4-bit bus at 40 MHz;
- lists the root directory;
- appends a line to `P4BOOT.TXT`, so the boot count goes up with every run;
- writes a 1 MiB file, `P4BENCH.BIN`, reads it back, checks it and deletes it.

```text
INFO - Card: manufacturer 0x03, OEM "SD", product "SU32G" rev 8.0, serial 00E0214D, made 12/2013
INFO - Capacity 29.7 GiB, SD spec V3, 4-bit bus at 40 MHz
INFO - Root directory:
INFO -            0  EMPTY
INFO -        <DIR>  SPOTLI~1
INFO - Appended to P4BOOT.TXT: Boot 2, 2175 ms after reset
INFO - 1 MiB file: write 20469 ms (50 KiB/s), read 841 ms (1217 KiB/s)
INFO - Read back 1048576 bytes, all match
```

The card and the slot's pins (GPIO39–44) are powered by the chip's internal LDO channel 4,
which esp-hal switches on with the SD host. The slot has no card-detect switch, so a
missing card shows up as a timeout on the first command.

esp-hal's SD host plugs into embassy's `sdio` crate, which brings the card up and exposes
it as an async block device. The file system is `embedded-sdmmc` (re-exported as
`sdcard::embedded_sdmmc`), the established FAT crate, which is blocking: `SdCard` runs
each block transfer to completion in place, briefly blocking the executor. The async
alternative the `sdio` docs suggest, `embedded-fatfs`, has no release on crates.io yet.

Writing is slow because `embedded-sdmmc` writes one 512-byte block at a time, and the card
finishes programming each block before it takes the next. Reading reaches about 1.2 MB/s.

```rust
let host = sdcard::host(p.SDHOST)?;
let card = SdCard::init(&host, esp32_p4_function_ev::sd_pins!(p)).await?;
let fs = embedded_sdmmc::VolumeManager::new(card, my_time_source);
let root = fs.open_volume(embedded_sdmmc::VolumeIdx(0))?.open_root_dir()?;
```

### 05 – Ethernet

```bash
cargo run --release --bin 05_ethernet
```

Needs a network cable to a network with a DHCP server (a home router). The demo brings
up the port and `embassy-net`, gets an address by DHCP, sets its clock from
`pool.ntp.org` and serves a status page on port 80. The board also answers ping.

```text
INFO - MAC address e8:f6:0a:e7:54:a2
INFO - Waiting for the link (is the cable plugged in?)
INFO - Link up, asking for an address by DHCP
INFO - IP address 192.168.32.71/23, gateway Some(192.168.32.3), DNS [192.168.32.3]
INFO - Time from pool.ntp.org: 2026-11-10 15:06:07 UTC
INFO - Open http://192.168.32.71/ in a browser, or ping it
INFO - "GET / HTTP/1.1" from Some(Endpoint { addr: Ipv4(192.168.33.161), port: 49203 }): 200 OK
```

On the test network DHCP took about 30 s after the link came up every time, which matches
a switch port with spanning tree holding traffic back after a link change.

The IP101 PHY takes its 50 MHz RMII clock from an oscillator on the board, which also
feeds the ESP32-P4 on GPIO50. `EthernetPort::new` resets the PHY (GPIO51), starts the
EMAC with the chip's factory MAC address and returns an `embassy-net` driver:

```rust
static DMA_STORAGE: StaticCell<ethernet::DmaStorage> = StaticCell::new();
let port = EthernetPort::new(p.ETH, esp32_p4_function_ev::eth_pins!(p), DMA_STORAGE.init(ethernet::DmaStorage::new())).await?;
let (stack, runner) = embassy_net::new(port, embassy_net::Config::dhcpv4(Default::default()), resources, seed);
```

GPIO35 is both the EMAC's TXD1 and the BOOT button: don't press BOOT while Ethernet runs.

**Frames longer than 264 bytes.** With esp-hal 1.2 as it is, the board sends small
frames fine but loses every frame longer than 264 bytes: ping works, DHCP and DNS work,
but a web page never arrives. esp-hal runs the EMAC's transmit side in store-and-forward
mode, which waits for the whole frame to be in the transmit FIFO, and on the ESP32-P4 that
FIFO holds only about 256 bytes. `EthernetPort` switches transmit to threshold mode (start
at 64 bytes), as esp-hal already does for receive. In that mode the EMAC can't insert IP,
TCP and UDP checksums, so `EthernetPort` has `embassy-net` compute them on transmit.

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT) at your option.

//! The 7" 1024×600 LCD: EK79007 panel driver on the MIPI-DSI bus, frame
//! buffers in PSRAM and an `embedded-graphics` drawing target.
//!
//! The LCD adapter board connects to the MIPI-DSI connector with the ribbon
//! cable, plus two jumper wires from header J1: GPIO27 to RST_LCD (panel
//! reset, active low) and GPIO26 to PWM (backlight, active high). Power it
//! through its own USB port or from the 5 V and GND pins of J1.
//!
//! esp-hal's MIPI-DSI driver powers the D-PHY from the chip's internal
//! `LDO_VO3` (2.5 V), sends the panel's init commands in command mode and
//! then streams a frame buffer to the panel in video mode, without the CPU.
//! [`Display`] wraps it with the EK79007 init sequence and timing from
//! Espressif's `esp_lcd_ek79007` component (Apache-2.0).
//!
//! The pixel format is RGB565, little endian in memory.

use core::alloc::Layout;

use embassy_time::{Duration, Timer};
use embedded_graphics::{
    Pixel,
    draw_target::DrawTarget,
    geometry::{OriginDimensions, Size},
    pixelcolor::{Rgb565, raw::RawU16},
    prelude::{Dimensions, RawData},
    primitives::Rectangle,
};
use esp_hal::{
    gpio::{Level, Output, OutputConfig},
    mipi_dsi::{
        self, DataLanes, MipiDsi,
        dpi::{ColorFormat, DpiClockSource, DpiConfig, DsiDpi, FrameTiming},
    },
    peripherals::{GPIO26, GPIO27, MIPI_DSI, VDMA_CH0},
};

/// Width of the LCD in pixels.
pub const WIDTH: u32 = 1024;
/// Height of the LCD in pixels.
pub const HEIGHT: u32 = 600;
/// Bytes per pixel (RGB565).
const BYTES_PER_PIXEL: usize = 2;
/// Size of one frame buffer in bytes (1.2 MB).
pub const FRAME_BYTES: usize = WIDTH as usize * HEIGHT as usize * BYTES_PER_PIXEL;
const ROW_BYTES: usize = WIDTH as usize * BYTES_PER_PIXEL;
/// Number of frame buffers [`Display`] needs.
///
/// Three, not two: esp-hal hands a committed buffer to the DMA only at the
/// end of the next frame, so it appears two frames after
/// [`Display::commit`]. With two buffers the back buffer would still be on
/// screen while you draw into it.
pub const FRAME_BUFFERS: usize = 3;

/// Two DSI data lanes at 900 Mbit/s each, as in Espressif's driver.
const LANES: DataLanes = DataLanes::_2;
const LANE_BIT_RATE_MBPS: f32 = 900.0;

/// Pixel clock and frame timing, about 56 Hz.
///
/// Based on Espressif's driver (52 MHz, front porch 160), adjusted so that
/// no clock conversion needs rounding. The DSI host counts a line in lane
/// byte clocks (900 / 8 = 112.5 MHz) and the DPI bridge in pixel clocks;
/// if the two line lengths differ even by a fraction of a pixel, the error
/// adds up line by line until the bridge FIFO overflows, and the picture
/// slants from there down. 240 MHz / 5 = 48 MHz exactly, and a 1344-pixel
/// line is exactly 3150 byte clocks.
const DPI_CONFIG: DpiConfig = DpiConfig {
    virtual_channel: 0,
    pixel_clock_mhz: 48.0,
    dpi_clk_src: DpiClockSource::PllF240m,
    in_color_format: ColorFormat::Rgb565,
    out_color_format: ColorFormat::Rgb565,
    timing: FrameTiming {
        h_active: WIDTH,
        hsw: 10,
        hbp: 160,
        hfp: 150,
        v_active: HEIGHT,
        vsw: 1,
        vbp: 23,
        vfp: 12,
    },
};

/// EK79007 commands: pad control (selects the number of DSI lanes), the
/// vendor's analog settings, then sleep out.
mod ek79007 {
    pub const PAD_CONTROL: u8 = 0xB2;
    pub const PAD_2_LANES: u8 = 0x10;
    pub const SLEEP_OUT: u8 = 0x11;
    /// Wait after sleep out before streaming video.
    pub const SLEEP_OUT_DELAY_MS: u64 = 120;
    /// Vendor init registers and values, from Espressif's `esp_lcd_ek79007`.
    pub const VENDOR_INIT: [(u8, u8); 7] = [
        (0x80, 0x8B),
        (0x81, 0x78),
        (0x82, 0x84),
        (0x83, 0x88),
        (0x84, 0xA8),
        (0x85, 0xE3),
        (0x86, 0x88),
    ];
}

/// The control pins wired from header J1 to the LCD adapter board.
#[allow(missing_docs, reason = "the field names are the signal names")]
pub struct LcdPins<'d> {
    /// RST_LCD, active low.
    pub reset: GPIO27<'d>,
    /// PWM, the backlight enable, active high.
    pub backlight: GPIO26<'d>,
}

/// The [`LcdPins`] taken out of `esp_hal::peripherals::Peripherals`.
#[macro_export]
macro_rules! lcd_pins {
    ($p:expr) => {
        $crate::display::LcdPins {
            reset: $p.GPIO27,
            backlight: $p.GPIO26,
        }
    };
}

/// Errors from bringing up the LCD.
#[derive(Debug)]
pub enum Error {
    /// The DSI bus or video configuration was rejected.
    Config(mipi_dsi::ConfigError),
    /// Sending an init command failed.
    Command(mipi_dsi::dbi::Error),
}

impl From<mipi_dsi::ConfigError> for Error {
    fn from(e: mipi_dsi::ConfigError) -> Self {
        Self::Config(e)
    }
}

impl From<mipi_dsi::dbi::Error> for Error {
    fn from(e: mipi_dsi::dbi::Error) -> Self {
        Self::Command(e)
    }
}

/// Allocates a black frame buffer of [`FRAME_BYTES`] in PSRAM, 64-byte
/// aligned as the DSI DMA needs. The PSRAM must have been added to the heap
/// (`board::psram_heap`). The buffer is never freed.
///
/// # Panics
///
/// If there isn't enough free PSRAM.
#[must_use]
pub fn alloc_framebuffer() -> &'static mut [u8] {
    let layout = Layout::from_size_align(FRAME_BYTES, 64).expect("valid layout");
    // SAFETY: the layout has a non-zero size.
    let ptr =
        unsafe { esp_alloc::HEAP.alloc_caps(esp_alloc::MemoryCapability::External.into(), layout) };
    assert!(!ptr.is_null(), "not enough PSRAM for a frame buffer");
    // SAFETY: `ptr` points to FRAME_BYTES freshly allocated bytes that nothing
    // else references; zeroing them makes every byte initialised.
    unsafe {
        ptr.write_bytes(0, FRAME_BYTES);
        core::slice::from_raw_parts_mut(ptr, FRAME_BYTES)
    }
}

/// The running LCD: the panel is initialised and the DSI host streams the
/// front frame buffer to it continuously.
///
/// Draw into the back buffer ([`back_buffer`](Self::back_buffer)), then show
/// it with [`commit`](Self::commit). The buffers rotate, so the back buffer
/// holds the frame committed [`FRAME_BUFFERS`] commits ago: redraw
/// everything that changed since then, or the whole frame.
pub struct Display<'d> {
    dpi: DsiDpi<'d>,
    backlight: Output<'d>,
    /// Held high (reset released) for as long as the display runs.
    _reset: Output<'d>,
}

impl<'d> Display<'d> {
    /// Resets and initialises the panel, starts video mode with the frame
    /// buffers (each [`FRAME_BYTES`] long, see [`alloc_framebuffer`]; the
    /// first one is shown) and switches the backlight on.
    ///
    /// # Errors
    ///
    /// If the DSI configuration is rejected or a command can't be sent.
    pub async fn new(
        dsi: MIPI_DSI<'d>,
        vdma: VDMA_CH0<'d>,
        pins: LcdPins<'d>,
        framebuffers: [&'d mut [u8]; FRAME_BUFFERS],
    ) -> Result<Self, Error> {
        let mut backlight = Output::new(pins.backlight, Level::Low, OutputConfig::default());
        let mut reset = Output::new(pins.reset, Level::Low, OutputConfig::default());

        let mut bus = MipiDsi::new(
            dsi,
            vdma,
            mipi_dsi::Config::default()
                .with_num_data_lanes(LANES)
                .with_lane_bit_rate_mbps(LANE_BIT_RATE_MBPS),
        )?;

        // Hardware reset: hold low for 10 ms, then give the panel 20 ms.
        Timer::after(Duration::from_millis(10)).await;
        reset.set_high();
        Timer::after(Duration::from_millis(20)).await;

        let mut dbi = bus.dbi(DPI_CONFIG.virtual_channel);
        dbi.write_cmd(ek79007::PAD_CONTROL, &[ek79007::PAD_2_LANES])?;
        for (register, value) in ek79007::VENDOR_INIT {
            dbi.write_cmd(register, &[value])?;
        }
        dbi.write_cmd(ek79007::SLEEP_OUT, &[])?;
        Timer::after(Duration::from_millis(ek79007::SLEEP_OUT_DELAY_MS)).await;

        let dpi = bus.dpi(DPI_CONFIG, &framebuffers)?;
        backlight.set_high();
        Ok(Self {
            dpi,
            backlight,
            _reset: reset,
        })
    }

    /// The frame buffer that is not on screen, to draw into.
    pub fn back_buffer(&mut self) -> FrameBuffer<'_> {
        FrameBuffer(self.dpi.framebuffer_mut())
    }

    /// Queues the back buffer for display (it appears two frames later) and
    /// moves on to the next buffer.
    pub fn commit(&mut self) {
        self.dpi.commit();
    }

    /// Waits for the start of the next vertical blank. Commit right after
    /// it, so the switch happens before the next frame starts.
    pub async fn wait_for_vsync(&mut self) {
        self.dpi.wait_for_vsync_async().await;
    }

    /// Switches the backlight on or off.
    pub fn set_backlight(&mut self, on: bool) {
        self.backlight.set_level(Level::from(on));
    }
}

/// A full-screen RGB565 frame buffer. Implements embedded-graphics'
/// [`DrawTarget`].
pub struct FrameBuffer<'a>(&'a mut [u8]);

impl FrameBuffer<'_> {
    /// Raw frame bytes (RGB565, little endian, row by row).
    pub fn as_bytes_mut(&mut self) -> &mut [u8] {
        self.0
    }
}

impl OriginDimensions for FrameBuffer<'_> {
    fn size(&self) -> Size {
        Size::new(WIDTH, HEIGHT)
    }
}

/// The two bytes of a pixel in memory.
fn pixel_bytes(color: Rgb565) -> [u8; 2] {
    RawU16::from(color).into_inner().to_le_bytes()
}

impl DrawTarget for FrameBuffer<'_> {
    type Color = Rgb565;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, color) in pixels {
            if p.x < 0 || p.y < 0 || p.x >= WIDTH as i32 || p.y >= HEIGHT as i32 {
                continue;
            }
            let i = p.y as usize * ROW_BYTES + p.x as usize * BYTES_PER_PIXEL;
            self.0[i..i + BYTES_PER_PIXEL].copy_from_slice(&pixel_bytes(color));
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let area = area.intersection(&self.bounding_box());
        let Some(bottom_right) = area.bottom_right() else {
            return Ok(());
        };
        let bytes = pixel_bytes(color);
        let (x0, x1) = (area.top_left.x as usize, bottom_right.x as usize);
        for y in area.top_left.y as usize..=bottom_right.y as usize {
            let row =
                &mut self.0[y * ROW_BYTES..][x0 * BYTES_PER_PIXEL..(x1 + 1) * BYTES_PER_PIXEL];
            for px in row.as_chunks_mut::<BYTES_PER_PIXEL>().0 {
                *px = bytes;
            }
        }
        Ok(())
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        let bytes = pixel_bytes(color);
        for px in self.0.as_chunks_mut::<BYTES_PER_PIXEL>().0 {
            *px = bytes;
        }
        Ok(())
    }
}

//! Demo 03: touch screen finger painting.
//!
//! Finds the GT911 touch controller, then polls it once per frame and paints
//! a line behind every finger (up to five), each finger in its own colour.
//! BOOT clears the canvas. Finger down and up events are logged with their
//! position.

#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use core::convert::Infallible;

use embassy_executor::Spawner;
use embedded_graphics::{
    mono_font::{MonoTextStyle, ascii::FONT_10X20},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, Line, PrimitiveStyle},
    text::{Baseline, Text},
};
use esp_backtrace as _;
use esp32_p4_function_ev::{
    board,
    display::{self, Display, FRAME_BUFFERS},
    touch::{Gt911, LCD_ORIENTATION, MAX_POINTS, Touches},
};
use log::{info, warn};

esp_bootloader_esp_idf::esp_app_desc!();

/// Line width in pixels.
const BRUSH: u32 = 10;
/// One colour per finger, by track ID.
const COLORS: [Rgb565; MAX_POINTS] = [
    Rgb565::RED,
    Rgb565::GREEN,
    Rgb565::CYAN,
    Rgb565::YELLOW,
    Rgb565::MAGENTA,
];
const BACKGROUND: Rgb565 = Rgb565::BLACK;

/// The stroke a finger made during one frame: from where it was in the
/// previous frame (or the same point when it just touched down) to where
/// it is now.
#[derive(Clone, Copy)]
struct Stroke {
    from: Point,
    to: Point,
    color: Rgb565,
}

type FrameStrokes = [Option<Stroke>; MAX_POINTS];

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    esp32_p4_function_ev_examples::start!(p, "Demo 03: touch");

    board::psram_heap(p.PSRAM);
    let framebuffers = core::array::from_fn(|_| display::alloc_framebuffer());
    let mut lcd = Display::new(
        p.MIPI_DSI,
        p.VDMA_CH0,
        esp32_p4_function_ev::lcd_pins!(p),
        framebuffers,
    )
    .await
    .unwrap();

    let i2c = board::i2c(p.I2C0, p.GPIO8, p.GPIO7).unwrap();
    let (touch, touch_info) = Gt911::find(i2c).await.unwrap();
    let mut touch = touch.with_orientation(LCD_ORIENTATION);
    info!(
        "GT911 at 0x{:02X}: product ID {}, firmware {:04X}, {}x{}",
        touch_info.address,
        touch_info.product_id_str(),
        touch_info.firmware,
        touch_info.width,
        touch_info.height
    );
    info!("Draw with up to {MAX_POINTS} fingers; press BOOT to clear");

    let boot = board::boot_button(p.GPIO35);
    let mut boot_was_pressed = false;

    // Painting adds to what is on screen, but each frame goes into the
    // oldest of the frame buffers. So keep the strokes of the last
    // FRAME_BUFFERS frames and paint all of them into every back buffer: then
    // each buffer gets every stroke it missed.
    let mut recent: [FrameStrokes; FRAME_BUFFERS] = [[None; MAX_POINTS]; FRAME_BUFFERS];
    // How many more back buffers to clear; all of them at the start.
    let mut to_clear = FRAME_BUFFERS;
    // Where each finger (by track ID) was in the last scan.
    let mut last: [Option<Point>; MAX_POINTS] = [None; MAX_POINTS];

    loop {
        let boot_pressed = boot.is_low();
        if boot_pressed && !boot_was_pressed {
            info!("Clearing the canvas");
            to_clear = FRAME_BUFFERS;
            recent = [[None; MAX_POINTS]; FRAME_BUFFERS];
        }
        boot_was_pressed = boot_pressed;

        let mut strokes: FrameStrokes = [None; MAX_POINTS];
        match touch.read().await {
            Ok(Some(touches)) => {
                strokes = track(&touches, &mut last);
            }
            Ok(None) => {}
            Err(e) => warn!("Touch read failed: {e:?}"),
        }
        recent.rotate_left(1);
        recent[FRAME_BUFFERS - 1] = strokes;

        let mut fb = lcd.back_buffer();
        if to_clear > 0 {
            to_clear -= 1;
            draw_blank_canvas(&mut fb);
        }
        for stroke in recent.iter().flatten().flatten() {
            paint(&mut fb, stroke);
        }

        lcd.wait_for_vsync().await;
        lcd.commit();
    }
}

/// Matches this scan's fingers to the last scan's by track ID, logs fingers
/// that touched down or lifted, and returns the strokes they made.
#[allow(
    clippy::large_stack_frames,
    reason = "the two log calls inline their formatting machinery; called from main only"
)]
fn track(touches: &Touches, last: &mut [Option<Point>; MAX_POINTS]) -> FrameStrokes {
    let mut strokes = [None; MAX_POINTS];
    let mut now: [Option<Point>; MAX_POINTS] = [None; MAX_POINTS];
    for t in touches.iter() {
        let slot = usize::from(t.id) % MAX_POINTS;
        let to = Point::new(i32::from(t.x), i32::from(t.y));
        let from = last[slot].unwrap_or_else(|| {
            info!("Finger {} down at ({}, {})", t.id, t.x, t.y);
            to
        });
        now[slot] = Some(to);
        strokes[slot] = Some(Stroke {
            from,
            to,
            color: COLORS[slot],
        });
    }
    for (slot, (before, after)) in last.iter().zip(&now).enumerate() {
        if let (Some(p), None) = (before, after) {
            info!("Finger {slot} up at ({}, {})", p.x, p.y);
        }
    }
    *last = now;
    strokes
}

fn draw_blank_canvas<D: DrawTarget<Color = Rgb565, Error = Infallible>>(fb: &mut D) {
    fb.clear(BACKGROUND).unwrap();
    Text::with_baseline(
        "Draw with up to five fingers. BOOT clears.",
        Point::new(12, 10),
        MonoTextStyle::new(&FONT_10X20, Rgb565::new(16, 32, 16)),
        Baseline::Top,
    )
    .draw(fb)
    .unwrap();
}

/// A line with round ends: a thick line plus a dot at each end.
fn paint<D: DrawTarget<Color = Rgb565, Error = Infallible>>(fb: &mut D, stroke: &Stroke) {
    let style = PrimitiveStyle::with_fill(stroke.color);
    for end in [stroke.from, stroke.to] {
        Circle::with_center(end, BRUSH)
            .into_styled(style)
            .draw(fb)
            .unwrap();
    }
    if stroke.from != stroke.to {
        Line::new(stroke.from, stroke.to)
            .into_styled(PrimitiveStyle::with_stroke(stroke.color, BRUSH))
            .draw(fb)
            .unwrap();
    }
}

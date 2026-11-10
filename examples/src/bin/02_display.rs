//! Demo 02: the 7" MIPI-DSI LCD.
//!
//! Brings up the EK79007 panel, shows a test pattern (colour bars, a grey
//! ramp, a one-pixel border and some text) for five seconds, then bounces
//! a ball across it. Each frame redraws only the ball and the pattern where
//! the ball was, into the back buffer, and shows it at the next vertical
//! blank. Logs the frame rate and the time to draw a frame every two seconds.

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
use embassy_time::{Duration, Instant, Timer};
use embedded_graphics::{
    mono_font::{MonoTextStyle, ascii::FONT_10X20},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, PrimitiveStyle, Rectangle},
    text::{Alignment, Text},
};
use esp_backtrace as _;
use esp32_p4_function_ev::{
    board,
    display::{self, Display, FRAME_BUFFERS, HEIGHT, WIDTH},
};
use log::info;

esp_bootloader_esp_idf::esp_app_desc!();

/// Height of the colour bars; the grey ramp fills the rest.
const BARS_HEIGHT: u32 = HEIGHT * 3 / 4;
const BALL_DIAMETER: u32 = 80;
const PATTERN_HOLD: Duration = Duration::from_secs(5);

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(_spawner: Spawner) -> ! {
    esp32_p4_function_ev_examples::start!(p, "Demo 02: display");

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
    info!("LCD on: {WIDTH}x{HEIGHT}, RGB565");

    // Put the pattern into every frame buffer, so the animation only has to
    // redraw what changes.
    let started = Instant::now();
    draw_test_pattern(&mut lcd.back_buffer());
    info!("Test pattern drawn in {} ms", started.elapsed().as_millis());
    for _ in 1..FRAME_BUFFERS {
        lcd.wait_for_vsync().await;
        lcd.commit();
        draw_test_pattern(&mut lcd.back_buffer());
    }
    info!("Still test pattern for {} s", PATTERN_HOLD.as_secs());
    Timer::after(PATTERN_HOLD).await;

    info!("Bouncing ball; logging the frame rate every 2 s");
    let size = Size::new_equal(BALL_DIAMETER);
    let max = Point::new(
        (WIDTH - BALL_DIAMETER) as i32,
        (HEIGHT - BALL_DIAMETER) as i32,
    );
    let mut ball = Point::new(100, 100);
    let mut velocity = Point::new(7, 5);
    // Where the ball is in each frame buffer, oldest first. The back buffer
    // is the oldest, so `drawn[0]` is the area to repair in it.
    let mut drawn: [Option<Rectangle>; FRAME_BUFFERS] = [None; FRAME_BUFFERS];
    let (mut frames, mut draw_us) = (0u32, 0u64);
    let mut window = Instant::now();
    loop {
        ball += velocity;
        if ball.x <= 0 || ball.x >= max.x {
            velocity.x = -velocity.x;
        }
        if ball.y <= 0 || ball.y >= max.y {
            velocity.y = -velocity.y;
        }
        ball = ball.component_max(Point::zero()).component_min(max);

        let started = Instant::now();
        let mut fb = lcd.back_buffer();
        if let Some(old) = drawn[0] {
            draw_test_pattern(&mut fb.clipped(&old));
        }
        Circle::new(ball, BALL_DIAMETER)
            .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
            .draw(&mut fb)
            .unwrap();
        drawn.rotate_left(1);
        drawn[FRAME_BUFFERS - 1] = Some(Rectangle::new(ball, size));
        draw_us += started.elapsed().as_micros();

        lcd.wait_for_vsync().await;
        lcd.commit();

        frames += 1;
        if window.elapsed() >= Duration::from_secs(2) {
            let ms = window.elapsed().as_millis();
            info!(
                "{}.{} fps, {} us to draw a frame",
                frames * 1000 / ms as u32,
                frames * 10_000 / ms as u32 % 10,
                draw_us / u64::from(frames)
            );
            (frames, draw_us) = (0, 0);
            window = Instant::now();
        }
    }
}

/// Eight colour bars over the top three quarters, a 32-step grey ramp
/// below them, a white border and a caption.
fn draw_test_pattern<D: DrawTarget<Color = Rgb565, Error = Infallible>>(fb: &mut D) {
    const BARS: [Rgb565; 8] = [
        Rgb565::WHITE,
        Rgb565::YELLOW,
        Rgb565::CYAN,
        Rgb565::GREEN,
        Rgb565::MAGENTA,
        Rgb565::RED,
        Rgb565::BLUE,
        Rgb565::BLACK,
    ];
    let bar_width = WIDTH / BARS.len() as u32;
    for (i, color) in BARS.into_iter().enumerate() {
        fill(
            fb,
            (i as u32 * bar_width, 0),
            (bar_width, BARS_HEIGHT),
            color,
        );
    }

    const STEPS: u32 = 32;
    let step_width = WIDTH / STEPS;
    for step in 0..STEPS {
        // Rgb565 has 5 bits of red and blue and 6 of green.
        let level = step as u8;
        let grey = Rgb565::new(level, level * 2, level);
        let height = HEIGHT - BARS_HEIGHT;
        fill(
            fb,
            (step * step_width, BARS_HEIGHT),
            (step_width, height),
            grey,
        );
    }

    Rectangle::new(Point::zero(), Size::new(WIDTH, HEIGHT))
        .into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 1))
        .draw(fb)
        .unwrap();

    let caption = "ESP32-P4X-Function-EV-Board\n1024x600 MIPI-DSI, Rust + esp-hal";
    let center = Point::new(WIDTH as i32 / 2, BARS_HEIGHT as i32 / 2);
    fill(
        fb,
        (WIDTH / 2 - 190, BARS_HEIGHT / 2 - 30),
        (380, 56),
        Rgb565::BLACK,
    );
    Text::with_alignment(
        caption,
        center,
        MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE),
        Alignment::Center,
    )
    .draw(fb)
    .unwrap();
}

fn fill<D: DrawTarget<Color = Rgb565, Error = Infallible>>(
    fb: &mut D,
    (x, y): (u32, u32),
    (w, h): (u32, u32),
    color: Rgb565,
) {
    fb.fill_solid(
        &Rectangle::new(Point::new(x as i32, y as i32), Size::new(w, h)),
        color,
    )
    .unwrap();
}

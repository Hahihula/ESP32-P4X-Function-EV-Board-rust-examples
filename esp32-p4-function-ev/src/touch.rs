//! The GT911 capacitive touch controller on the 7" LCD, with the board's
//! specifics on top of the [`gt911`] crate.
//!
//! On this board the GT911's INT and RESET pins are not connected to the
//! ESP32-P4, so there is no touch interrupt: poll it, every frame or every
//! 10 ms or so (the GT911 scans at about 100 Hz). Without the reset sequence
//! it keeps the address it picked at power-up, 0x5D or 0x14: [`find`] tries
//! both.
//!
//! The touch panel is mounted upside down relative to the display: the GT911
//! reports (0, 0) at the bottom right corner of the picture. [`lcd_point`]
//! turns its coordinates into display coordinates.
//!
//! The coordinate mapping touches no hardware, so it builds and is
//! unit-tested on a PC.

pub use gt911;
use gt911::{Error, Gt911};

/// The two I2C addresses the GT911 can come up at, default first.
pub const ADDRESSES: [u8; 2] = [0x5D, 0x14];
/// The LCD size in pixels (see `display::WIDTH` and `display::HEIGHT`).
const WIDTH: u16 = 1024;
const HEIGHT: u16 = 600;
/// The most touch points the GT911 reports at once.
pub const MAX_POINTS: usize = 5;

/// A GT911 point in display coordinates: x from the left edge, y from the
/// top edge, both in pixels. The touch panel is rotated by 180° relative to
/// the display.
#[must_use]
pub fn lcd_point(x: u16, y: u16) -> (u16, u16) {
    (
        WIDTH.saturating_sub(1).saturating_sub(x),
        HEIGHT.saturating_sub(1).saturating_sub(y),
    )
}

/// Looks for the GT911 at both of its addresses and initialises the one that
/// answers with product ID "911". Returns the driver and its address.
///
/// # Errors
///
/// The error of the last address tried: [`Error::UnexpectedProductId`] or an
/// I2C error if no GT911 answers (is the LCD connected and powered?).
pub async fn find<I2C, E>(i2c: &mut I2C) -> Result<(Gt911<I2C>, u8), Error<E>>
where
    I2C: embedded_hal_async::i2c::I2c<Error = E>,
{
    let mut buf = [0u8; gt911::GET_TOUCH_BUF_SIZE];
    let mut last = Error::UnexpectedProductId;
    for address in ADDRESSES {
        let touch = Gt911::new(address);
        match touch.init(i2c, &mut buf).await {
            Ok(()) => return Ok((touch, address)),
            Err(e) => last = e,
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotated_180() {
        assert_eq!(lcd_point(0, 10), (1023, 589));
        assert_eq!(lcd_point(1023, 599), (0, 0));
        assert_eq!(lcd_point(512, 300), (511, 299));
    }
}

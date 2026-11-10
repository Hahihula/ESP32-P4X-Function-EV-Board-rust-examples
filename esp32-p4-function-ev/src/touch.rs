//! GT911 capacitive touch controller on the 7" LCD, polled over the shared
//! I2C bus.
//!
//! On this board the GT911's INT and RESET pins are not connected to the
//! ESP32-P4, so there is no touch interrupt: poll [`Gt911::read`], every
//! frame or every 10 ms or so (the GT911 scans at about 100 Hz). The address
//! it picked at power-up (0x5D or 0x14) is found by [`Gt911::find`].
//!
//! The touch panel is mounted upside down relative to the display: the
//! GT911 reports (0, 0) at the bottom right corner of the picture. Pass
//! [`LCD_ORIENTATION`] to [`Gt911::with_orientation`] to get display
//! coordinates.
//!
//! The protocol: registers have 16-bit big-endian addresses. When the GT911
//! has a new scan, it sets bit 7 of the status register (0x814E) and puts
//! the number of touch points in bits 0..=3; the points follow at 0x814F,
//! 8 bytes each. The host acknowledges by writing 0 to the status register.
//!
//! The I2C code is generic over `embedded-hal-async`, and the byte parsing
//! ([`Touches::parse`]) touches no hardware, so this module also builds and
//! is unit-tested on a PC.

use core::ops::Deref;

use embedded_hal_async::i2c::I2c;

/// The two I2C addresses the GT911 can come up at, default first.
pub const ADDRESSES: [u8; 2] = [0x5D, 0x14];
/// The most touch points the GT911 reports at once.
pub const MAX_POINTS: usize = 5;
/// The orientation that turns the GT911's coordinates into the display's
/// on this board: the touch panel is rotated by 180°.
pub const LCD_ORIENTATION: Orientation = Orientation {
    mirror_x: true,
    mirror_y: true,
};

/// Register addresses.
mod reg {
    /// Product ID ("911\0"), then firmware version and resolution.
    pub const PRODUCT_ID: u16 = 0x8140;
    /// Bit 7: a new scan is ready; bits 0..=3: number of touch points.
    pub const STATUS: u16 = 0x814E;
}
const STATUS_READY: u8 = 0x80;
const STATUS_COUNT_MASK: u8 = 0x0F;
const POINT_BYTES: usize = 8;
/// The status register followed by all point slots.
const STATUS_AND_POINTS: usize = 1 + MAX_POINTS * POINT_BYTES;

/// Errors from the touch controller.
#[derive(Debug, PartialEq, Eq)]
pub enum Error<E> {
    /// The I2C transfer failed.
    I2c(E),
    /// No GT911 answered at either address (is the LCD connected and
    /// powered?).
    NotFound,
}

impl<E> From<E> for Error<E> {
    fn from(e: E) -> Self {
        Self::I2c(e)
    }
}

/// Identity and configuration read from the controller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    /// The I2C address it answered at.
    pub address: u8,
    /// Product ID, ASCII, "911" on a GT911.
    pub product_id: [u8; 4],
    /// Firmware version.
    pub firmware: u16,
    /// Configured X resolution (the panel width, 1024 on this board).
    pub width: u16,
    /// Configured Y resolution (600).
    pub height: u16,
}

impl Info {
    /// The product ID as text, without the trailing NULs.
    pub fn product_id_str(&self) -> &str {
        let id = &self.product_id;
        let len = id.iter().position(|&b| b == 0).unwrap_or(id.len());
        core::str::from_utf8(&id[..len]).unwrap_or("?")
    }
}

/// How to map the GT911's coordinates to the screen's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Orientation {
    /// Flip left and right.
    pub mirror_x: bool,
    /// Flip top and bottom.
    pub mirror_y: bool,
}

/// One finger on the screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TouchPoint {
    /// Track ID: stays the same for one finger while it touches.
    pub id: u8,
    /// X position in pixels, 0 at the left edge.
    pub x: u16,
    /// Y position in pixels, 0 at the top edge.
    pub y: u16,
    /// Contact size (arbitrary units).
    pub size: u16,
}

impl TouchPoint {
    /// The point with `orientation` applied, on a `width` × `height` panel.
    #[must_use]
    pub fn oriented(self, orientation: Orientation, width: u16, height: u16) -> Self {
        let flip = |v: u16, size: u16, mirror: bool| {
            if mirror {
                size.saturating_sub(1).saturating_sub(v)
            } else {
                v
            }
        };
        Self {
            x: flip(self.x, width, orientation.mirror_x),
            y: flip(self.y, height, orientation.mirror_y),
            ..self
        }
    }
}

/// The fingers on the screen in one scan; dereferences to a slice of
/// [`TouchPoint`]s, empty when all fingers have lifted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Touches {
    points: [TouchPoint; MAX_POINTS],
    len: usize,
}

impl Deref for Touches {
    type Target = [TouchPoint];

    fn deref(&self) -> &[TouchPoint] {
        &self.points[..self.len]
    }
}

impl Touches {
    /// Parses the status register and the point slots that follow it.
    /// Returns `None` if no new scan is ready, or if the point count is
    /// invalid.
    pub fn parse(bytes: &[u8; STATUS_AND_POINTS]) -> Option<Self> {
        let (&status, slots) = bytes.split_first()?;
        if status & STATUS_READY == 0 {
            return None;
        }
        let len = usize::from(status & STATUS_COUNT_MASK);
        if len > MAX_POINTS {
            return None;
        }
        let mut touches = Self {
            len,
            ..Self::default()
        };
        let (slots, _) = slots.as_chunks::<POINT_BYTES>();
        for (point, slot) in touches.points.iter_mut().zip(slots).take(len) {
            let word = |i: usize| u16::from_le_bytes([slot[i], slot[i + 1]]);
            *point = TouchPoint {
                id: slot[0],
                x: word(1),
                y: word(3),
                size: word(5),
            };
        }
        Some(touches)
    }

    /// The touches with `orientation` applied, on a `width` × `height` panel.
    #[must_use]
    pub fn oriented(mut self, orientation: Orientation, width: u16, height: u16) -> Self {
        for point in &mut self.points[..self.len] {
            *point = point.oriented(orientation, width, height);
        }
        self
    }
}

/// The GT911 driver. `I2C` can be an owned bus or `&mut` to a shared one.
pub struct Gt911<I2C> {
    i2c: I2C,
    address: u8,
    orientation: Orientation,
    /// The resolution from the last [`info`](Self::info), for mirroring.
    width: u16,
    height: u16,
}

impl<I2C: I2c> Gt911<I2C> {
    /// A driver for the GT911 at a known `address` (see [`ADDRESSES`]).
    /// Call [`info`](Self::info) once before mirroring coordinates, so the
    /// driver knows the resolution; [`find`](Self::find) does.
    pub fn new(i2c: I2C, address: u8) -> Self {
        Self {
            i2c,
            address,
            orientation: Orientation::default(),
            width: 0,
            height: 0,
        }
    }

    /// Applies `orientation` to every point that [`read`](Self::read)
    /// returns. On this board, use [`LCD_ORIENTATION`].
    #[must_use]
    pub fn with_orientation(mut self, orientation: Orientation) -> Self {
        self.orientation = orientation;
        self
    }

    /// Looks for the GT911 at both of its addresses and reads its [`Info`].
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] if neither address answers with product ID "911".
    pub async fn find(i2c: I2C) -> Result<(Self, Info), Error<I2C::Error>> {
        let mut touch = Self::new(i2c, ADDRESSES[0]);
        for address in ADDRESSES {
            touch.address = address;
            if let Ok(info) = touch.info().await
                && info.product_id_str() == "911"
            {
                return Ok((touch, info));
            }
        }
        Err(Error::NotFound)
    }

    /// Reads the product ID, firmware version and resolution.
    ///
    /// # Errors
    ///
    /// If the I2C transfer fails.
    pub async fn info(&mut self) -> Result<Info, Error<I2C::Error>> {
        let mut bytes = [0u8; 10];
        self.read_registers(reg::PRODUCT_ID, &mut bytes).await?;
        let word = |i: usize| u16::from_le_bytes([bytes[i], bytes[i + 1]]);
        let info = Info {
            address: self.address,
            product_id: [bytes[0], bytes[1], bytes[2], bytes[3]],
            firmware: word(4),
            width: word(6),
            height: word(8),
        };
        (self.width, self.height) = (info.width, info.height);
        Ok(info)
    }

    /// Reads the latest scan. Returns `None` if there is no new one since
    /// the last call; otherwise the fingers on the screen, which is empty
    /// once they have all lifted.
    ///
    /// # Errors
    ///
    /// If an I2C transfer fails.
    pub async fn read(&mut self) -> Result<Option<Touches>, Error<I2C::Error>> {
        let mut bytes = [0u8; STATUS_AND_POINTS];
        self.read_registers(reg::STATUS, &mut bytes).await?;
        if bytes[0] & STATUS_READY == 0 {
            return Ok(None);
        }
        // Acknowledge, so the GT911 reports the next scan.
        let [hi, lo] = reg::STATUS.to_be_bytes();
        self.i2c.write(self.address, &[hi, lo, 0]).await?;
        Ok(Touches::parse(&bytes)
            .map(|touches| touches.oriented(self.orientation, self.width, self.height)))
    }

    /// Gives the I2C bus back.
    pub fn release(self) -> I2C {
        self.i2c
    }

    async fn read_registers(&mut self, start: u16, buf: &mut [u8]) -> Result<(), I2C::Error> {
        self.i2c
            .write_read(self.address, &start.to_be_bytes(), buf)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(status: u8, points: &[[u8; POINT_BYTES]]) -> [u8; STATUS_AND_POINTS] {
        let mut bytes = [0u8; STATUS_AND_POINTS];
        bytes[0] = status;
        for (slot, point) in bytes[1..].chunks_mut(POINT_BYTES).zip(points) {
            slot.copy_from_slice(point);
        }
        bytes
    }

    #[test]
    fn no_new_scan() {
        assert_eq!(Touches::parse(&scan(0x00, &[])), None);
        // A count without the ready bit is stale.
        assert_eq!(Touches::parse(&scan(0x02, &[])), None);
    }

    #[test]
    fn all_lifted() {
        let touches = Touches::parse(&scan(0x80, &[])).unwrap();
        assert!(touches.is_empty());
    }

    #[test]
    fn two_fingers() {
        let touches = Touches::parse(&scan(
            0x82,
            &[
                // id 0 at (1023, 599), size 32
                [0, 0xFF, 0x03, 0x57, 0x02, 32, 0, 0],
                // id 3 at (10, 300), size 300
                [3, 10, 0, 0x2C, 0x01, 0x2C, 0x01, 0],
            ],
        ))
        .unwrap();
        assert_eq!(
            *touches,
            [
                TouchPoint {
                    id: 0,
                    x: 1023,
                    y: 599,
                    size: 32
                },
                TouchPoint {
                    id: 3,
                    x: 10,
                    y: 300,
                    size: 300
                },
            ]
        );
    }

    #[test]
    fn rotated_180() {
        let point = TouchPoint {
            id: 1,
            x: 0,
            y: 10,
            size: 5,
        };
        assert_eq!(
            point.oriented(LCD_ORIENTATION, 1024, 600),
            TouchPoint {
                id: 1,
                x: 1023,
                y: 589,
                size: 5
            }
        );
        assert_eq!(point.oriented(Orientation::default(), 1024, 600), point);
    }

    #[test]
    fn invalid_count() {
        assert_eq!(Touches::parse(&scan(0x86, &[])), None);
    }

    #[test]
    fn product_id_text() {
        let info = Info {
            address: 0x5D,
            product_id: *b"911\0",
            firmware: 0x1060,
            width: 1024,
            height: 600,
        };
        assert_eq!(info.product_id_str(), "911");
    }
}

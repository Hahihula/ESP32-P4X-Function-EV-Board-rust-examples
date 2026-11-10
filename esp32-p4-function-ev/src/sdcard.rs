//! microSD card: esp-hal's SD host in 4-bit mode, embassy's `sdio` card
//! driver, and an adapter for the `embedded-sdmmc` FAT file system.
//!
//! The slot is wired to the SD host's slot 0 on its fixed IO_MUX pins
//! (CLK GPIO43, CMD GPIO44, D0..D3 GPIO39..42). The card and those pins
//! are powered by the chip's internal LDO channel 4 at 3.3 V, which esp-hal
//! switches on in [`host`]. The slot has no card-detect switch wired.
//!
//! The stack, bottom up:
//!
//! - [`host`] creates esp-hal's `SdHostController`; [`SdCard::init`] takes
//!   its slot 0, wires the pins and lets the `sdio` crate identify the card
//!   and switch it to a 4-bit bus at up to [`MAX_CLOCK_HZ`].
//! - [`SdCard`] implements `embedded_sdmmc::BlockDevice`, so it can be
//!   handed to an `embedded_sdmmc::VolumeManager` for FAT16/FAT32 files.
//!
//! `embedded-sdmmc` is blocking, while the card driver is async: each block
//! read or write runs the async transfer to completion in place
//! (`embassy_futures::block_on`). That blocks the executor for the length
//! of a transfer, a fraction of a millisecond per block. Other tasks still
//! run between file system calls.

use core::cell::RefCell;

use aligned::{A4, Aligned};
use block_device_driver::BlockDevice as _;
use embassy_futures::block_on;
use embassy_time::Delay;
/// The FAT file system crate [`SdCard`] plugs into, re-exported so you use
/// the same version.
pub use embedded_sdmmc;
use embedded_sdmmc::{Block, BlockCount, BlockDevice, BlockIdx};
use esp_hal::{
    Async,
    peripherals::{GPIO39, GPIO40, GPIO41, GPIO42, GPIO43, GPIO44, SDHOST},
    sdmmc::{self, SdHostController, Slot, SlotConfig},
};
use sdio::Addressable as _;
pub use sdio::{MmcError, sd::Card};

/// The fastest card clock esp-hal's SD host supports (default-speed cards
/// run at 25 MHz, high-speed cards at this).
pub const MAX_CLOCK_HZ: u32 = 40_000_000;

/// The microSD slot's pins.
#[allow(missing_docs, reason = "the field names are the signal names")]
pub struct SdPins<'d> {
    pub clk: GPIO43<'d>,
    pub cmd: GPIO44<'d>,
    pub d0: GPIO39<'d>,
    pub d1: GPIO40<'d>,
    pub d2: GPIO41<'d>,
    pub d3: GPIO42<'d>,
}

/// The [`SdPins`] taken out of `esp_hal::peripherals::Peripherals`.
#[macro_export]
macro_rules! sd_pins {
    ($p:expr) => {
        $crate::sdcard::SdPins {
            clk: $p.GPIO43,
            cmd: $p.GPIO44,
            d0: $p.GPIO39,
            d1: $p.GPIO40,
            d2: $p.GPIO41,
            d3: $p.GPIO42,
        }
    };
}

/// Errors from bringing up the card.
#[derive(Debug)]
pub enum Error {
    /// The SD host rejected its configuration.
    Config(sdmmc::ConfigError),
    /// The card didn't answer or failed to initialise (is a card inserted?).
    Card(MmcError),
}

impl From<sdmmc::ConfigError> for Error {
    fn from(e: sdmmc::ConfigError) -> Self {
        Self::Config(e)
    }
}

impl From<MmcError> for Error {
    fn from(e: MmcError) -> Self {
        Self::Card(e)
    }
}

/// Creates the SD host controller and powers the card. Keep it alive for
/// as long as the [`SdCard`]: the card borrows it.
///
/// # Errors
///
/// If the controller rejects its default configuration.
pub fn host(sdhost: SDHOST<'_>) -> Result<SdHostController<'_>, sdmmc::ConfigError> {
    SdHostController::new(sdhost, sdmmc::Config::default())
}

type Device<'d> = sdio::DefaultBlockDevice<Card, Slot<'d, 0, Async>, Delay>;

/// An initialised microSD card.
pub struct SdCard<'d> {
    device: RefCell<Device<'d>>,
}

impl<'d> SdCard<'d> {
    /// Identifies the card and switches it to the 4-bit bus at the fastest
    /// clock both sides support.
    ///
    /// # Errors
    ///
    /// If the slot is already in use, or the card doesn't answer or fails
    /// to initialise.
    pub async fn init(host: &'d SdHostController<'_>, pins: SdPins<'d>) -> Result<Self, Error> {
        let slot = host
            .slot::<0>(SlotConfig::default())?
            .with_clk(pins.clk)
            .with_cmd(pins.cmd)
            .with_data0(pins.d0)
            .with_data1(pins.d1)
            .with_data2(pins.d2)
            .with_data3(pins.d3)
            .into_async();
        let device = Device::new_sd_card(slot, MAX_CLOCK_HZ, Delay).await?;
        Ok(Self {
            device: RefCell::new(device),
        })
    }

    /// The card's registers: identity (CID), capacity (CSD), supported
    /// features (SCR) and status.
    pub fn card(&self) -> Card {
        *self.device.borrow().card()
    }

    /// The card clock in use, in Hz.
    pub fn clock_hz(&self) -> u32 {
        self.device.borrow().freq()
    }

    /// The capacity in bytes.
    pub fn size_bytes(&self) -> u64 {
        u64::from(self.device.borrow().card().block_count()) * Block::LEN as u64
    }
}

impl BlockDevice for SdCard<'_> {
    type Error = MmcError;

    fn read(&self, blocks: &mut [Block], start: BlockIdx) -> Result<(), MmcError> {
        let mut device = self.device.borrow_mut();
        // The card DMA needs 4-byte aligned buffers; `Block` is not.
        let mut buf: Aligned<A4, [u8; Block::LEN]> = Aligned([0; Block::LEN]);
        for (block, address) in blocks.iter_mut().zip(start.0..) {
            block_on(device.read(address, core::slice::from_mut(&mut buf)))?;
            block.contents = *buf;
        }
        Ok(())
    }

    fn write(&self, blocks: &[Block], start: BlockIdx) -> Result<(), MmcError> {
        let mut device = self.device.borrow_mut();
        for (block, address) in blocks.iter().zip(start.0..) {
            let buf: Aligned<A4, [u8; Block::LEN]> = Aligned(block.contents);
            block_on(device.write(address, core::slice::from_ref(&buf)))?;
        }
        Ok(())
    }

    fn num_blocks(&self) -> Result<BlockCount, MmcError> {
        Ok(BlockCount(self.device.borrow().card().block_count()))
    }
}

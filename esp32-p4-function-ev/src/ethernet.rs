//! 100 Mbit Ethernet: the chip's EMAC with the IP101GRI PHY over RMII,
//! ready for `embassy-net`.
//!
//! The IP101 drives the RMII signals and takes its 50 MHz reference clock
//! from a crystal oscillator on the board, which also feeds the ESP32-P4 on
//! GPIO50. Its management interface (MDC GPIO31, MDIO GPIO52) answers at
//! [`PHY_ADDRESS`], and its reset (GPIO51, active low) is held released for
//! as long as the [`EthernetPort`] lives.
//!
//! GPIO35 is both TXD1 and the BOOT button: while Ethernet runs, don't use
//! the button (pressing it pulls a transmit line low).
//!
//! [`EthernetPort`] implements `embassy_net_driver::Driver`, so it goes
//! straight into `embassy_net::new()`; the link state, speed and duplex
//! follow the PHY's auto-negotiation. It also works around frames longer
//! than 264 bytes getting lost on transmit with esp-hal 1.2 (see
//! `use_tx_threshold_mode` in the source).

use core::task::Context;

use embassy_net_driver::{Capabilities, Checksum, Driver, HardwareAddress, LinkState};
use embassy_time::{Duration, Timer};
use esp_hal::{
    Async, efuse,
    ethernet::{
        self, Ethernet, EthernetDmaStorage, RmiiPinBundle, clock::ExternalRefClock,
        phy::generic::GenericPhy,
    },
    gpio::{Level, Output, OutputConfig},
    peripherals::{
        ETH, GPIO28, GPIO29, GPIO30, GPIO31, GPIO34, GPIO35, GPIO49, GPIO50, GPIO51, GPIO52,
    },
};

/// MDIO address of the IP101 PHY (set by its strapping pins).
pub const PHY_ADDRESS: u8 = 1;
/// Receive and transmit DMA slots, one frame (about 1.5 KiB) each.
pub const RX_SLOTS: usize = 10;
/// See [`RX_SLOTS`].
pub const TX_SLOTS: usize = 10;

/// DMA descriptors and frame buffers, in internal RAM (about 30 KiB). Give
/// [`EthernetPort::new`] a `&'static mut` one, from a `StaticCell` for
/// example.
pub type DmaStorage = EthernetDmaStorage<RX_SLOTS, TX_SLOTS>;

/// How long to hold the PHY in reset, and to wait after releasing it.
const PHY_RESET_PULSE: Duration = Duration::from_millis(10);
const PHY_RESET_SETTLE: Duration = Duration::from_millis(50);

/// The RMII, management and reset pins of the IP101.
#[allow(missing_docs, reason = "the field names are the signal names")]
pub struct EthPins<'d> {
    /// 50 MHz reference clock from the board's oscillator.
    pub ref_clk: GPIO50<'d>,
    pub rxd0: GPIO29<'d>,
    pub rxd1: GPIO30<'d>,
    pub crs_dv: GPIO28<'d>,
    pub txd0: GPIO34<'d>,
    /// Also the BOOT button.
    pub txd1: GPIO35<'d>,
    pub tx_en: GPIO49<'d>,
    pub mdc: GPIO31<'d>,
    pub mdio: GPIO52<'d>,
    /// PHY reset, active low.
    pub phy_reset: GPIO51<'d>,
}

/// The [`EthPins`] taken out of `esp_hal::peripherals::Peripherals`.
#[macro_export]
macro_rules! eth_pins {
    ($p:expr) => {
        $crate::ethernet::EthPins {
            ref_clk: $p.GPIO50,
            rxd0: $p.GPIO29,
            rxd1: $p.GPIO30,
            crs_dv: $p.GPIO28,
            txd0: $p.GPIO34,
            txd1: $p.GPIO35,
            tx_en: $p.GPIO49,
            mdc: $p.GPIO31,
            mdio: $p.GPIO52,
            phy_reset: $p.GPIO51,
        }
    };
}

/// The MAC address the port uses: the chip's factory (universally
/// administered) MAC address from eFuse.
pub fn mac_address() -> [u8; 6] {
    let mut mac = [0; 6];
    mac.copy_from_slice(efuse::base_mac_address().as_bytes());
    mac
}

type Emac<'d> = Ethernet<'d, Async, GenericPhy>;

/// Works around the P4's small EMAC transmit FIFO.
///
/// esp-hal 1.2 runs transmit in store-and-forward mode: the EMAC only starts
/// sending once the whole frame is in its FIFO. On the ESP32-P4 that FIFO
/// holds about 256 bytes, so every frame longer than 264 bytes is silently
/// dropped (ping works, web pages don't). In threshold mode the EMAC starts
/// once 64 bytes are queued and the DMA keeps up easily at 100 Mbit/s.
/// esp-hal already does the same on the receive side for the same reason.
///
/// The catch: the EMAC inserts IP, TCP and UDP checksums only in
/// store-and-forward mode, so [`EthernetPort`] asks the stack to compute
/// them in software.
fn use_tx_threshold_mode() {
    /// `TTC = 0`: start transmitting at 64 bytes in the FIFO.
    const TX_THRESHOLD_64: u8 = 0;
    esp_hal::peripherals::EMAC_DMA::regs()
        .dmaoperation_mode()
        .modify(|_, w| {
            w.tx_str_fwd().clear_bit();
            // SAFETY: 0..=7 are all valid thresholds.
            unsafe { w.tx_thresh_ctrl().bits(TX_THRESHOLD_64) }
        });
}

/// The Ethernet port: EMAC and PHY, running. An `embassy-net` driver.
pub struct EthernetPort<'d> {
    emac: Emac<'d>,
    /// Held high (reset released) for as long as the port runs.
    _phy_reset: Output<'d>,
}

impl<'d> EthernetPort<'d> {
    /// Resets the PHY, starts the EMAC with the [`mac_address`] and starts
    /// auto-negotiation. The link comes up later, when a cable is plugged in.
    ///
    /// # Errors
    ///
    /// If the PHY doesn't answer on MDIO.
    pub async fn new(
        eth: ETH<'d>,
        pins: EthPins<'d>,
        storage: &'d mut DmaStorage,
    ) -> Result<Self, ethernet::Error> {
        let mut phy_reset = Output::new(pins.phy_reset, Level::Low, OutputConfig::default());
        Timer::after(PHY_RESET_PULSE).await;
        phy_reset.set_high();
        Timer::after(PHY_RESET_SETTLE).await;

        let emac = Ethernet::new(
            eth,
            storage,
            mac_address(),
            GenericPhy::new(PHY_ADDRESS),
            RmiiPinBundle {
                clock: ExternalRefClock::new(pins.ref_clk),
                rxd0: pins.rxd0,
                rxd1: pins.rxd1,
                rx_dv: pins.crs_dv,
                txd0: pins.txd0,
                txd1: pins.txd1,
                tx_en: pins.tx_en,
                mdc: pins.mdc,
                mdio: pins.mdio,
            },
        )?
        .into_async();
        use_tx_threshold_mode();
        Ok(Self {
            emac,
            _phy_reset: phy_reset,
        })
    }
}

/// Passed through to esp-hal's driver, except the checksum capabilities. (Called through the
/// trait: `Ethernet` also has inherent methods with the same names.)
impl<'d> Driver for EthernetPort<'d> {
    type RxToken<'a>
        = <Emac<'d> as Driver>::RxToken<'a>
    where
        Self: 'a;
    type TxToken<'a>
        = <Emac<'d> as Driver>::TxToken<'a>
    where
        Self: 'a;

    fn receive(&mut self, cx: &mut Context<'_>) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        Driver::receive(&mut self.emac, cx)
    }

    fn transmit(&mut self, cx: &mut Context<'_>) -> Option<Self::TxToken<'_>> {
        Driver::transmit(&mut self.emac, cx)
    }

    fn link_state(&mut self, cx: &mut Context<'_>) -> LinkState {
        Driver::link_state(&mut self.emac, cx)
    }

    fn capabilities(&self) -> Capabilities {
        // Without store-and-forward the EMAC can't insert checksums (see
        // `use_tx_threshold_mode`), so the stack computes them on transmit.
        // Received frames are still checked by the hardware.
        let mut caps = Driver::capabilities(&self.emac);
        let checksum = &mut caps.checksum;
        for protocol in [
            &mut checksum.ipv4,
            &mut checksum.tcp,
            &mut checksum.udp,
            &mut checksum.icmpv4,
        ] {
            *protocol = Checksum::Tx;
        }
        caps
    }

    fn hardware_address(&self) -> HardwareAddress {
        Driver::hardware_address(&self.emac)
    }
}

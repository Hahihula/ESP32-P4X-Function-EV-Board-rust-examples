//! Audio: I2S to and from the ES8311 codec.
//!
//! Wiring: MCLK GPIO13, BCLK GPIO12, WS GPIO10, DOUT GPIO9 (to the ES8311),
//! DIN GPIO11 (from the ES8311). The NS4150B speaker amplifier is enabled by
//! GPIO53. The on-board microphone is wired to the codec's MIC1 input.
//!
//! The ESP32-P4 is I2S master and exchanges 16-bit stereo frames (Philips).
//! The ES8311 driver from `oxstack-drivers` (re-exported as [`es8311`]) runs
//! the codec as slave with its clock derived from BCLK, so it needs no MCLK;
//! MCLK is still generated, at 256 × fs. The codec has a single ADC, which it
//! sends in the left channel.
//!
//! [`AudioIn`] receives through a DMA ring buffer, which has to be drained
//! often enough (see [`AudioIn::read`]).

use esp_hal::{
    Async,
    dma::DmaRxStreamBuf,
    dma_rx_stream_buffer,
    i2s::master::{Channels, DataFormat, Error as I2sError, I2s, I2sRxDmaTransfer, TdmConfig},
    peripherals::{DMA_CH0, GPIO10, GPIO11, GPIO12, GPIO13, I2S0},
    time::Rate,
};
pub use oxstack_drivers::es8311;
use oxstack_drivers::es8311::{Es8311, SampleRate};

/// Creates the ES8311 driver on the shared I2C bus (address 0x18). Call
/// [`Es8311::init`] once the I2S clocks run.
pub fn codec<I2C: embedded_hal_async::i2c::I2c>(i2c: I2C) -> Es8311<I2C> {
    Es8311::new(i2c, crate::board::i2c_addr::ES8311)
}

/// Bytes per stereo frame (two 16-bit samples).
const FRAME_BYTES: usize = 4;
/// Size of the DMA ring buffer for recording: 128 ms at 32 kHz.
const IN_RING_BYTES: usize = 16 * 1024;

/// The I2S pins for recording. [`i2s_in_pins!`](crate::i2s_in_pins) fills
/// it in from the peripherals.
#[allow(missing_docs, reason = "the field names are the signal names")]
pub struct I2sInPins<'d> {
    pub mclk: GPIO13<'d>,
    pub bclk: GPIO12<'d>,
    pub ws: GPIO10<'d>,
    /// ASDOUT of the ES8311.
    pub din: GPIO11<'d>,
}

/// The [`I2sInPins`] taken out of `esp_hal::peripherals::Peripherals`.
#[macro_export]
macro_rules! i2s_in_pins {
    ($p:expr) => {
        $crate::audio::I2sInPins {
            mclk: $p.GPIO13,
            bclk: $p.GPIO12,
            ws: $p.GPIO10,
            din: $p.GPIO11,
        }
    };
}

/// Errors from the audio streams.
#[derive(Debug)]
pub enum Error {
    /// The I2S configuration was rejected.
    Config(esp_hal::i2s::master::ConfigError),
    /// Starting a transfer failed.
    I2s(I2sError),
    /// The DMA stopped (the ring overflowed). The stream has been restarted;
    /// the next call works again.
    Dma(esp_hal::dma::DmaError),
    /// Restarting after a DMA error failed; the stream is gone.
    Stopped,
}

/// A running I2S input stream: the ES8311 ADC (the microphone), mono.
pub struct AudioIn<'d> {
    /// `None` only if restarting after an error failed.
    transfer: Option<I2sRxDmaTransfer<'d, Async, DmaRxStreamBuf>>,
    /// Bytes of a frame that was split between two DMA descriptors.
    partial: [u8; FRAME_BYTES],
    partial_len: usize,
}

impl<'d> AudioIn<'d> {
    /// Starts the I2S clocks and the receive stream at `rate`, one the ES8311
    /// driver supports. Initialise the codec once this is running.
    ///
    /// # Errors
    ///
    /// If the I2S configuration is rejected or the transfer can't start.
    pub fn new(
        i2s: I2S0<'d>,
        dma: DMA_CH0<'d>,
        pins: I2sInPins<'d>,
        rate: SampleRate,
    ) -> Result<Self, Error> {
        let config = TdmConfig::new_tdm_philips()
            .with_sample_rate(Rate::from_hz(rate.hz()))
            .with_data_format(DataFormat::Data16Channel16)
            .with_channels(Channels::STEREO);
        let i2s = I2s::new(i2s, dma, config)
            .map_err(Error::Config)?
            .with_mclk(pins.mclk)
            .into_async();
        let rx = i2s
            .i2s_rx
            .with_bclk(pins.bclk)
            .with_ws(pins.ws)
            .with_din(pins.din)
            .build();
        let buf = dma_rx_stream_buffer!(IN_RING_BYTES, 1024);
        let transfer = rx.read(buf).map_err(|(e, _, _)| Error::I2s(e))?;
        Ok(Self {
            transfer: Some(transfer),
            partial: [0; FRAME_BYTES],
            partial_len: 0,
        })
    }

    /// Waits for recorded data and passes each sample to `sample`. Returns
    /// the number of samples. Has to be called often enough that the ring
    /// (128 ms at 32 kHz) never fills up.
    ///
    /// # Errors
    ///
    /// If the ring overflowed, the DMA stops with an error. The stream is
    /// then restarted and [`Error::Dma`] returned, so the caller can log
    /// the gap; the next call records again. [`Error::Stopped`] if
    /// restarting failed.
    pub async fn read(&mut self, mut sample: impl FnMut(i16)) -> Result<usize, Error> {
        let transfer = self.transfer.as_mut().ok_or(Error::Stopped)?;
        if let Err(e) = transfer.wait_for_available_async().await {
            self.restart()?;
            return Err(Error::Dma(e));
        }
        let transfer = self.transfer.as_mut().ok_or(Error::Stopped)?;
        // The ADC is in the left channel, the first sample of each frame.
        let emit = |f: &[u8], sample: &mut dyn FnMut(i16)| sample(i16::from_le_bytes([f[0], f[1]]));
        let mut frames = 0;
        loop {
            let data = transfer.peek();
            if data.is_empty() {
                break;
            }
            let len = data.len();
            let mut rest = data;
            // A descriptor doesn't always end on a frame boundary: complete
            // the frame carried over from the previous piece first.
            if self.partial_len > 0 {
                let take = (FRAME_BYTES - self.partial_len).min(rest.len());
                self.partial[self.partial_len..self.partial_len + take]
                    .copy_from_slice(&rest[..take]);
                self.partial_len += take;
                rest = &rest[take..];
                if self.partial_len == FRAME_BYTES {
                    emit(&self.partial, &mut sample);
                    frames += 1;
                    self.partial_len = 0;
                }
            }
            let (whole, tail) = rest.as_chunks::<FRAME_BYTES>();
            for f in whole {
                emit(f, &mut sample);
            }
            frames += whole.len();
            self.partial[self.partial_len..self.partial_len + tail.len()].copy_from_slice(tail);
            self.partial_len += tail.len();
            // Always consume everything, or the ring stops draining.
            transfer.consume(len);
        }
        Ok(frames)
    }

    /// Stops the dead transfer and starts a new one on the same ring. A
    /// frame split across the gap is dropped.
    fn restart(&mut self) -> Result<(), Error> {
        let transfer = self.transfer.take().ok_or(Error::Stopped)?;
        let (rx, buf) = transfer.stop();
        self.partial_len = 0;
        let transfer = rx.read(buf).map_err(|(e, _, _)| Error::I2s(e))?;
        self.transfer = Some(transfer);
        Ok(())
    }
}

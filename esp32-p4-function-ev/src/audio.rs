//! Audio: I2S to and from the ES8311 codec.
//!
//! Wiring: MCLK GPIO13, BCLK GPIO12, WS GPIO10, DOUT GPIO9 (to the ES8311),
//! DIN GPIO11 (from the ES8311). The NS4150B speaker amplifier is enabled by
//! GPIO53. The on-board microphone is wired to the codec's MIC1 input.
//!
//! The ESP32-P4 is I2S master and exchanges 16-bit stereo frames (Philips).
//! The ES8311 driver from `es8311-async` (re-exported as [`es8311`]) runs the
//! codec as slave from MCLK, which the P4 generates at 256 × fs. The codec has
//! a single ADC, which it sends in the left channel, and a single DAC, which
//! plays the left channel.
//!
//! [`AudioOut`] is the playback side, fed by [`AudioOut::fill`]. Playback and
//! recording each take the whole I2S0 peripheral and the shared clock pins,
//! so use one at a time.
//!
//! [`AudioIn`] receives through a DMA ring buffer, which has to be drained
//! often enough (see [`AudioIn::read`]). If it isn't, the DMA stops; `read`
//! notices and restarts the stream, losing the samples in between.
//!
//! # Keeping the ring drained
//!
//! The ring holds about 125 ms at 32 kHz. A reader on the thread-mode
//! Embassy executor can't run while another task on that core runs without
//! awaiting: a full-screen redraw of the LCD in PSRAM, or heavy computation.
//! Put the reader on a higher-priority interrupt executor instead
//! (`esp_rtos::embassy::InterruptExecutor`), which preempts those. Two
//! things to keep in mind there:
//!
//! - [`AudioIn`] is not `Send` (esp-hal's async drivers aren't), so pass the
//!   peripherals ([`I2sInPins`], `I2S0`, `DMA_CH0`, which are) to the task
//!   and call [`AudioIn::new`] inside it.
//! - Keep floating-point code out of interrupt-executor tasks: the interrupt
//!   entry doesn't save the FPU registers, so float code there corrupts the
//!   float state of whatever it preempted.

use embassy_time::{Duration, with_timeout};
pub use es8311_async as es8311;
use es8311_async::Es8311;
use esp_hal::{
    Async,
    dma::{DmaRxStreamBuf, DmaTxStreamBuf},
    dma_rx_stream_buffer, dma_tx_stream_buffer,
    i2s::master::{
        Channels, DataFormat, Error as I2sError, I2s, I2sRxDmaTransfer, I2sTxDmaTransfer, TdmConfig,
    },
    peripherals::{DMA_CH0, GPIO9, GPIO10, GPIO11, GPIO12, GPIO13, I2S0},
    time::Rate,
};

/// Creates the ES8311 driver on the shared I2C bus (address 0x18). Call
/// [`Es8311::init`] once the I2S clocks run.
pub fn codec<I2C: embedded_hal_async::i2c::I2c>(i2c: I2C) -> Es8311<I2C> {
    Es8311::new(i2c, crate::board::i2c_addr::ES8311)
}

/// Bytes per stereo frame (two 16-bit samples).
const FRAME_BYTES: usize = 4;
/// Size of the DMA ring buffer for recording: 128 ms at 32 kHz.
const IN_RING_BYTES: usize = 16 * 1024;
/// Size of one DMA descriptor's chunk of the ring.
const IN_CHUNK_BYTES: usize = 1024;
/// With no data for this long, the stream has stopped. A chunk fills every
/// 2.7 ms (96 kHz) to 11.6 ms (22.05 kHz).
const STALL_TIMEOUT: Duration = Duration::from_millis(100);

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
    /// The DMA stopped (the input ring overflowed or the output ring ran
    /// dry) and reported an error. The stream has been restarted; the next
    /// call works again.
    Dma(esp_hal::dma::DmaError),
    /// No data for 100 ms: the DMA stopped without reporting an
    /// error. It does that when the ring overflows again after a restart: it
    /// runs off the end of its descriptor chain and waits. The stream has
    /// been restarted; the next call works again.
    Stalled,
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
    /// Starts the I2S clocks (MCLK at 256 × `sample_rate`) and the receive
    /// stream, at a sample rate the ES8311 driver supports. Initialise the
    /// codec once this is running.
    ///
    /// # Errors
    ///
    /// If the I2S configuration is rejected or the transfer can't start.
    pub fn new(
        i2s: I2S0<'d>,
        dma: DMA_CH0<'d>,
        pins: I2sInPins<'d>,
        sample_rate: u32,
    ) -> Result<Self, Error> {
        let config = TdmConfig::new_tdm_philips()
            .with_sample_rate(Rate::from_hz(sample_rate))
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
        let buf = dma_rx_stream_buffer!(IN_RING_BYTES, IN_CHUNK_BYTES);
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
    /// If the ring overflowed, the DMA stops, with an error
    /// ([`Error::Dma`]) or without ([`Error::Stalled`], noticed after
    /// 100 ms without data). Either way the stream is restarted
    /// before returning, so the caller can log the gap and the next call
    /// records again. [`Error::Stopped`] if restarting failed.
    pub async fn read(&mut self, mut sample: impl FnMut(i16)) -> Result<usize, Error> {
        let transfer = self.transfer.as_mut().ok_or(Error::Stopped)?;
        match with_timeout(STALL_TIMEOUT, transfer.wait_for_available_async()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                self.restart()?;
                return Err(Error::Dma(e));
            }
            Err(_) => {
                self.restart()?;
                return Err(Error::Stalled);
            }
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

/// The I2S pins for playback. [`i2s_out_pins!`](crate::i2s_out_pins) fills
/// it in from the peripherals.
#[allow(missing_docs, reason = "the field names are the signal names")]
pub struct I2sOutPins<'d> {
    pub mclk: GPIO13<'d>,
    pub bclk: GPIO12<'d>,
    pub ws: GPIO10<'d>,
    /// DSDIN of the ES8311.
    pub dout: GPIO9<'d>,
}

/// The [`I2sOutPins`] taken out of `esp_hal::peripherals::Peripherals`.
#[macro_export]
macro_rules! i2s_out_pins {
    ($p:expr) => {
        $crate::audio::I2sOutPins {
            mclk: $p.GPIO13,
            bclk: $p.GPIO12,
            ws: $p.GPIO10,
            dout: $p.GPIO9,
        }
    };
}

/// Size of the DMA ring buffer for playback: about 46 ms at 44.1 kHz.
const OUT_RING_BYTES: usize = 8192;

/// A running I2S output stream: the ES8311 DAC (the speaker), mono.
pub struct AudioOut<'d> {
    /// `None` only if restarting after an error failed.
    transfer: Option<I2sTxDmaTransfer<'d, Async, DmaTxStreamBuf>>,
}

impl<'d> AudioOut<'d> {
    /// Starts the I2S clocks (MCLK at 256 × `sample_rate`) and a stream of
    /// silence, at a sample rate the ES8311 driver supports. Initialise the
    /// codec once this is running.
    ///
    /// # Errors
    ///
    /// If the I2S configuration is rejected or the transfer can't start.
    pub fn new(
        i2s: I2S0<'d>,
        dma: DMA_CH0<'d>,
        pins: I2sOutPins<'d>,
        sample_rate: u32,
    ) -> Result<Self, Error> {
        let config = TdmConfig::new_tdm_philips()
            .with_sample_rate(Rate::from_hz(sample_rate))
            .with_data_format(DataFormat::Data16Channel16)
            .with_channels(Channels::STEREO);
        let i2s = I2s::new(i2s, dma, config)
            .map_err(Error::Config)?
            .with_mclk(pins.mclk)
            .into_async();
        let tx = i2s
            .i2s_tx
            .with_bclk(pins.bclk)
            .with_ws(pins.ws)
            .with_dout(pins.dout)
            .build();
        let mut buf = dma_tx_stream_buffer!(OUT_RING_BYTES, 1024);
        // The stream must be pre-filled before it starts; fill all of it, so
        // there is slack before the first refill.
        while buf.push(&[0u8; 512]) > 0 {}
        let transfer = tx.write(buf).map_err(|(e, _, _)| Error::I2s(e))?;
        Ok(Self {
            transfer: Some(transfer),
        })
    }

    /// Waits until there is room in the ring, then lets `render` fill all of
    /// it, in pieces of up to 256 mono samples. Each sample goes to both
    /// channels (the ES8311 plays the left one).
    ///
    /// Call again before the ring runs dry (about 46 ms at 44.1 kHz). If the
    /// DMA stops anyway, the stream is restarted (the ring replays its old
    /// contents once) and [`Error::Dma`] is returned so the caller can log it.
    ///
    /// # Errors
    ///
    /// [`Error::Dma`] after a restart, [`Error::Stopped`] if restarting failed.
    pub async fn fill(&mut self, mut render: impl FnMut(&mut [i16])) -> Result<(), Error> {
        let transfer = self.transfer.as_mut().ok_or(Error::Stopped)?;
        if let Err(e) = transfer.wait_for_available_async().await {
            self.restart()?;
            return Err(Error::Dma(e));
        }
        let mut mono = [0i16; 256];
        while transfer.available_bytes() >= FRAME_BYTES {
            transfer.push_with(|buf| {
                let frames = (buf.len() / FRAME_BYTES).min(mono.len());
                render(&mut mono[..frames]);
                for (frame, s) in buf
                    .as_chunks_mut::<FRAME_BYTES>()
                    .0
                    .iter_mut()
                    .zip(&mono[..frames])
                {
                    let [lo, hi] = s.to_le_bytes();
                    frame.copy_from_slice(&[lo, hi, lo, hi]);
                }
                frames * FRAME_BYTES
            });
        }
        Ok(())
    }

    /// Stops the dead transfer and starts a new one on the same ring.
    fn restart(&mut self) -> Result<(), Error> {
        let transfer = self.transfer.take().ok_or(Error::Stopped)?;
        let (tx, buf) = transfer.stop();
        self.transfer = Some(tx.write(buf).map_err(|(e, _, _)| Error::I2s(e))?);
        Ok(())
    }
}

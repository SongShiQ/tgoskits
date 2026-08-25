#![no_std]

//! Portable hardware core for the RK3588 I2S/TDM controller used by the
//! Orange Pi 5 Plus ES8388 capture path.
//!
//! MMIO mapping, clock/reset/pinctrl setup, DMA allocation, IRQ registration,
//! and StarryOS VFS integration intentionally remain outside this crate.

use core::ptr::{read_volatile, write_volatile};

use thiserror::Error;

pub const RK3588_I2S_TDM_BASE: usize = 0xfe47_0000;
pub const RK3588_I2S_TDM_IRQ: u32 = 0xb4;
pub const RK3588_I2S_TDM_REGISTER_SIZE: usize = 0x1000;

const TXCR: usize = 0x000;
const RXCR: usize = 0x004;
const CKR: usize = 0x008;
const DMACR: usize = 0x010;
const INTCR: usize = 0x014;
const INTSR: usize = 0x018;
const XFER: usize = 0x01c;
const CLR: usize = 0x020;

const XFER_RX_START: u32 = 1 << 1;
const RXFIFOLR: usize = 0x02c;
const RXCR_VDW_16_BIT: u32 = 15;
const RXCR_CSR_TWO_SLOTS: u32 = 0;
const INT_RX_READY: u32 = 1 << 16;
const INT_RX_OVERFLOW: u32 = 1 << 17;
const INT_RX_OVERFLOW_CLEAR: u32 = 1 << 18;
const DMACR_RX_ENABLE: u32 = 1 << 24;
const DMACR_RX_LEVEL: u32 = 16 << 16;
const INT_RX_THRESHOLD: u32 = 16 << 20;
const INT_RX_ENABLE: u32 = 1 << 16;
const INT_RX_OVERFLOW_ENABLE: u32 = 1 << 17;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum AudioConfigError {
    #[error("only mono capture is supported in the first implementation")]
    UnsupportedChannels,
    #[error("only 16-bit PCM is supported in the first implementation")]
    UnsupportedSampleWidth,
    #[error("only 16 kHz and 48 kHz capture are supported in the first implementation")]
    UnsupportedSampleRate,
    #[error("I2S clock dividers must be non-zero")]
    InvalidClockDividers,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureFormat {
    pub sample_rate_hz: u32,
    pub channels: u8,
    pub sample_width_bits: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockDividers {
    pub mclk_div: u8,
    pub rx_bclk_div: u8,
    pub tx_bclk_div: u8,
}

impl ClockDividers {
    pub const fn validate(self) -> bool {
        self.mclk_div != 0 && self.rx_bclk_div != 0 && self.tx_bclk_div != 0
    }

    const fn register_value(self) -> u32 {
        (((self.mclk_div - 1) as u32) << 16)
            | (((self.rx_bclk_div - 1) as u32) << 8)
            | (self.tx_bclk_div - 1) as u32
    }
}

impl CaptureFormat {
    pub const MONO_16K: Self = Self {
        sample_rate_hz: 16_000,
        channels: 1,
        sample_width_bits: 16,
    };

    pub const MONO_48K: Self = Self {
        sample_rate_hz: 48_000,
        channels: 1,
        sample_width_bits: 16,
    };

    pub const fn validate(self) -> Result<Self, AudioConfigError> {
        if self.channels != 1 {
            return Err(AudioConfigError::UnsupportedChannels);
        }
        if self.sample_width_bits != 16 {
            return Err(AudioConfigError::UnsupportedSampleWidth);
        }
        if self.sample_rate_hz != 16_000 && self.sample_rate_hz != 48_000 {
            return Err(AudioConfigError::UnsupportedSampleRate);
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrqEvent {
    None,
    RxReady,
    RxOverflow,
}

/// Register-level controller. The caller owns the MMIO mapping for its whole
/// lifetime and must serialize task-context register access against IRQ code.
pub struct I2sTdmController {
    base: *mut u8,
    format: CaptureFormat,
}

// SAFETY: moving the controller transfers exclusive ownership of the MMIO
// mapping; all register methods require `&mut self`, so this type does not
// permit concurrent access through the portable core.
unsafe impl Send for I2sTdmController {}

impl I2sTdmController {
    /// # Safety
    /// `base` must be a valid, aligned mapping of the RK3588 register file and
    /// remain valid until this controller is dropped.
    pub unsafe fn from_mmio(
        base: *mut u8,
        format: CaptureFormat,
    ) -> Result<Self, AudioConfigError> {
        let format = format.validate()?;
        Ok(Self { base, format })
    }

    pub const fn format(&self) -> CaptureFormat {
        self.format
    }

    pub fn configure_capture(&mut self, clock: ClockDividers) -> Result<(), AudioConfigError> {
        if !clock.validate() {
            return Err(AudioConfigError::InvalidClockDividers);
        }
        self.write(TXCR, 0);
        self.write(RXCR, RXCR_CSR_TWO_SLOTS | RXCR_VDW_16_BIT);
        self.write(CKR, clock.register_value());
        self.write(RXFIFOLR, 0);
        self.write(DMACR, DMACR_RX_ENABLE | DMACR_RX_LEVEL);
        self.write(
            INTCR,
            INT_RX_THRESHOLD | INT_RX_ENABLE | INT_RX_OVERFLOW_ENABLE | INT_RX_OVERFLOW_CLEAR,
        );
        self.write(CLR, 1 << 1);
        Ok(())
    }

    pub fn start_capture(&mut self) {
        self.write(XFER, XFER_RX_START);
    }

    pub fn stop_capture(&mut self) {
        self.write(XFER, 0);
    }

    pub fn handle_irq(&mut self) -> IrqEvent {
        let status = self.read(INTSR);
        self.write(CLR, status);
        if status & INT_RX_OVERFLOW != 0 {
            IrqEvent::RxOverflow
        } else if status & INT_RX_READY != 0 {
            IrqEvent::RxReady
        } else {
            IrqEvent::None
        }
    }

    fn read(&self, offset: usize) -> u32 {
        // SAFETY: construction requires a valid MMIO mapping; offsets are in
        // the RK3588 register file and accesses are volatile by definition.
        unsafe { read_volatile(self.base.add(offset).cast::<u32>()) }
    }

    fn write(&mut self, offset: usize, value: u32) {
        // SAFETY: construction requires a valid MMIO mapping; offsets are in
        // the RK3588 register file and accesses are volatile by definition.
        unsafe { write_volatile(self.base.add(offset).cast::<u32>(), value) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RingError {
    Full,
    Empty,
}

/// Single-producer (DMA/IRQ) and single-consumer (read) PCM ring.
pub struct PcmRing<const CAPACITY: usize> {
    samples: [i16; CAPACITY],
    read: usize,
    write: usize,
    len: usize,
    overruns: usize,
}

impl<const CAPACITY: usize> PcmRing<CAPACITY> {
    pub const fn new() -> Self {
        Self {
            samples: [0; CAPACITY],
            read: 0,
            write: 0,
            len: 0,
            overruns: 0,
        }
    }

    pub const fn capacity(&self) -> usize {
        CAPACITY
    }

    pub const fn available(&self) -> usize {
        self.len
    }

    pub const fn overruns(&self) -> usize {
        self.overruns
    }

    pub fn push(&mut self, sample: i16) -> Result<(), RingError> {
        if CAPACITY == 0 || self.len == CAPACITY {
            self.overruns = self.overruns.saturating_add(1);
            return Err(RingError::Full);
        }
        self.samples[self.write] = sample;
        self.write = (self.write + 1) % CAPACITY;
        self.len += 1;
        Ok(())
    }

    pub fn pop(&mut self) -> Result<i16, RingError> {
        if self.len == 0 {
            return Err(RingError::Empty);
        }
        let sample = self.samples[self.read];
        self.read = (self.read + 1) % CAPACITY;
        self.len -= 1;
        Ok(sample)
    }

    pub fn pop_slice(&mut self, output: &mut [i16]) -> usize {
        let mut count = 0;
        while count < output.len() {
            let Ok(sample) = self.pop() else { break };
            output[count] = sample;
            count += 1;
        }
        count
    }
}

impl<const CAPACITY: usize> Default for PcmRing<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_validation_rejects_unimplemented_shapes() {
        assert_eq!(
            CaptureFormat::MONO_48K.validate(),
            Ok(CaptureFormat::MONO_48K)
        );
        assert_eq!(
            CaptureFormat {
                channels: 2,
                ..CaptureFormat::MONO_48K
            }
            .validate(),
            Err(AudioConfigError::UnsupportedChannels)
        );
        assert_eq!(
            CaptureFormat {
                sample_width_bits: 24,
                ..CaptureFormat::MONO_48K
            }
            .validate(),
            Err(AudioConfigError::UnsupportedSampleWidth)
        );
    }

    #[test]
    fn ring_preserves_order_and_reports_overrun() {
        let mut ring = PcmRing::<2>::new();
        assert_eq!(ring.push(10), Ok(()));
        assert_eq!(ring.push(20), Ok(()));
        assert_eq!(ring.push(30), Err(RingError::Full));
        assert_eq!(ring.overruns(), 1);
        assert_eq!(ring.pop(), Ok(10));
        assert_eq!(ring.pop(), Ok(20));
        assert_eq!(ring.pop(), Err(RingError::Empty));
    }
}

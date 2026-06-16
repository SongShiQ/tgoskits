//! Cpuset controller — read-only CPU/memory node affinity.
//!
//! In StarryOS this controller is **read-only**: it exposes the effective
//! CPU set and memory node set but does not enforce affinity. This matches
//! the minimum requirement for Docker and systemd which probe these files
//! even if they are not actively controlling placement.
//!
//! Exposes control files:
//! - `cpuset.cpus` — allowed CPUs (read-only)
//! - `cpuset.cpus.effective` — effective CPUs (read-only)
//! - `cpuset.mems` — allowed memory nodes (read-only)
//! - `cpuset.mems.effective` — effective memory nodes (read-only)

use alloc::format;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU32, Ordering};

use ax_errno::{AxError, AxResult};

use super::{Controller, SubControl, SubControlStatic, SubController};

// ---------------------------------------------------------------------------
// CpuSetController
// ---------------------------------------------------------------------------

/// Read-only cpuset controller.
///
/// Reports the system-wide CPU and memory node mask. Since StarryOS
/// currently targets single-CPU systems, the effective set is always `0`.
pub struct CpuSetController {
    /// Bitmask of available CPUs. Currently always `1 << 0` (CPU 0).
    cpus_mask: AtomicU32,
    /// Bitmask of available memory nodes. Currently always `1 << 0` (node 0).
    mems_mask: AtomicU32,
}

impl CpuSetController {
    /// Return the effective CPU set as a range string (e.g. "0-3").
    fn cpus_effective_str(&self) -> alloc::string::String {
        let mask = self.cpus_mask.load(Ordering::Relaxed);
        mask_to_range_str(mask)
    }

    /// Return the effective memory node set as a range string.
    fn mems_effective_str(&self) -> alloc::string::String {
        let mask = self.mems_mask.load(Ordering::Relaxed);
        mask_to_range_str(mask)
    }
}

// ---------------------------------------------------------------------------
// SubControl impl
// ---------------------------------------------------------------------------

impl SubControl for CpuSetController {
    fn read_attr_at(&self, name: &str, offset: usize, buf: &mut [u8]) -> AxResult<usize> {
        let value = match name {
            "cpuset.cpus" | "cpuset.cpus.effective" => format!("{}\n", self.cpus_effective_str()),
            "cpuset.mems" | "cpuset.mems.effective" => format!("{}\n", self.mems_effective_str()),
            _ => return Err(AxError::NotFound),
        };

        let bytes = value.as_bytes();
        if offset >= bytes.len() {
            return Ok(0);
        }
        let remaining = &bytes[offset..];
        let n = remaining.len().min(buf.len());
        buf[..n].copy_from_slice(&remaining[..n]);
        Ok(n)
    }

    fn write_attr(&self, name: &str, _data: &[u8]) -> AxResult<usize> {
        // All cpuset attributes are read-only in StarryOS.
        match name {
            "cpuset.cpus" | "cpuset.cpus.effective" | "cpuset.mems" | "cpuset.mems.effective" => {
                Err(AxError::PermissionDenied)
            }
            _ => Err(AxError::NotFound),
        }
    }
}

// ---------------------------------------------------------------------------
// SubControlStatic impl
// ---------------------------------------------------------------------------

impl SubControlStatic for CpuSetController {
    fn new(is_root: bool, _is_active: bool) -> Self {
        // Default: all CPUs and nodes available (single-CPU: mask = 0x1).
        let _ = is_root; // Root and child get the same default.
        Self {
            cpus_mask: AtomicU32::new(0x1), // CPU 0
            mems_mask: AtomicU32::new(0x1), // Node 0
        }
    }

    fn read_from(controller: &Controller) -> Arc<SubController<Self>> {
        controller.cpuset()
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a bitmask to a compact range string.
///
/// Examples:
/// - `0b0001` → `"0"`
/// - `0b1111` → `"0-3"`
/// - `0b1010` → `"1,3"`
fn mask_to_range_str(mask: u32) -> alloc::string::String {
    use alloc::string::String;
    use alloc::vec::Vec;

    if mask == 0 {
        return String::new();
    }

    let mut ranges: Vec<(u32, u32)> = Vec::new();
    let mut i = 0u32;
    while i < 32 {
        if mask & (1 << i) != 0 {
            let start = i;
            while i < 32 && mask & (1 << i) != 0 {
                i += 1;
            }
            ranges.push((start, i - 1));
        } else {
            i += 1;
        }
    }

    let mut result = String::new();
    for (idx, &(start, end)) in ranges.iter().enumerate() {
        if idx > 0 {
            result.push(',');
        }
        if start == end {
            use core::fmt::Write;
            let _ = write!(result, "{start}");
        } else {
            use core::fmt::Write;
            let _ = write!(result, "{start}-{end}");
        }
    }
    result
}

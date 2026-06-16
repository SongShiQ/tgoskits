<h1 align="center">ax-cgroup</h1>

<p align="center">cgroup v2 subsystem for StarryOS</p>

<div align="center">

[![Crates.io](https://img.shields.io/crates/v/ax-cgroup.svg)](https://crates.io/crates/ax-cgroup)
[![Docs.rs](https://docs.rs/ax-cgroup/badge.svg)](https://docs.rs/ax-cgroup)
[![Rust](https://img.shields.io/badge/edition-2021-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](./LICENSE)

</div>

English | [中文](README_CN.md)

# Introduction

`ax-cgroup` provides a kernel-independent cgroup v2 subsystem for StarryOS. It
owns the cgroup hierarchy, per-controller state, and process membership
bookkeeping. The crate is `no_std` and does not depend on the kernel task
layer directly: the kernel supplies the task/process primitives by
implementing the [`CgroupProvider`] trait and registering it during boot.

This crate is maintained as part of the TGOSKits component set and is intended
for Rust projects that integrate with ArceOS, StarryOS, or related low-level
systems software.

## Design

The implementation follows the Linux cgroup v2 semantics and borrows several
ideas from the [Asterinas](https://github.com/asterinas/asterinas) cgroupfs
(domain-controller rules, global membership serialization, `subtree_control`
propagation). It is **not** a port of Asterinas' `SysTree`-based architecture:
StarryOS has no `aster_systree` component and uses `axfs-ng-vfs`, so the
hierarchy here is a self-contained tree rather than a `SysBranchNode` graph.

Concrete differences from Asterinas:

| Aspect            | Asterinas                               | ax-cgroup                                      |
| ----------------- | --------------------------------------- | ---------------------------------------------- |
| Hierarchy         | `SysTree` (`SysBranchNode` / `SysObj`)  | self-managed `BTreeMap<String, Arc<CgroupNode>>` |
| Controller access | `Controller` + `SubControl` trait       | dynamic registry (factory/instance pattern)    |
| Attribute I/O     | trait-method dispatch                    | registry dispatch via `CgroupController` trait |
| Membership lock   | `CgroupMembership` global `Mutex`        | `SpinNoIrq<MembershipState>` (`LazyInit`)      |
| Filesystem        | custom cgroupfs over `SysTree`           | `axfs-ng-vfs` adapter in the kernel            |

### Controller Registry

The cgroup subsystem uses a **factory/instance registry pattern** to support dynamic controller registration. This allows adding new controllers without modifying the central dispatch code in `lib.rs`.

#### Architecture

```
Global Registry (once at boot)
  ↓
CgroupControllerFactory (name, attr_names, new_instance)
  ↓
Per-node instances
  ↓
CgroupController (read_attr, write_attr)
```

#### Adding a New Controller

To add a new controller (e.g., `memory`):

**Step 1: Create the controller file** (`src/memory.rs`):

```rust
use alloc::{format, sync::Arc};
use core::sync::atomic::{AtomicI64, Ordering};
use axfs_ng_vfs::{VfsError, VfsResult};
use super::controller::{AttrInfo, CgroupController, CgroupControllerFactory, write_to_buf};

// Define state structure
pub struct MemoryState {
    pub current: AtomicI64,
    pub max: AtomicI64,
}

impl MemoryState {
    pub fn new() -> Self {
        Self {
            current: AtomicI64::new(0),
            max: AtomicI64::new(-1),
        }
    }
}

// Define controller instance
const MEMORY_ATTRS: &[AttrInfo] = &[
    AttrInfo { name: "current", read_only: true },
    AttrInfo { name: "max", read_only: false },
];

pub struct MemoryController {
    state: Arc<MemoryState>,
}

impl MemoryController {
    pub fn new(state: Arc<MemoryState>) -> Self {
        Self { state }
    }
}

impl CgroupController for MemoryController {
    fn name(&self) -> &str { "memory" }
    
    fn read_attr(&self, name: &str, offset: usize, buf: &mut [u8]) -> VfsResult<usize> {
        let value = match name {
            "current" => format!("{}\n", self.state.current.load(Ordering::Acquire)),
            "max" => {
                let max = self.state.max.load(Ordering::Acquire);
                if max < 0 { "max\n".to_string() } else { format!("{}\n", max) }
            }
            _ => return Err(VfsError::NotFound),
        };
        write_to_buf(&value, offset, buf)
    }
    
    fn write_attr(&self, name: &str, data: &[u8]) -> VfsResult<usize> {
        let text = core::str::from_utf8(data)
            .map_err(|_| VfsError::InvalidInput)?
            .trim();
        match name {
            "max" => {
                let value = if text == "max" {
                    -1
                } else {
                    text.parse::<i64>().map_err(|_| VfsError::InvalidInput)?
                };
                self.state.max.store(value, Ordering::Release);
                Ok(data.len())
            }
            "current" => Err(VfsError::OperationNotPermitted),
            _ => Err(VfsError::NotFound),
        }
    }
    
    fn attr_names(&self) -> &[AttrInfo] { MEMORY_ATTRS }
    fn as_any(&self) -> &dyn core::any::Any { self }
}

// Define factory
pub struct MemoryControllerFactory;

impl CgroupControllerFactory for MemoryControllerFactory {
    fn name(&self) -> &str { "memory" }
    fn attr_names(&self) -> &[AttrInfo] { MEMORY_ATTRS }
    fn new_instance(&self) -> Arc<dyn CgroupController> {
        Arc::new(MemoryController {
            state: Arc::new(MemoryState::new()),
        })
    }
}
```

**Step 2: Register the factory** in `src/lib.rs`:

```rust
pub mod memory;

pub fn init() {
    // ...
    controller::register_factory(Arc::new(pids::PidsControllerFactory));
    controller::register_factory(Arc::new(cpu::CpuControllerFactory));
    controller::register_factory(Arc::new(memory::MemoryControllerFactory));  // Add this line
}
```

**Step 3: Done!** No need to modify central dispatch code in `lib.rs`.

#### Design Principles

1. **Factory/Instance Separation**: `CgroupControllerFactory` is registered globally, `CgroupController` is per-node.
2. **Arc Lifecycle Management**: Use `Arc` instead of `&'static` for flexibility.
3. **Multi-dot Attribute Names**: Supports `cpu.stat.periods` format via `parse_attr_name()`.
4. **Controller Name Constraint**: Controller names must not contain `.` character.
5. **Immutable Controllers Map**: The `controllers` map in `CgroupNode` is immutable after node creation (lock-free read path).

### Module layout

| Module        | Responsibility                                                       |
| ------------- | -------------------------------------------------------------------- |
| `controller`  | `CgroupController` / `CgroupControllerFactory` traits and global registry. |
| `core`        | `CgroupNode`, the global root, and the id-to-node registry.          |
| `pids`        | `PidsState` / `PidsController` — process-count accounting with a CAS-based charge path. |
| `cpu`         | `CpuState` / `CpuController` — `cpu.weight` and `cpu.max` state.    |
| `provider`    | `CgroupProvider` trait and the registration cell.                    |
| crate root    | membership, fork/migrate/exit transactions, and attribute parsing.   |

### Supported Controllers

Five controllers are implemented:

#### pids
- **Attributes**: `pids.max` (read-write), `pids.current` (read-only)
- **Purpose**: Limits the number of processes in a cgroup
- **Implementation**: Charging walks the path to the root and rolls back partial charges on failure; the per-node counter uses a CAS loop to avoid the TOCTOU race on SMP
- **Fast path**: `CgroupNode.pids` field provides direct access for `begin_fork()` to avoid registry lookup overhead

#### cpu
- **Attributes**: 
  - `cpu.weight` (read-write) — CPU weight (1-10000, default 100)
  - `cpu.max` (read-write) — CPU bandwidth limit in microseconds (format: `"QUOTA PERIOD"` or `"max PERIOD"`)
  - `cpu.stat` (read-only) — CPU usage statistics (`nr_periods`, `nr_throttled`, `throttled_usec`)
- **Purpose**: Controls CPU time allocation and bandwidth throttling
- **Implementation**: The bandwidth quota/period state is maintained here; the timer-tick enforcement hook lives on the kernel side because it needs `ax_task` / `ax_hal` access
- **Domain controller**: Returns `is_domain() = true` (affects process hosting rules)

#### cpuset
- **Attributes**:
  - `cpuset.cpus` (read-write) — Allowed CPU list (format: `"0-3,5,7"` or `"0,1,2"`)
  - `cpuset.mems` (read-write) — Allowed memory node list (same format as cpus)
  - `cpuset.cpus.effective` (read-only) — Effective CPU mask after hierarchy intersection
  - `cpuset.mems.effective` (read-only) — Effective memory node mask after hierarchy intersection
- **Purpose**: Restricts processes to specific CPUs and memory nodes
- **Implementation**: Uses 64-bit bitmaps for CPU/memory masks; child nodes inherit parent masks on creation (subset constraint)
- **Domain controller**: Returns `is_domain() = true` (affects process hosting rules)
- **Format**: Supports range notation (`0-3`) and comma-separated lists (`0,2,4`)

#### memory
- **Attributes**:
  - `memory.current` (read-only) — Current memory usage in bytes
  - `memory.max` (read-write) — Hard memory limit (format: `"1G"`, `"512M"`, `"1048576"`, or `"max"`)
  - `memory.high` (read-write) — High watermark for memory reclaim pressure (same format as max)
  - `memory.low` (read-write) — Memory protection threshold in bytes (0 = no protection)
  - `memory.min` (read-write) — Hard memory protection in bytes (0 = no protection)
  - `memory.events` (read-only) — Event counters (`max`, `high`, `oom`)
- **Purpose**: Limits and tracks memory usage
- **Implementation**: Supports human-readable size suffixes (K, M, G, T); provides charge/uncharge methods for memory accounting
- **Domain controller**: Returns `is_domain() = true` (affects process hosting rules)
- **Format**: Accepts both numeric bytes and size suffixes (case-insensitive)

#### io
- **Attributes**:
  - `io.weight` (read-write) — Default I/O weight (1-10000, default 100)
  - `io.max` (read-write) — Per-device I/O limits (format: `"MAJOR:MINOR rbps=N wbps=N riops=N wiops=N"`)
  - `io.stat` (read-only) — Per-device I/O statistics (format: `"MAJOR:MINOR rbytes=N wbytes=N rios=N wios=N"`)
- **Purpose**: Controls I/O bandwidth and prioritization for block devices
- **Implementation**: Per-device limits and statistics; supports both bandwidth (bps) and IOPS limits
- **Domain controller**: Returns `is_domain() = true` (affects process hosting rules)
- **Format**: Device specified as `major:minor` (e.g., `8:0` for `/dev/sda`); `max` means unlimited

## Quick Start

### Installation

Add this crate to your `Cargo.toml`:

```toml
[dependencies]
ax-cgroup = "0.1.0"
```

### Usage

```rust,ignore
use alloc::sync::Arc;
use ax_cgroup::{CgroupNode, CgroupProvider};

struct KernelProvider;

impl CgroupProvider for KernelProvider {
    fn is_zombie(&self, pid: u32) -> bool {
        // query the kernel process table
    }
    fn get_cgroup(&self, pid: u32) -> Option<Arc<CgroupNode>> {
        // return the process's current cgroup
    }
    fn set_cgroup(&self, pid: u32, cgroup: Arc<CgroupNode>) {
        // store the process's new cgroup
    }
}

static PROVIDER: KernelProvider = KernelProvider;

fn boot() {
    ax_cgroup::init();
    ax_cgroup::register_provider(&PROVIDER);
}
```

### Run Check and Test

```bash
# Enter the crate directory
cd components/ax-cgroup

# Format code
cargo fmt --all

# Run clippy
cargo clippy --all-targets --all-features

# Build documentation
cargo doc --no-deps
```

# Contributing

1. Fork the repository and create a branch
2. Run local format and checks
3. Run local tests relevant to this crate
4. Submit a PR and ensure CI passes

# License

Licensed under the Apache License, Version 2.0. See [LICENSE](../../LICENSE) for details.

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
| Controller access | `Controller` + `SubControl` trait       | registry-based `BTreeMap<String, Arc<dyn CgroupController>>` |
| Attribute I/O     | trait-method dispatch                    | trait-method dispatch via controller instances |
| Membership lock   | `CgroupMembership` global `Mutex`        | `SpinNoIrq<MembershipState>` (`LazyInit`)      |
| Filesystem        | custom cgroupfs over `SysTree`           | `axfs-ng-vfs` adapter in the kernel            |

### Module layout

| Module        | Responsibility                                                       |
| ------------- | -------------------------------------------------------------------- |
| `controller`  | `CgroupController` / `CgroupControllerFactory` traits, factory registry, `parse_attr_name`. |
| `core`        | `CgroupNode`, the global root, and the id-to-node registry.          |
| `pids`        | `PidsState` + `PidsController` — process-count accounting with a CAS-based charge path. |
| `cpu`         | `CpuState` + `CpuController` / `BandwidthState` — `cpu.weight` and `cpu.max` state. |
| `provider`    | `CgroupProvider` trait and the registration cell.                    |
| crate root    | membership, fork/migrate/exit transactions, and attribute dispatch.  |

### Controllers

Controllers follow a **factory + instance** pattern:

- **Factory** (`CgroupControllerFactory`): registered globally during `init()`.
  Reports controller name, attribute metadata, and creates per-node instances.
- **Instance** (`CgroupController`): stored per-node in
  `CgroupNode.controllers`. Handles `read_attr` / `write_attr` for the
  controller's attributes. Attribute names are **short** (without the controller
  prefix): e.g. `"max"` not `"pids.max"`.

Two controllers are built in:

- **pids** — `pids.max` / `pids.current`. Charging walks the path to the root
  and rolls back partial charges on failure; the per-node counter uses a CAS
  loop to avoid the TOCTOU race on SMP. The `PidsState` is also stored directly
  on `CgroupNode.pids` for lock-free fork fast path.
- **cpu** — `cpu.weight`, `cpu.max` (quota/period), and `cpu.stat`. The
  bandwidth quota/period state is maintained here; the timer-tick enforcement
  hook lives on the kernel side because it needs `ax_task` / `ax_hal` access.

### Adding a new controller

1. Define state struct (e.g. `MemoryState`) and controller struct
   (`MemoryController`) implementing `CgroupController`.
2. Define a factory struct (`MemoryControllerFactory`) implementing
   `CgroupControllerFactory`.
3. Register the factory in `init()`:
   ```rust
   controller::register_factory(Arc::new(MemoryControllerFactory));
   ```
4. The `subtree_control` mechanism, attribute dispatch, and interface file
   detection all work automatically through the registry.

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
    // init() registers built-in controller factories and creates the root node
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

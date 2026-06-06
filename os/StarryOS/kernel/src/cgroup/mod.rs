//! cgroup v2 subsystem skeleton.

mod core;
pub mod cpu;
pub mod pids;

pub use core::{CgroupNode, GLOBAL_CGROUP_ROOT};

/// Initialize the cgroup subsystem. Called once during boot.
pub fn init() {
    core::init();

    // Register bandwidth tick hook for CPU bandwidth accounting.
    // This is called on each scheduler timer tick to track CPU usage
    // and enforce cpu.max quotas.
    ax_task::set_tick_hook(cpu::bandwidth_tick);

    info!("cgroup: initialized");
}

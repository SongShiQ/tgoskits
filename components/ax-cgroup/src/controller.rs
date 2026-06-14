//! Controller trait definitions and factory registry.
//!
//! This module defines the core abstractions for cgroup controllers:
//! - [`CgroupController`]: per-node instance that handles attribute I/O
//! - [`CgroupControllerFactory`]: global factory that creates instances and reports metadata
//! - Factory registry: global map of controller name → factory
//!
//! New controllers register their factory via [`register_factory`] during `init()`.
//! The central dispatch in `lib.rs` uses [`parse_attr_name`] to route attribute
//! operations to the correct controller instance.

use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use core::any::Any;

use ax_kspin::SpinNoIrq;
use ax_lazyinit::LazyInit;
use axfs_ng_vfs::VfsResult;

/// Attribute metadata (short name without controller prefix).
#[derive(Debug, Clone, Copy)]
pub struct AttrInfo {
    /// Attribute name as seen by the controller (e.g. "max", "current", "weight").
    pub name: &'static str,
    /// Whether this attribute is read-only.
    pub read_only: bool,
}

/// Per-node controller instance.
///
/// Each cgroup node holds `Arc<dyn CgroupController>` for each enabled controller.
/// The controller receives attribute names **without** the controller prefix
/// (e.g. "max" not "pids.max", "stat.periods" not "cpu.stat.periods").
pub trait CgroupController: Send + Sync {
    /// Controller name (e.g. "pids", "cpu").
    fn name(&self) -> &str;

    /// Whether this is a domain controller (affects process grouping).
    fn is_domain(&self) -> bool {
        false
    }

    /// Read an attribute value into `buf` at `offset`.
    /// Returns the number of bytes written to `buf`.
    fn read_attr(&self, name: &str, offset: usize, buf: &mut [u8]) -> VfsResult<usize>;

    /// Write `data` to an attribute.
    /// Returns the number of bytes consumed from `data`.
    fn write_attr(&self, name: &str, data: &[u8]) -> VfsResult<usize>;

    /// List of attributes this controller supports.
    fn attr_names(&self) -> &[AttrInfo];

    /// Downcast support for extracting concrete state (e.g. PidsState for fork fast path).
    fn as_any(&self) -> &dyn Any;
}

/// Global factory for creating per-node controller instances.
///
/// Factories are registered once during `init()` and stored in the global registry.
/// They provide metadata (name, attributes) without requiring an instance.
pub trait CgroupControllerFactory: Send + Sync {
    /// Controller name (e.g. "pids", "cpu").
    fn name(&self) -> &str;

    /// Whether this is a domain controller.
    fn is_domain(&self) -> bool {
        false
    }

    /// List of attributes this controller supports (for querying without an instance).
    fn attr_names(&self) -> &[AttrInfo];

    /// Create a new per-node controller instance.
    fn new_instance(&self) -> Arc<dyn CgroupController>;
}

/// Global factory registry: controller name → factory.
static FACTORY_REGISTRY: LazyInit<SpinNoIrq<BTreeMap<String, Arc<dyn CgroupControllerFactory>>>> =
    LazyInit::new();

/// Initialize the factory registry. Must be called before `register_factory`.
pub fn init_registry() {
    FACTORY_REGISTRY.init_once(SpinNoIrq::new(BTreeMap::new()));
}

/// Register a controller factory in the global registry.
pub fn register_factory(factory: Arc<dyn CgroupControllerFactory>) {
    if let Some(registry) = FACTORY_REGISTRY.get() {
        registry.lock().insert(factory.name().to_string(), factory);
    }
}

/// Get a factory by controller name.
pub fn get_factory(name: &str) -> Option<Arc<dyn CgroupControllerFactory>> {
    FACTORY_REGISTRY
        .get()
        .and_then(|registry| registry.lock().get(name).cloned())
}

/// List all registered controller names.
pub fn all_factory_names() -> Vec<String> {
    FACTORY_REGISTRY.get().map_or_else(Vec::new, |registry| {
        registry.lock().keys().cloned().collect()
    })
}

/// Parse a full attribute name into (controller_name, attr_name).
///
/// Uses `find('.')` to support multi-dot attribute names.
/// E.g. `"cpu.stat.periods"` → `("cpu", "stat.periods")`.
/// Returns `None` if there is no dot.
pub fn parse_attr_name(full_name: &str) -> Option<(&str, &str)> {
    full_name
        .find('.')
        .map(|pos| (&full_name[..pos], &full_name[pos + 1..]))
}

/// Helper: write a formatted string value into a buffer at the given offset.
/// Returns the number of bytes written.
pub fn write_to_buf(value: &str, offset: usize, buf: &mut [u8]) -> VfsResult<usize> {
    let bytes = value.as_bytes();
    if offset >= bytes.len() {
        return Ok(0);
    }
    let remaining = &bytes[offset..];
    let n = remaining.len().min(buf.len());
    buf[..n].copy_from_slice(&remaining[..n]);
    Ok(n)
}

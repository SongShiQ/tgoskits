//! Cgroup v2 controller framework.
//!
//! This module provides the core abstractions for cgroup controllers:
//! - [`SubControl`] trait: per-controller attribute read/write interface
//! - [`SubControlStatic`] trait: factory + type metadata for controllers
//! - [`SubController<T>`]: wrapper combining controller state with parent hierarchy
//! - [`Controller`]: composite holding the registered sub-controllers
//!
//! Design adapted from Asterinas cgroupfs with StarryOS-specific changes:
//! - `&[u8]` / `&mut [u8]` instead of `VmReader` / `VmWriter`
//! - `SpinNoIrq<T>` instead of `Rcu<T>` (StarryOS lacks RCU primitives)
//! - `ax_errno::{AxError, AxResult}` instead of `aster_systree::{Error, Result}`

pub mod cpu;
pub mod cpuset;
pub mod pids;

use alloc::string::{String, ToString};
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU8, Ordering};

use ax_errno::{AxError, AxResult};
use ax_kspin::SpinNoIrq;

use self::cpu::CpuController;
use self::cpuset::CpuSetController;
use self::pids::PidsController;

// ---------------------------------------------------------------------------
// SubControl trait (Task 1.2)
// ---------------------------------------------------------------------------

/// Per-controller attribute read/write interface.
///
/// Each controller implements this trait to expose its control files
/// (e.g. `pids.max`, `cpu.weight`). Attribute registration, ownership, and
/// permissions are described by [`ControllerDescriptor`] instead of probing.
pub trait SubControl {
    /// Read a controller attribute at the given offset into `buf`.
    ///
    /// Returns the number of bytes actually written to `buf`.
    fn read_attr_at(&self, name: &str, offset: usize, buf: &mut [u8]) -> AxResult<usize>;

    /// Write data to a controller attribute.
    ///
    /// Returns the number of bytes consumed from `data`.
    fn write_attr(&self, name: &str, data: &[u8]) -> AxResult<usize>;
}

// ---------------------------------------------------------------------------
// SubControlStatic trait (Task 1.3)
// ---------------------------------------------------------------------------

/// Factory + registry access for a concrete controller type.
///
/// Extends [`SubControl`] with construction and type-erased access.
pub trait SubControlStatic: SubControl + Sized + 'static {
    /// Create a new instance. `is_root` marks the root cgroup; `is_active`
    /// determines whether the controller is initially enabled.
    fn new(is_root: bool, is_active: bool) -> Self;

    /// Obtain the `Arc<SubController<Self>>` from a composite [`Controller`].
    fn read_from(controller: &Controller) -> Arc<SubController<Self>>;
}

type ControllerReadFn = fn(&Controller, &str, usize, &mut [u8]) -> AxResult<usize>;
type ControllerWriteFn = fn(&Controller, &str, &[u8]) -> AxResult<usize>;

fn read_subcontroller_attr<T: SubControlStatic>(
    controller: &Controller,
    name: &str,
    offset: usize,
    buf: &mut [u8],
) -> AxResult<usize> {
    T::read_from(controller).read_attr_at(name, offset, buf)
}

fn write_subcontroller_attr<T: SubControlStatic>(
    controller: &Controller,
    name: &str,
    data: &[u8],
) -> AxResult<usize> {
    T::read_from(controller).write_attr(name, data)
}

/// Static metadata for a controller attribute.
#[derive(Debug, Clone, Copy)]
pub struct AttrDescriptor {
    pub name: &'static str,
    pub read_only: bool,
}

/// Static metadata for a cgroup controller.
#[derive(Debug, Clone, Copy)]
pub struct ControllerDescriptor {
    pub ty: SubCtrlType,
    pub name: &'static str,
    pub attrs: &'static [AttrDescriptor],
    pub read_attr_at: ControllerReadFn,
    pub write_attr: ControllerWriteFn,
}

const PID_ATTRS: &[AttrDescriptor] = &[
    AttrDescriptor { name: "pids.max", read_only: false },
    AttrDescriptor { name: "pids.current", read_only: true },
    AttrDescriptor { name: "pids.peak", read_only: true },
];

const CPUSET_ATTRS: &[AttrDescriptor] = &[
    AttrDescriptor { name: "cpuset.cpus", read_only: true },
    AttrDescriptor { name: "cpuset.cpus.effective", read_only: true },
    AttrDescriptor { name: "cpuset.mems", read_only: true },
    AttrDescriptor { name: "cpuset.mems.effective", read_only: true },
];

const CPU_ATTRS: &[AttrDescriptor] = &[
    AttrDescriptor { name: "cpu.stat", read_only: true },
    AttrDescriptor { name: "cpu.weight", read_only: false },
    AttrDescriptor { name: "cpu.max", read_only: false },
];

pub const CONTROLLER_DESCRIPTORS: &[ControllerDescriptor] = &[
    ControllerDescriptor {
        ty: SubCtrlType::CpuSet,
        name: "cpuset",
        attrs: CPUSET_ATTRS,
        read_attr_at: read_subcontroller_attr::<CpuSetController>,
        write_attr: write_subcontroller_attr::<CpuSetController>,
    },
    ControllerDescriptor {
        ty: SubCtrlType::Cpu,
        name: "cpu",
        attrs: CPU_ATTRS,
        read_attr_at: read_subcontroller_attr::<CpuController>,
        write_attr: write_subcontroller_attr::<CpuController>,
    },
    ControllerDescriptor {
        ty: SubCtrlType::Pids,
        name: "pids",
        attrs: PID_ATTRS,
        read_attr_at: read_subcontroller_attr::<PidsController>,
        write_attr: write_subcontroller_attr::<PidsController>,
    },
];

pub fn controller_by_name(name: &str) -> Option<&'static ControllerDescriptor> {
    CONTROLLER_DESCRIPTORS.iter().find(|desc| desc.name == name)
}

pub fn attr_descriptor(name: &str) -> Option<(&'static ControllerDescriptor, &'static AttrDescriptor)> {
    CONTROLLER_DESCRIPTORS.iter().find_map(|controller| {
        controller
            .attrs
            .iter()
            .find(|attr| attr.name == name)
            .map(|attr| (controller, attr))
    })
}

// ---------------------------------------------------------------------------
// SubCtrlType enum (Task 1.6)
// ---------------------------------------------------------------------------

/// Identifies one registered cgroup v2 controller.
///
/// Each variant is assigned a power-of-two value for use as bit positions
/// in [`SubCtrlSet`].
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubCtrlType {
    /// `cpuset` — CPU and memory node affinity (read-only in StarryOS).
    CpuSet = 1,
    /// `cpu` — CPU bandwidth controller.
    Cpu = 2,
    /// `pids` — Process count limiting.
    Pids = 8,
}

impl SubCtrlType {
    /// Convert to the corresponding [`SubCtrlSet`] bit.
    pub fn to_set(self) -> SubCtrlSet {
        SubCtrlSet::from_bits_truncate(self as u8)
    }

    /// Whether this controller has cgroup v2 domain semantics.
    ///
    /// `pids` is intentionally treated as non-domain: it limits task counts
    /// but does not make the parent a resource distributor for children.
    pub const fn is_domain(self) -> bool {
        matches!(self, Self::CpuSet | Self::Cpu)
    }
}

// ---------------------------------------------------------------------------
// SubCtrlSet bitflags (Task 1.7)
// ---------------------------------------------------------------------------

bitflags::bitflags! {
    /// Bit-set of active cgroup controllers.
    ///
    /// Each bit corresponds to a [`SubCtrlType`] variant.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct SubCtrlSet: u8 {
        const CPUSET  = SubCtrlType::CpuSet as u8;
        const CPU     = SubCtrlType::Cpu as u8;
        const PIDS    = SubCtrlType::Pids as u8;
    }
}

impl SubCtrlSet {
    pub fn all_controllers() -> Self {
        CONTROLLER_DESCRIPTORS
            .iter()
            .fold(Self::empty(), |set, desc| set | desc.ty.to_set())
    }

    /// Controllers that make a non-root cgroup a domain distributor.
    pub fn domain_controllers() -> Self {
        CONTROLLER_DESCRIPTORS
            .iter()
            .filter(|desc| desc.ty.is_domain())
            .fold(Self::empty(), |set, desc| set | desc.ty.to_set())
    }

    pub fn contains_domain_controller(self) -> bool {
        self.intersects(Self::domain_controllers())
    }

    pub fn names_text(self) -> String {
        let mut names = alloc::vec::Vec::new();
        for desc in CONTROLLER_DESCRIPTORS {
            if self.contains(desc.ty.to_set()) {
                names.push(desc.name);
            }
        }
        names.join(" ")
    }
}

/// Atomic wrapper around [`SubCtrlSet`] for lock-free concurrent access.
///
/// Uses `AtomicU8` internally since `SubCtrlSet` is a `u8` bitflags.
#[derive(Debug)]
pub struct AtomicSubCtrlSet(AtomicU8);

impl AtomicSubCtrlSet {
    pub const fn empty() -> Self {
        Self(AtomicU8::new(0))
    }

    pub fn from(val: SubCtrlSet) -> Self {
        Self(AtomicU8::new(val.bits()))
    }

    pub fn load(&self, order: Ordering) -> SubCtrlSet {
        SubCtrlSet::from_bits_truncate(self.0.load(order))
    }

    pub fn store(&self, val: SubCtrlSet, order: Ordering) {
        self.0.store(val.bits(), order);
    }

    pub fn fetch_or(&self, val: SubCtrlSet, order: Ordering) -> SubCtrlSet {
        SubCtrlSet::from_bits_truncate(self.0.fetch_or(val.bits(), order))
    }

    pub fn fetch_and(&self, val: SubCtrlSet, order: Ordering) -> SubCtrlSet {
        SubCtrlSet::from_bits_truncate(self.0.fetch_and(val.bits(), order))
    }
}

// ---------------------------------------------------------------------------
// SubController<T> wrapper (Task 1.4)
// ---------------------------------------------------------------------------

/// Wrapper combining a controller's state with a reference to its parent.
///
/// `T` must implement [`SubControlStatic`]. The `inner` holds the actual
/// controller logic; `parent` links to the parent cgroup's sub-controller
/// for hierarchy-aware operations (e.g. `pids.max` inheritance).
pub struct SubController<T: SubControlStatic> {
    inner: Option<T>,
    parent: Option<Arc<SubController<T>>>,
}

impl<T: SubControlStatic> SubController<T> {
    /// Create a new sub-controller, optionally linking to the parent's.
    pub fn new(inner: Option<T>, parent: Option<Arc<SubController<T>>>) -> Self {
        Self { inner, parent }
    }

    /// Borrow the inner controller, if present.
    pub fn inner(&self) -> Option<&T> {
        self.inner.as_ref()
    }

    /// Borrow the parent sub-controller, if present.
    pub fn parent(&self) -> Option<&Arc<SubController<T>>> {
        self.parent.as_ref()
    }
}

impl<T: SubControlStatic> SubControl for SubController<T> {
    fn read_attr_at(&self, name: &str, offset: usize, buf: &mut [u8]) -> AxResult<usize> {
        match self.inner.as_ref() {
            Some(ctrl) => ctrl.read_attr_at(name, offset, buf),
            None => Err(AxError::NotFound),
        }
    }

    fn write_attr(&self, name: &str, data: &[u8]) -> AxResult<usize> {
        match self.inner.as_ref() {
            Some(ctrl) => ctrl.write_attr(name, data),
            None => Err(AxError::NotFound),
        }
    }
}

// ---------------------------------------------------------------------------
// Controller composite (Task 1.5)
// ---------------------------------------------------------------------------

/// Composite controller holding all registered sub-controllers.
///
/// Each cgroup node owns one `Controller`. Sub-controllers are stored as
/// `SpinNoIrq<Arc<SubController<T>>>` (StarryOS lacks RCU, so we use a
/// spinlock-guarded read path — acceptable given the low contention profile
/// of controller attribute access).
pub struct Controller {
    /// Bit-set tracking which controllers are currently active.
    active_set: AtomicSubCtrlSet,

    cpuset: SpinNoIrq<Arc<SubController<CpuSetController>>>,
    cpu: SpinNoIrq<Arc<SubController<CpuController>>>,
    pids: SpinNoIrq<Arc<SubController<PidsController>>>,
}

impl Controller {
    /// Create a new root or child controller, linking to the parent's hierarchy.
    pub fn new(parent: Option<&Arc<Controller>>) -> Self {
        let is_root = parent.is_none();

        // Inherit the parent's active_set so child cgroups see
        // controllers activated via subtree_control (fixes H-03).
        let inherited_active = parent
            .map(|p| p.active_set.load(Ordering::Acquire))
            .unwrap_or(SubCtrlSet::empty());

        // Link to parent's sub-controllers for hierarchy-aware charging.
        // MUST use pids()/cpuset()/cpu() to get the parent's own
        // SubController, NOT parent().cloned() which would get the grandparent.
        let parent_pids = parent.map(|p| p.pids());
        let parent_cpuset = parent.map(|p| p.cpuset());
        let parent_cpu = parent.map(|p| p.cpu());

        let cpuset_ctrl = CpuSetController::new(is_root, true);
        let cpu_ctrl = CpuController::new(is_root, true);
        let pids_ctrl = PidsController::new(is_root, true);

        let cpuset = Arc::new(SubController::new(Some(cpuset_ctrl), parent_cpuset));
        let cpu = Arc::new(SubController::new(Some(cpu_ctrl), parent_cpu));
        let pids = Arc::new(SubController::new(Some(pids_ctrl), parent_pids));

        Self {
            active_set: AtomicSubCtrlSet::from(inherited_active),
            cpuset: SpinNoIrq::new(cpuset),
            cpu: SpinNoIrq::new(cpu),
            pids: SpinNoIrq::new(pids),
        }
    }

    /// Returns the currently active controller set.
    pub fn active_set(&self) -> SubCtrlSet {
        self.active_set.load(Ordering::Acquire)
    }

    /// Activate a controller on this cgroup.
    ///
    /// Sets the corresponding bit in `active_set`.
    pub fn activate(&self, ctrl_type: SubCtrlType) {
        self.active_set.fetch_or(ctrl_type.to_set(), Ordering::AcqRel);
    }

    /// Deactivate a controller on this cgroup.
    ///
    /// Clears the corresponding bit in `active_set`.
    pub fn deactivate(&self, ctrl_type: SubCtrlType) {
        self.active_set.fetch_and(!ctrl_type.to_set(), Ordering::AcqRel);
    }

    /// Check if a named attribute is absent across all sub-controllers.
    ///
    /// Returns `true` only if **every** sub-controller reports the attribute
    /// as absent (mirrors Asterinas semantics).
    pub fn is_attr_absent(&self, name: &str) -> bool {
        Self::attr_owner(name).is_none()
    }

    /// Read a controller attribute through the descriptor registry.
    pub fn read_attr_at(
        &self,
        name: &str,
        offset: usize,
        buf: &mut [u8],
    ) -> AxResult<usize> {
        let (controller, _) = attr_descriptor(name).ok_or(AxError::NotFound)?;
        (controller.read_attr_at)(self, name, offset, buf)
    }

    /// Write a controller attribute through the descriptor registry.
    pub fn write_attr(&self, name: &str, data: &[u8]) -> AxResult<usize> {
        let (controller, _) = attr_descriptor(name).ok_or(AxError::NotFound)?;
        (controller.write_attr)(self, name, data)
    }

    /// Collect all attribute names from all sub-controllers.
    ///
    /// Used by CgroupNode to build a complete attribute set at init time.
    /// Follows Asterinas `init_attr_set()` / `SysAttrSetBuilder` pattern.
    pub fn attr_names(&self) -> alloc::vec::Vec<String> {
        CONTROLLER_DESCRIPTORS
            .iter()
            .flat_map(|controller| controller.attrs.iter().map(|attr| attr.name.to_string()))
            .collect()
    }

    pub fn attr_owner(name: &str) -> Option<SubCtrlType> {
        attr_descriptor(name).map(|(controller, _)| controller.ty)
    }

    pub fn attr_is_read_only(name: &str) -> Option<bool> {
        attr_descriptor(name).map(|(_, attr)| attr.read_only)
    }

    // -- Sub-controller accessors -------------------------------------------

    /// Borrow the pids sub-controller (lock and clone the Arc).
    pub fn pids(&self) -> Arc<SubController<PidsController>> {
        self.pids.lock().clone()
    }

    /// Borrow the cpuset sub-controller (lock and clone the Arc).
    pub fn cpuset(&self) -> Arc<SubController<CpuSetController>> {
        self.cpuset.lock().clone()
    }

    /// Borrow the cpu sub-controller (lock and clone the Arc).
    pub fn cpu(&self) -> Arc<SubController<CpuController>> {
        self.cpu.lock().clone()
    }
}

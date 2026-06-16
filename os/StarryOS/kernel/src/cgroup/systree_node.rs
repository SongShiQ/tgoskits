//! Cgroup system tree node and membership management.
//!
//! This module provides:
//! - [`CgroupMembership`]: global read-write lock protecting process-to-cgroup
//!   migration atomically
//! - [`CgroupSysNode`]: trait for cgroup nodes in the system tree
//! - [`with_process_cgroup_locked`]: helper ensuring correct lock ordering
//!
//! ## Lock Ordering Protocol (Task 1.9)
//!
//! To prevent deadlocks, all code must acquire locks in this order:
//!
//! 1. **CgroupMembership** (global, acquired first)
//!    - Read lock: for querying which cgroup a process belongs to
//!    - Write lock: for migrating processes between cgroups
//!
//! 2. **Parent node → Child node** (hierarchical ordering)
//!    - Always acquire parent's locks before child's locks
//!
//! 3. **Within a single node**: Inner Lock → Children Lock
//!    - The node's inner data lock before iterating children
//!
//! ### move_process_to_node lock sequence:
//! 1. CgroupMembership write lock
//! 2. new_cgroup controller lock
//! 3. new_cgroup inner lock
//! 4. old_cgroup inner lock
//!
//! Violating this order will eventually deadlock. The `with_process_cgroup_locked`
//! helper enforces the correct sequence for common migration operations.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::sync::{Arc, Weak};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};

use ax_errno::{AxError, AxResult};
use ax_kspin::SpinNoIrq;
use spin::{RwLock, RwLockReadGuard, RwLockWriteGuard};
use starry_process::Pid;

use super::CgroupId;
use super::controller::{Controller, SubCtrlSet, controller_by_name};

// ---------------------------------------------------------------------------
// CgroupMembership (Task 1.8)
// ---------------------------------------------------------------------------

/// Global lock protecting process-to-cgroup membership.
///
/// All process migration operations (moving a process from one cgroup to
/// another) must hold the write lock. Queries about which cgroup a process
/// belongs to should hold the read lock.
///
/// This is a singleton — the static `CGROUP_MEMBERSHIP` is the only instance.
pub struct CgroupMembership {
    _private: (),
}

/// Global singleton for cgroup membership lock.
static CGROUP_MEMBERSHIP: RwLock<CgroupMembership> =
    RwLock::new(CgroupMembership { _private: () });

impl CgroupMembership {
    /// Acquire a read lock for querying process cgroup membership.
    ///
    /// Multiple readers can hold the lock concurrently. Use this when
    /// you need to check which cgroup a process belongs to without
    /// modifying membership.
    pub fn read_lock() -> RwLockReadGuard<'static, CgroupMembership> {
        CGROUP_MEMBERSHIP.read()
    }

    /// Acquire a write lock for migrating processes between cgroups.
    ///
    /// Only one writer can hold the lock at a time. All migration
    /// operations (move_process_to_node, move_process_to_root) must
    /// hold this lock.
    pub fn write_lock() -> RwLockWriteGuard<'static, CgroupMembership> {
        CGROUP_MEMBERSHIP.write()
    }

    /// DEPRECATED: Use `cgroup::migrate_process()` instead.
    ///
    /// This method uses full-chain charge/uncharge which double-counts
    /// common ancestors. It does not update ProcessData.cgroup_id or
    /// process member lists. Kept for API compatibility only.
    #[deprecated(note = "Use cgroup::migrate_process() instead")]
    pub fn move_process_to_node(
        &mut self,
        _pid: Pid,
        _new_cgroup: &Controller,
        _old_cgroup: &Controller,
    ) -> AxResult<()> {
        Err(AxError::from(ax_errno::LinuxError::EOPNOTSUPP))
    }

    /// DEPRECATED: Use `cgroup::migrate_process()` instead.
    #[deprecated(note = "Use cgroup::migrate_process() instead")]
    pub fn move_process_to_root(
        &mut self,
        _pid: Pid,
        _root_controller: &Controller,
        _current_controller: &Controller,
    ) -> AxResult<()> {
        Err(AxError::from(ax_errno::LinuxError::EOPNOTSUPP))
    }

    /// Count the number of processes in a subtree.
    ///
    /// Traverses the cgroup tree starting from the given node and counts
    /// all processes in the node and its descendants.
    ///
    /// # Arguments
    /// - `get_children`: Function to get child cgroup controllers for a given controller
    /// - `root`: The root controller to start counting from
    ///
    /// # Lock Ordering
    /// Caller must hold CgroupMembership read lock.
    pub fn count_subtree_processes<F>(
        root: &Controller,
        get_children: F,
    ) -> usize
    where
        F: Fn(&Controller) -> Vec<Controller>,
    {
        let mut count = root.pids().inner().map_or(0, |p| p.current() as usize);

        for child in get_children(root) {
            count += Self::count_subtree_processes(&child, &get_children);
        }

        count
    }
}

// ---------------------------------------------------------------------------
// Process lookup helper
// ---------------------------------------------------------------------------

/// Find a process by PID. Delegates to the module-level helper.
fn find_process_by_pid(pid: Pid) -> AxResult<Arc<crate::task::ProcessData>> {
    super::find_process_by_pid(pid)
}

// ---------------------------------------------------------------------------
// CgroupSysNode trait (Task 1.11)
// ---------------------------------------------------------------------------

/// Trait for cgroup nodes in the system tree.
///
/// Every cgroup node (root or child) must implement this trait to provide
/// access to its controller and parent relationship.
pub trait CgroupSysNode {
    /// Get a reference to this node's composite controller.
    fn controller(&self) -> &Controller;

    /// Get the parent cgroup node, or `None` if this is the root.
    fn cgroup_parent(&self) -> Option<Arc<dyn CgroupSysNode>>;

    /// Get the depth of this node in the tree (root = 0).
    fn depth(&self) -> usize;

    /// Get the populated count (number of processes in this cgroup and descendants).
    fn populated_count(&self) -> usize;
}

// ---------------------------------------------------------------------------
// with_process_cgroup_locked helper (Task 1.10)
// ---------------------------------------------------------------------------

/// Execute a closure with proper lock ordering for process cgroup operations.
///
/// This helper ensures the correct lock sequence:
/// 1. Find the process (acquires process table lock briefly)
/// 2. Acquire CgroupMembership write lock
/// 3. Execute the provided closure with the process and membership lock
///
/// # Arguments
/// - `pid`: The process to operate on
/// - `op`: Closure that receives the process and membership guard
///
/// # Lock Ordering
/// The closure receives a `&mut CgroupMembership` (write-locked) and
/// can safely perform migration operations.
///
/// # Errors
/// - `ESRCH` if the process doesn't exist
/// - Any error returned by the closure
pub fn with_process_cgroup_locked<F, R>(pid: Pid, op: F) -> AxResult<R>
where
    F: FnOnce(&crate::task::ProcessData, &mut CgroupMembership) -> AxResult<R>,
{
    // First, verify the process exists (brief lock).
    let proc = find_process_by_pid(pid)?;

    // Then acquire the membership write lock and execute.
    let mut membership = CgroupMembership::write_lock();
    op(&proc, &mut membership)
}

// ---------------------------------------------------------------------------
// CgroupNode (Tasks 3.1-3.10)
// ---------------------------------------------------------------------------

/// Inner data protected by the node's RwLock.
///
/// Contains mutable state that changes during the cgroup's lifetime:
/// - `processes`: set of PIDs in this cgroup
/// - `dead`: whether the node has been marked as dead (removed from tree)
struct Inner {
    /// PIDs of processes in this cgroup.
    processes: Vec<Pid>,
    /// Whether this node is dead (removed from the tree).
    dead: bool,
}

/// A cgroup node in the system tree.
///
/// Each cgroup in the hierarchy is represented by a `CgroupNode`. The node
/// holds:
/// - A composite [`Controller`] for resource limits
/// - Inner mutable state (process list, dead flag)
/// - Parent/children relationships
/// - Depth in the tree and populated count
///
/// # Lock Ordering
/// When accessing multiple nodes, always lock parent before child.
/// Within a node: inner lock before children lock.
pub struct CgroupNode {
    /// Stable cgroup ID. Root = ROOT_ID (1), children assigned at registration.
    /// Eliminates pointer-to-ID conversion and the root ID mismatch problem.
    cgroup_id: AtomicU64,
    /// Node name (e.g., "my-cgroup").
    name: String,
    /// Full path from root (e.g., "/sys/fs/cgroup/my-cgroup").
    path: String,
    /// Parent node (None for root).
    parent: Option<Weak<CgroupNode>>,
    /// Child nodes, keyed by name.
    children: SpinNoIrq<BTreeMap<String, Arc<CgroupNode>>>,
    /// Composite controller for resource limits (shared via Arc for parent linking).
    controller: Arc<Controller>,
    /// Mutable inner state (process list, dead flag).
    inner: RwLock<Option<Inner>>,
    /// Depth in the tree (root = 0).
    depth: usize,
    /// Number of processes in this cgroup and its descendants.
    populated_count: AtomicUsize,
    /// Whether the populated event should be generated on next read.
    events: AtomicBool,
    /// Maximum descendant depth allowed below this node. `-1` means `max`.
    max_depth: AtomicI64,
    /// Controllers available at this node (inherited from parent's subtree_control).
    /// Used for `cgroup.controllers` — what this node CAN enable for children.
    available_controllers: super::controller::AtomicSubCtrlSet,
    /// Controllers enabled for child cgroups via `echo +X > cgroup.subtree_control`.
    /// Used for `cgroup.subtree_control` — what IS enabled for children.
    subtree_control: super::controller::AtomicSubCtrlSet,
    /// Complete set of attribute names (builtins + controller attrs).
    /// Built at node init time. Used for VFS lookup and read_dir.
    /// Follows Asterinas `SysAttrSetBuilder` / `init_attr_set()` pattern.
    attr_names: Vec<String>,
}

/// Collect attribute names for a cgroup node, filtered by available controllers.
///
/// Root node gets all controller attrs. Non-root nodes only get attrs
/// from controllers enabled in their `available_controllers` (inherited
/// from parent's subtree_control). This enforces cgroup v2 delegation:
/// a child cannot see or write controller attrs that the parent didn't enable.
fn collect_attr_names(
    controller: &Controller,
    available: super::controller::SubCtrlSet,
    is_root: bool,
) -> Vec<String> {
    let mut attrs = Vec::new();
    // Cgroup v2 builtin attributes (always present)
    attrs.extend([
        "cgroup.controllers",
        "cgroup.events",
        "cgroup.procs",
        "cgroup.subtree_control",
        "cgroup.max.depth",
    ].iter().map(|s| s.to_string()));

    // Controller-specific attributes — only if the controller is available.
    // Root always has all controllers available.
    for name in controller.attr_names() {
        let ctrl_available = Controller::attr_owner(&name)
            .map(|owner| is_root || available.contains(owner.to_set()))
            .unwrap_or(false);
        if ctrl_available {
            attrs.push(name);
        }
    }
    attrs
}

impl CgroupNode {
    /// Create a new root cgroup node.
    pub fn new_root() -> Arc<Self> {
        let controller = Arc::new(Controller::new(None));
        let all_controllers = SubCtrlSet::all_controllers();
        let attr_names = collect_attr_names(&controller, all_controllers, true);
        Arc::new(Self {
            cgroup_id: AtomicU64::new(super::ROOT_ID),
            name: String::new(),
            path: "/".to_string(),
            parent: None,
            children: SpinNoIrq::new(BTreeMap::new()),
            controller,
            inner: RwLock::new(Some(Inner {
                processes: Vec::new(),
                dead: false,
            })),
            depth: 0,
            populated_count: AtomicUsize::new(0),
            events: AtomicBool::new(false),
            max_depth: AtomicI64::new(-1),
            // Root has all controllers available, none enabled for subtree.
            available_controllers: super::controller::AtomicSubCtrlSet::from(
                SubCtrlSet::all_controllers(),
            ),
            subtree_control: super::controller::AtomicSubCtrlSet::empty(),
            attr_names,
        })
    }

    /// Create a new child cgroup node under the given parent.
    ///
    /// # Arguments
    /// - `name`: Name of the new cgroup
    /// - `parent`: Parent node
    ///
    /// # Returns
    /// The newly created child node.
    pub fn new_child(name: &str, parent: &Arc<CgroupNode>) -> Arc<Self> {
        let path = if parent.path == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", parent.path, name)
        };

        let controller = Arc::new(Controller::new(Some(&parent.controller)));
        let parent_subtree = parent.subtree_control.load(core::sync::atomic::Ordering::Acquire);
        let attr_names = collect_attr_names(&controller, parent_subtree, false);

        Arc::new(Self {
            cgroup_id: AtomicU64::new(0), // Set by register_node() after creation.
            name: name.to_string(),
            path,
            parent: Some(Arc::downgrade(parent)),
            children: SpinNoIrq::new(BTreeMap::new()),
            controller,
            inner: RwLock::new(Some(Inner {
                processes: Vec::new(),
                dead: false,
            })),
            depth: parent.depth + 1,
            populated_count: AtomicUsize::new(0),
            events: AtomicBool::new(false),
            max_depth: AtomicI64::new(-1),
            // Child inherits parent's subtree_control as its available controllers.
            available_controllers: super::controller::AtomicSubCtrlSet::from(
                parent.subtree_control.load(core::sync::atomic::Ordering::Acquire),
            ),
            subtree_control: super::controller::AtomicSubCtrlSet::empty(),
            attr_names,
        })
    }

    /// Get the stable cgroup ID.
    pub fn cgroup_id(&self) -> CgroupId {
        self.cgroup_id.load(Ordering::Acquire)
    }

    /// Set the cgroup ID (called by register_node after creation).
    pub(crate) fn set_cgroup_id(&self, id: CgroupId) {
        self.cgroup_id.store(id, Ordering::Release);
    }

    /// Get the node name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Get the full path.
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Get the depth in the tree.
    pub fn depth(&self) -> usize {
        self.depth
    }

    /// Whether this node has a domain controller enabled for child cgroups.
    pub fn domain_subtree_control_enabled(&self) -> bool {
        self.subtree_control
            .load(Ordering::Acquire)
            .contains_domain_controller()
    }

    /// Whether ordinary processes may be attached to this cgroup.
    pub fn can_host_process(&self) -> bool {
        self.depth == 0 || !self.domain_subtree_control_enabled()
    }

    /// Check all ancestor `cgroup.max.depth` limits before creating a child.
    pub fn check_child_depth_allowed(&self) -> AxResult<()> {
        let child_depth = self.depth + 1;
        self.check_descendant_depth_allowed(child_depth)?;
        let mut current = self.parent();
        while let Some(node) = current {
            node.check_descendant_depth_allowed(child_depth)?;
            current = node.parent();
        }
        Ok(())
    }

    fn check_descendant_depth_allowed(&self, descendant_depth: usize) -> AxResult<()> {
        let max_depth = self.max_depth.load(Ordering::Acquire);
        if max_depth >= 0 && descendant_depth.saturating_sub(self.depth) > max_depth as usize {
            return Err(AxError::from(ax_errno::LinuxError::EAGAIN));
        }
        Ok(())
    }

    pub fn max_depth_text(&self) -> String {
        let max_depth = self.max_depth.load(Ordering::Acquire);
        if max_depth < 0 {
            "max\n".to_string()
        } else {
            format!("{}\n", max_depth)
        }
    }

    pub fn write_max_depth(&self, data: &[u8]) -> AxResult<usize> {
        let _membership = CgroupMembership::write_lock();
        self.write_max_depth_locked(data)
    }

    fn write_max_depth_locked(&self, data: &[u8]) -> AxResult<usize> {
        let s = core::str::from_utf8(data).map_err(|_| AxError::InvalidInput)?.trim();
        let max_depth = if s == "max" {
            -1
        } else {
            let value = s.parse::<u64>().map_err(|_| AxError::InvalidInput)?;
            if value > i64::MAX as u64 {
                return Err(AxError::InvalidInput);
            }
            value as i64
        };
        if max_depth >= 0 && self.max_existing_descendant_distance() > max_depth as usize {
            return Err(AxError::from(ax_errno::LinuxError::EBUSY));
        }
        self.max_depth.store(max_depth, Ordering::Release);
        Ok(data.len())
    }

    fn max_existing_descendant_distance(&self) -> usize {
        let children: Vec<_> = self.children.lock().values().cloned().collect();
        children
            .into_iter()
            .map(|child| 1 + child.max_existing_descendant_distance())
            .max()
            .unwrap_or(0)
    }

    /// Check if the given name is a known attribute (builtin or controller).
    pub fn is_controller_attr(&self, name: &str) -> bool {
        self.attr_names.iter().any(|n| n == name)
    }

    /// Get all attribute names (for read_dir listing).
    pub fn all_attr_names(&self) -> &[String] {
        &self.attr_names
    }

    /// Get the populated count.
    pub fn populated_count(&self) -> usize {
        self.populated_count.load(Ordering::Relaxed)
    }

    /// Get a reference to the controller.
    pub fn controller(&self) -> &Arc<Controller> {
        &self.controller
    }

    /// Get the parent node, or None if this is the root.
    pub fn parent(&self) -> Option<Arc<CgroupNode>> {
        self.parent.as_ref().and_then(|w| w.upgrade())
    }

    /// Add a child node.
    ///
    /// # Arguments
    /// - `name`: Name of the child cgroup
    ///
    /// # Returns
    /// The newly created child node.
    ///
    /// # Errors
    /// - `AlreadyExists` if a child with this name exists
    pub fn add_child(self: &Arc<Self>, name: &str) -> AxResult<Arc<CgroupNode>> {
        let mut children = self.children.lock();

        if children.contains_key(name) {
            return Err(AxError::AlreadyExists);
        }

        let child = CgroupNode::new_child(name, self);
        children.insert(name.to_string(), child.clone());

        Ok(child)
    }

    /// Mark this node as dead.
    ///
    /// After calling this method, the node will no longer accept new
    /// processes and its inner data will be `None`.
    pub fn mark_as_dead(&self) {
        let mut inner = self.inner.write();
        if let Some(ref mut inner_data) = *inner {
            inner_data.dead = true;
        }
        *inner = None;
    }

    /// Check if this node is dead.
    pub fn is_dead(&self) -> bool {
        let inner = self.inner.read();
        inner.is_none()
    }

    /// Add a process to this cgroup.
    ///
    /// # Arguments
    /// - `pid`: Process ID to add
    ///
    /// # Errors
    /// - `NotFound` if the node is dead
    pub fn add_process(&self, pid: Pid) -> AxResult<()> {
        let mut inner = self.inner.write();
        match inner.as_mut() {
            Some(inner_data) => {
                if !inner_data.processes.contains(&pid) {
                    inner_data.processes.push(pid);
                }
                Ok(())
            }
            None => Err(AxError::NotFound),
        }
    }

    /// Remove a process from this cgroup.
    ///
    /// # Arguments
    /// - `pid`: Process ID to remove
    ///
    /// # Returns
    /// `true` if the process was found and removed, `false` otherwise.
    pub fn remove_process(&self, pid: Pid) -> bool {
        let mut inner = self.inner.write();
        match inner.as_mut() {
            Some(inner_data) => {
                let len_before = inner_data.processes.len();
                inner_data.processes.retain(|&p| p != pid);
                inner_data.processes.len() < len_before
            }
            None => false,
        }
    }

    /// Get the list of processes in this cgroup.
    pub fn process_list(&self) -> Vec<Pid> {
        let inner = self.inner.read();
        match inner.as_ref() {
            Some(inner_data) => inner_data.processes.clone(),
            None => Vec::new(),
        }
    }

    /// Get the number of processes in this cgroup.
    pub fn process_count(&self) -> usize {
        let inner = self.inner.read();
        match inner.as_ref() {
            Some(inner_data) => inner_data.processes.len(),
            None => 0,
        }
    }

    /// Check if this cgroup contains the given PID.
    pub fn process_contains(&self, pid: Pid) -> bool {
        let inner = self.inner.read();
        match inner.as_ref() {
            Some(inner_data) => inner_data.processes.contains(&pid),
            None => false,
        }
    }

    /// Get the list of child cgroup names.
    pub fn child_names(&self) -> Vec<String> {
        let children = self.children.lock();
        children.keys().cloned().collect()
    }

    /// Get a child cgroup by name.
    pub fn get_child(&self, name: &str) -> Option<Arc<CgroupNode>> {
        let children = self.children.lock();
        children.get(name).cloned()
    }

    /// Remove a child cgroup by name.
    ///
    /// # Arguments
    /// - `name`: Name of the child to remove
    ///
    /// # Returns
    /// `true` if the child was found and removed, `false` otherwise.
    pub fn remove_child(&self, name: &str) -> bool {
        let mut children = self.children.lock();
        children.remove(name).is_some()
    }

    /// Propagate populated count increment upward.
    ///
    /// Called when a process is added to this cgroup.
    pub fn propagate_add_populated(&self) {
        let old = self.populated_count.fetch_add(1, Ordering::Relaxed);
        if old == 0 {
            // 0→1 transition: cgroup became non-empty.
            self.events.store(true, Ordering::Relaxed);
        }

        // Propagate to parent.
        if let Some(parent) = self.parent() {
            parent.propagate_add_populated();
        }
    }

    /// Propagate populated count decrement upward.
    ///
    /// Called when a process is removed from this cgroup.
    pub fn propagate_sub_populated(&self) {
        let old = self.populated_count.load(Ordering::Relaxed);
        if old == 0 {
            return; // Already at zero, prevent underflow.
        }

        let old = self.populated_count.fetch_sub(1, Ordering::Relaxed);
        if old == 1 {
            // 1→0 transition: cgroup became empty.
            self.events.store(true, Ordering::Relaxed);
        }

        // Propagate to parent.
        if let Some(parent) = self.parent() {
            parent.propagate_sub_populated();
        }
    }

    /// Read an attribute from this cgroup node.
    ///
    /// Supported attributes:
    /// - `cgroup.controllers`: list of available controllers
    /// - `cgroup.events`: populated status events
    /// - `cgroup.procs`: list of process IDs
    /// - `cgroup.subtree_control`: list of active subtree controllers
    ///
    /// # Arguments
    /// - `name`: Attribute name
    /// - `offset`: Read offset
    /// - `buf`: Output buffer
    ///
    /// # Returns
    /// Number of bytes written to `buf`.
    /// Check if a controller attribute is available at this node.
    /// Root has all controllers; non-root checks available_controllers.
    fn is_ctrl_attr_available(&self, name: &str) -> bool {
        let is_root = self.depth == 0;
        if is_root {
            return true;
        }
        let available = self.available_controllers.load(core::sync::atomic::Ordering::Acquire);
        Controller::attr_owner(name)
            .map(|owner| available.contains(owner.to_set()))
            .unwrap_or(false)
    }

    pub fn read_attr_at(
        &self,
        name: &str,
        offset: usize,
        buf: &mut [u8],
    ) -> AxResult<usize> {
        let value = match name {
            "cgroup.controllers" => self.controller_list_text(),
            "cgroup.events" => self.events_text(),
            "cgroup.procs" => self.procs_text(),
            "cgroup.subtree_control" => self.subtree_control_text(),
            "cgroup.max.depth" => self.max_depth_text(),
            _ => {
                // Controller attrs — check availability before delegating.
                if !self.is_ctrl_attr_available(name) {
                    return Err(AxError::NotFound);
                }
                return self.controller.read_attr_at(name, offset, buf);
            }
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

    /// Write an attribute to this cgroup node.
    ///
    /// Supported attributes:
    /// - `cgroup.procs`: add a process to this cgroup
    /// - `cgroup.subtree_control`: enable/disable subtree controllers
    ///
    /// # Arguments
    /// - `name`: Attribute name
    /// - `data`: Input data
    ///
    /// # Returns
    /// Number of bytes consumed from `data`.
    pub fn write_attr(&self, name: &str, data: &[u8]) -> AxResult<usize> {
        match name {
            "cgroup.procs" => self.write_procs(data),
            "cgroup.subtree_control" => self.write_subtree_control(data),
            "cgroup.max.depth" => self.write_max_depth(data),
            _ => {
                // Controller attrs — check availability before delegating.
                if !self.is_ctrl_attr_available(name) {
                    return Err(AxError::NotFound);
                }
                self.controller.write_attr(name, data)
            }
        }
    }

    /// Get the controller list as text.
    pub fn controller_list_text(&self) -> String {
        // cgroup.controllers = what this node CAN enable for children.
        self.available_controllers
            .load(core::sync::atomic::Ordering::Acquire)
            .names_text()
    }

    /// Get the events text.
    pub fn events_text(&self) -> String {
        let populated = if self.populated_count() > 0 { 1 } else { 0 };
        let _events = if self.events.swap(false, Ordering::Relaxed) { 1 } else { 0 };
        format!("populated {}\nfrozen 0\n", populated)
    }

    /// Get the procs text (list of PIDs).
    pub fn procs_text(&self) -> String {
        let pids = self.process_list();
        let mut text = String::new();
        for pid in pids {
            let Ok(proc) = crate::task::get_process_data(pid) else {
                continue;
            };
            if !proc.is_cgroup_attached() || proc.proc.is_zombie() {
                continue;
            }
            use core::fmt::Write;
            let _ = writeln!(text, "{}", pid);
        }
        text
    }

    /// Get the subtree_control text — shows which controllers are
    /// enabled for child cgroups (separate from available_controllers).
    pub fn subtree_control_text(&self) -> String {
        self.subtree_control
            .load(core::sync::atomic::Ordering::Acquire)
            .names_text()
    }

    /// Write to cgroup.procs — migrate a process into this cgroup.
    ///
    /// Delegates to the authoritative `migrate_process()` in mod.rs.
    pub fn write_procs(&self, data: &[u8]) -> AxResult<usize> {
        let s = core::str::from_utf8(data).map_err(|_| AxError::InvalidInput)?;
        let s = s.trim();
        let pid: Pid = s.parse().map_err(|_| AxError::InvalidInput)?;

        // Verify the process exists.
        let _proc = find_process_by_pid(pid)?;

        // Use this node's own stable cgroup_id (Issue 1 fix).
        super::migrate_process(pid, self.cgroup_id())?;

        Ok(data.len())
    }

    /// Write to cgroup.subtree_control — enables/disables controllers for children.
    ///
    /// In cgroup v2, this modifies which controllers are available to child cgroups.
    /// Only controllers in `available_controllers` (inherited from parent) can be enabled.
    pub fn write_subtree_control(&self, data: &[u8]) -> AxResult<usize> {
        let _membership = CgroupMembership::write_lock();
        self.write_subtree_control_locked(data)
    }

    /// Write to cgroup.subtree_control with `CgroupMembership` write-locked.
    pub(crate) fn write_subtree_control_locked(&self, data: &[u8]) -> AxResult<usize> {
        let s = core::str::from_utf8(data).map_err(|_| AxError::InvalidInput)?;
        let s = s.trim();
        let available = self.available_controllers.load(core::sync::atomic::Ordering::Acquire);
        let mut next = self.subtree_control.load(core::sync::atomic::Ordering::Acquire);

        for part in s.split_whitespace() {
            if part.starts_with('+') {
                let ctrl_name = &part[1..];
                let flag = controller_by_name(ctrl_name)
                    .map(|desc| desc.ty.to_set())
                    .ok_or(AxError::InvalidInput)?;
                // Check that the controller is available at this node.
                if !available.contains(flag) {
                    return Err(AxError::InvalidInput);
                }
                next.insert(flag);
            } else if part.starts_with('-') {
                let ctrl_name = &part[1..];
                let flag = controller_by_name(ctrl_name)
                    .map(|desc| desc.ty.to_set())
                    .ok_or(AxError::InvalidInput)?;
                next.remove(flag);
            } else {
                return Err(AxError::InvalidInput);
            }
        }

        if self.depth != 0 && next.contains_domain_controller() && self.process_count() != 0 {
            return Err(AxError::from(ax_errno::LinuxError::EBUSY));
        }

        self.subtree_control.store(next, core::sync::atomic::Ordering::Release);
        Ok(data.len())
    }
}

impl CgroupSysNode for CgroupNode {
    fn controller(&self) -> &Controller {
        &self.controller
    }

    fn cgroup_parent(&self) -> Option<Arc<dyn CgroupSysNode>> {
        self.parent().map(|p| p as Arc<dyn CgroupSysNode>)
    }

    fn depth(&self) -> usize {
        self.depth
    }

    fn populated_count(&self) -> usize {
        self.populated_count.load(Ordering::Relaxed)
    }
}

// ---------------------------------------------------------------------------
// CgroupSystem singleton (Task 3.11)
// ---------------------------------------------------------------------------

/// Global cgroup system root.
///
/// This is the singleton that holds the root cgroup node. All cgroup
/// operations ultimately go through this root.
pub struct CgroupSystem {
    root: Arc<CgroupNode>,
}

impl CgroupSystem {
    /// Create a new CgroupSystem with a root cgroup.
    pub fn new() -> Self {
        Self {
            root: CgroupNode::new_root(),
        }
    }

    /// Get the root cgroup node.
    pub fn root(&self) -> &Arc<CgroupNode> {
        &self.root
    }

    /// Find a cgroup by path.
    ///
    /// # Arguments
    /// - `path`: Absolute path from root (e.g., "/my-cgroup/sub")
    ///
    /// # Returns
    /// The cgroup node at the given path, or `NotFound` if not found.
    pub fn find_by_path(&self, path: &str) -> AxResult<Arc<CgroupNode>> {
        if path == "/" {
            return Ok(self.root.clone());
        }

        let mut current = self.root.clone();
        for component in path.split('/').filter(|s| !s.is_empty()) {
            let child = current
                .get_child(component)
                .ok_or(AxError::NotFound)?;
            current = child;
        }

        Ok(current)
    }
}

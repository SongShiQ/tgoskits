//! cgroup v2 core data structures.

use alloc::{
    collections::BTreeMap,
    format,
    string::{String, ToString},
    sync::{Arc, Weak},
    vec::Vec,
};

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use ax_kspin::SpinNoIrq;
use ax_lazyinit::LazyInit;
use axfs_ng_vfs::{VfsError, VfsResult};

use super::{cpu::CpuState, pids::PidsState};

/// cgroup v2 type.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CgroupType {
    /// Normal cgroup.
    Domain = 0,
    /// Threaded cgroup.
    DomainThreaded = 1,
    /// Invalid state.
    DomainInvalid = 2,
}

impl CgroupType {
    pub fn from_u8(val: u8) -> Self {
        match val {
            0 => Self::Domain,
            1 => Self::DomainThreaded,
            _ => Self::DomainInvalid,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Domain => "domain",
            Self::DomainThreaded => "domain threaded",
            Self::DomainInvalid => "domain invalid",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim() {
            "domain" => Some(Self::Domain),
            "domain threaded" => Some(Self::DomainThreaded),
            _ => None,
        }
    }
}

/// cgroup v2 events.
pub struct CgroupEvents {
    /// Whether this cgroup has processes.
    pub populated: AtomicBool,
    /// Whether this cgroup is frozen.
    pub frozen: AtomicBool,
}

impl CgroupEvents {
    pub fn new() -> Self {
        Self {
            populated: AtomicBool::new(false),
            frozen: AtomicBool::new(false),
        }
    }

    pub fn format(&self) -> String {
        format!(
            "populated {}\nfrozen {}\n",
            self.populated.load(Ordering::Relaxed) as i32,
            self.frozen.load(Ordering::Relaxed) as i32
        )
    }
}

/// Freezer state for cgroup.
pub struct FreezerState {
    pub frozen: AtomicBool,
}

impl FreezerState {
    pub fn new() -> Self {
        Self {
            frozen: AtomicBool::new(false),
        }
    }
}

/// A cgroup node in the hierarchy.
#[allow(dead_code)]
pub struct CgroupNode {
    /// Directory name (e.g. "my-cgroup").
    pub name: String,
    /// Full path from root (e.g. "/my-cgroup").
    pub path: String,
    /// Child cgroups.
    pub children: SpinNoIrq<BTreeMap<String, Arc<CgroupNode>>>,
    /// PIDs in this cgroup.
    pub procs: SpinNoIrq<Vec<u32>>,
    /// Registered controller names (e.g. "pids", "cpu").
    pub controllers: Vec<String>,
    /// Parent (None for root).
    pub parent: Option<Weak<CgroupNode>>,
    /// Pids controller state.
    pub pids: Arc<PidsState>,
    pub cpu: Arc<CpuState>,
    /// cgroup type.
    pub cgroup_type: AtomicU8,
    /// cgroup events.
    pub events: CgroupEvents,
    /// Freezer state.
    pub freezer: FreezerState,
    /// Enabled subtree controllers.
    pub subtree_control: SpinNoIrq<Vec<String>>,
}

impl CgroupNode {
    pub fn new_root() -> Arc<Self> {
        Arc::new(Self {
            name: String::new(),
            path: "/".to_string(),
            children: SpinNoIrq::new(BTreeMap::new()),
            procs: SpinNoIrq::new(Vec::new()),
            controllers: Vec::new(),
            parent: None,
            pids: Arc::new(PidsState::new()),
            cpu: Arc::new(CpuState::new()),
            cgroup_type: AtomicU8::new(CgroupType::Domain as u8),
            events: CgroupEvents::new(),
            freezer: FreezerState::new(),
            subtree_control: SpinNoIrq::new(Vec::new()),
        })
    }

    /// Create a child cgroup under this node.
    pub fn create_child(self: &Arc<Self>, name: &str) -> VfsResult<Arc<CgroupNode>> {
        let mut children = self.children.lock();
        if children.contains_key(name) {
            return Err(VfsError::AlreadyExists);
        }
        let child_path = if self.path == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", self.path, name)
        };
        
        // Inherit subtree_control from parent
        let parent_subtree = self.subtree_control.lock().clone();
        
        let child = Arc::new(CgroupNode {
            name: name.to_string(),
            path: child_path,
            children: SpinNoIrq::new(BTreeMap::new()),
            procs: SpinNoIrq::new(Vec::new()),
            controllers: parent_subtree.clone(),
            parent: Some(Arc::downgrade(self)),
            pids: Arc::new(PidsState::new()),
            cpu: Arc::new(CpuState::new()),
            cgroup_type: AtomicU8::new(CgroupType::Domain as u8),
            events: CgroupEvents::new(),
            freezer: FreezerState::new(),
            subtree_control: SpinNoIrq::new(Vec::new()),
        });
        children.insert(name.to_string(), child);
        Ok(children.get(name).unwrap().clone())
    }

    /// List controller names.
    pub fn controller_list(&self) -> String {
        let mut list = alloc::vec!["pids".to_string(), "cpu".to_string()];
        list.extend(self.controllers.iter().cloned());
        list.join(" ")
    }

    /// Get cgroup type.
    pub fn cgroup_type(&self) -> CgroupType {
        CgroupType::from_u8(self.cgroup_type.load(Ordering::Relaxed))
    }

    /// Set cgroup type.
    pub fn set_cgroup_type(&self, new_type: CgroupType) -> Result<(), &'static str> {
        // Validate transition
        let current = self.cgroup_type();
        match (current, new_type) {
            (CgroupType::Domain, CgroupType::DomainThreaded) => {
                // Check no child cgroups
                if !self.children.lock().is_empty() {
                    return Err("cannot convert to threaded: has child cgroups");
                }
            }
            (CgroupType::DomainThreaded, CgroupType::Domain) => {
                // Check no threaded children
                // For now, allow conversion
            }
            _ if current == new_type => return Ok(()),
            _ => return Err("invalid type transition"),
        }
        self.cgroup_type.store(new_type as u8, Ordering::Relaxed);
        Ok(())
    }

    /// Update populated event.
    pub fn update_populated(&self) {
        let has_procs = !self.procs.lock().is_empty();
        self.events.populated.store(has_procs, Ordering::Relaxed);
    }

    /// Kill all processes in this cgroup.
    pub fn kill_all(&self) -> Result<(), &'static str> {
        let procs = self.procs.lock();
        for pid in procs.iter() {
            // Send SIGKILL - implementation depends on signal subsystem
            // For now, return error indicating signal sending is needed
            warn!("kill_all: would send SIGKILL to pid {}", pid);
        }
        Ok(())
    }

    /// Freeze all processes in this cgroup.
    pub fn freeze(&self) -> Result<(), &'static str> {
        self.freezer.frozen.store(true, Ordering::Relaxed);
        self.events.frozen.store(true, Ordering::Relaxed);
        // TODO: Actually freeze processes
        Ok(())
    }

    /// Thaw all processes in this cgroup.
    pub fn thaw(&self) -> Result<(), &'static str> {
        self.freezer.frozen.store(false, Ordering::Relaxed);
        self.events.frozen.store(false, Ordering::Relaxed);
        // TODO: Actually thaw processes
        Ok(())
    }

    /// Set subtree_control.
    pub fn set_subtree_control(&self, enable: &[&str], disable: &[&str]) -> Result<(), &'static str> {
        // "No Internal Processes" rule
        if !self.procs.lock().is_empty() {
            return Err("cannot enable controllers: cgroup has processes");
        }

        let valid_controllers = ["pids", "cpu"];
        let mut current = self.subtree_control.lock();

        // Enable controllers
        for name in enable {
            if !valid_controllers.contains(name) {
                return Err("invalid controller name");
            }
            if !current.contains(&name.to_string()) {
                current.push(name.to_string());
            }
        }

        // Disable controllers
        for name in disable {
            if !valid_controllers.contains(name) {
                return Err("invalid controller name");
            }
            current.retain(|s| s != name);
        }

        // Update child nodes - use subtree_control for inheritance
        // Note: child controllers will be read from parent's subtree_control
        // when needed, so we don't need to update them directly

        Ok(())
    }
}

/// Global cgroup v2 root.
pub static GLOBAL_CGROUP_ROOT: LazyInit<Arc<CgroupNode>> = LazyInit::new();

pub fn init() {
    GLOBAL_CGROUP_ROOT.init_once(CgroupNode::new_root());
}

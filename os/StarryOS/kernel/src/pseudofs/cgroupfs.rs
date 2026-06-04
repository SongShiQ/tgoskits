//! cgroup v2 pseudo-filesystem — `/cgroup/`.
//!
//! Provides file interfaces for cgroup.controllers, cgroup.procs,
//! cgroup.subtree_control, cgroup.type, pids.max, pids.current,
//! cpu.weight, cpu.max, cpu.stat.

use alloc::{borrow::Cow, boxed::Box, format, string::String, sync::Arc, vec::Vec};

use axfs_ng_vfs::{Filesystem, VfsResult};

use super::{
    DirMaker, NodeOpsMux, RwFile, SimpleDir, SimpleDirOps, SimpleFile, SimpleFileOperation,
    SimpleFs,
};
use crate::cgroup::CgroupId;

const CGROUP2_MAGIC: u32 = 0x63677270;

/// Create a new cgroup2 filesystem.
pub fn new_cgroupfs() -> Filesystem {
    SimpleFs::new_with("cgroup2".into(), CGROUP2_MAGIC, builder)
}

fn builder(fs: Arc<SimpleFs>) -> DirMaker {
    let root_id = crate::cgroup::root_id();
    build_cgroup_dir(fs, root_id)
}

fn build_cgroup_dir(fs: Arc<SimpleFs>, cgroup_id: CgroupId) -> DirMaker {
    let ops = CgroupDirOps::new(fs.clone(), cgroup_id);
    SimpleDir::new_maker(fs, Arc::new(ops))
}

struct CgroupDirOps {
    fs: Arc<SimpleFs>,
    cgroup_id: CgroupId,
}

impl CgroupDirOps {
    fn new(fs: Arc<SimpleFs>, cgroup_id: CgroupId) -> Self {
        Self { fs, cgroup_id }
    }
}

impl SimpleDirOps for CgroupDirOps {
    fn child_names<'a>(&'a self) -> Box<dyn Iterator<Item = Cow<'a, str>> + 'a> {
        let static_names = [
            "cgroup.controllers",
            "cgroup.subtree_control",
            "cgroup.type",
            "cgroup.procs",
            "pids.max",
            "pids.current",
            "cpu.weight",
            "cpu.max",
            "cpu.stat",
        ];
        let child_names: Vec<String> =
            crate::cgroup::child_names(self.cgroup_id).unwrap_or_default();
        Box::new(
            static_names
                .into_iter()
                .map(Cow::Borrowed)
                .chain(child_names.into_iter().map(Cow::Owned)),
        )
    }

    fn lookup_child(&self, name: &str) -> VfsResult<NodeOpsMux> {
        let fs = self.fs.clone();
        let id = self.cgroup_id;
        Ok(match name {
            "cgroup.controllers" => SimpleFile::new_regular(fs, move || {
                Ok(crate::cgroup::controllers_text(id)?.as_bytes().to_vec())
            })
            .into(),
            "cgroup.subtree_control" => SimpleFile::new_regular(fs, move || {
                Ok(crate::cgroup::subtree_control_text(id)?.as_bytes().to_vec())
            })
            .into(),
            "cgroup.type" => SimpleFile::new_regular(fs, || Ok(b"domain\n".to_vec())).into(),
            "cgroup.procs" => SimpleFile::new_regular(
                fs,
                RwFile::new(move |req| match req {
                    SimpleFileOperation::Read => {
                        let text = crate::cgroup::procs_text(id)?;
                        Ok(Some(text.into_bytes()))
                    }
                    SimpleFileOperation::Write(data) => {
                        crate::cgroup::write_procs(id, data)?;
                        Ok(None)
                    }
                }),
            )
            .into(),
            "pids.max" => SimpleFile::new_regular(
                fs,
                RwFile::new(move |req| match req {
                    SimpleFileOperation::Read => {
                        if let Some(pids) = crate::cgroup::get_pids_state(id) {
                            let max = pids.max.load(core::sync::atomic::Ordering::Relaxed);
                            if max < 0 {
                                Ok(Some(b"max\n".to_vec()))
                            } else {
                                Ok(Some(format!("{}\n", max).into_bytes()))
                            }
                        } else {
                            Ok(Some(b"max\n".to_vec()))
                        }
                    }
                    SimpleFileOperation::Write(data) => {
                        if let Some(pids) = crate::cgroup::get_pids_state(id) {
                            let s = core::str::from_utf8(data).unwrap_or("").trim();
                            if s == "max" {
                                pids.max.store(-1, core::sync::atomic::Ordering::Relaxed);
                            } else if let Ok(val) = s.parse::<i64>() {
                                pids.max.store(val, core::sync::atomic::Ordering::Relaxed);
                            }
                        }
                        Ok(None)
                    }
                }),
            )
            .into(),
            "pids.current" => SimpleFile::new_regular(fs, move || {
                if let Some(pids) = crate::cgroup::get_pids_state(id) {
                    let count = pids.current.load(core::sync::atomic::Ordering::Relaxed);
                    Ok(format!("{}\n", count).into_bytes())
                } else {
                    Ok(b"0\n".to_vec())
                }
            })
            .into(),
            "cpu.weight" => {
                SimpleFile::new_regular(
                    fs,
                    RwFile::new(move |req| match req {
                        SimpleFileOperation::Read => {
                            if let Some(bw) = crate::cgroup::get_bandwidth_state(id) {
                                // cpu.weight is stored in the CpuState, not BandwidthState
                                // For now return default
                                Ok(Some(b"100\n".to_vec()))
                            } else {
                                Ok(Some(b"100\n".to_vec()))
                            }
                        }
                        SimpleFileOperation::Write(_data) => {
                            // TODO: update cpu.weight in CpuState
                            Ok(None)
                        }
                    }),
                )
                .into()
            }
            "cpu.max" => {
                SimpleFile::new_regular(
                    fs,
                    RwFile::new(move |req| match req {
                        SimpleFileOperation::Read => {
                            if let Some(bw) = crate::cgroup::get_bandwidth_state(id) {
                                let quota = bw.quota.load(core::sync::atomic::Ordering::Relaxed);
                                let period = bw.period.load(core::sync::atomic::Ordering::Relaxed);
                                if quota < 0 {
                                    Ok(Some(format!("max {}\n", period).into_bytes()))
                                } else {
                                    Ok(Some(format!("{} {}\n", quota, period).into_bytes()))
                                }
                            } else {
                                Ok(Some(b"max 100000\n".to_vec()))
                            }
                        }
                        SimpleFileOperation::Write(data) => {
                            if let Some(bw) = crate::cgroup::get_bandwidth_state(id) {
                                let s = core::str::from_utf8(data).unwrap_or("").trim();
                                let parts: Vec<&str> = s.split_whitespace().collect();
                                if !parts.is_empty() {
                                    if parts[0] == "max" {
                                        bw.quota.store(-1, core::sync::atomic::Ordering::Relaxed);
                                    } else if let Ok(quota) = parts[0].parse::<i64>() {
                                        bw.quota
                                            .store(quota, core::sync::atomic::Ordering::Relaxed);
                                    }
                                }
                                if parts.len() > 1 {
                                    if let Ok(period) = parts[1].parse::<i64>() {
                                        bw.period
                                            .store(period, core::sync::atomic::Ordering::Relaxed);
                                    }
                                }
                                // Reset consumed on quota/period change
                                bw.consumed.store(0, core::sync::atomic::Ordering::Relaxed);
                                bw.period_start
                                    .store(0, core::sync::atomic::Ordering::Relaxed);
                            }
                            Ok(None)
                        }
                    }),
                )
                .into()
            }
            "cpu.stat" => SimpleFile::new_regular(fs, move || {
                if let Some(bw) = crate::cgroup::get_bandwidth_state(id) {
                    let nr_periods = bw.nr_periods.load(core::sync::atomic::Ordering::Relaxed);
                    let nr_throttled = bw.nr_throttled.load(core::sync::atomic::Ordering::Relaxed);
                    let throttled_usec = bw
                        .throttled_usec
                        .load(core::sync::atomic::Ordering::Relaxed);
                    Ok(format!(
                        "nr_periods {}\nnr_throttled {}\nthrottled_usec {}\n",
                        nr_periods, nr_throttled, throttled_usec
                    )
                    .into_bytes())
                } else {
                    Ok(b"nr_periods 0\nnr_throttled 0\nthrottled_usec 0\n".to_vec())
                }
            })
            .into(),
            _ => match crate::cgroup::lookup_child(id, name) {
                Ok(child_id) => NodeOpsMux::Dir(build_cgroup_dir(fs, child_id)),
                Err(_) => return Err(axfs_ng_vfs::VfsError::NotFound),
            },
        })
    }

    fn is_cacheable(&self) -> bool {
        false
    }

    fn create_dir(&self, name: &str) -> VfsResult<()> {
        crate::cgroup::create_child(self.cgroup_id, name)?;
        Ok(())
    }

    fn remove_dir(&self, name: &str) -> VfsResult<()> {
        crate::cgroup::remove_child(self.cgroup_id, name)?;
        Ok(())
    }
}

//! Unit tests for the cgroup v2 controller framework.

#[cfg(test)]
mod tests {
    use core::sync::atomic::Ordering;

    use crate::cgroup::controller::{
        AtomicSubCtrlSet, Controller, SubControl, SubControlStatic, SubCtrlSet, SubCtrlType,
    };
    use crate::cgroup::controller::pids::PidsController;
    use crate::cgroup::controller::cpuset::CpuSetController;
    use crate::cgroup::controller::cpu::CpuController;

    // -----------------------------------------------------------------------
    // SubCtrlType tests (Task 4.1)
    // -----------------------------------------------------------------------

    #[test]
    fn test_sub_ctrl_type_values() {
        assert_eq!(SubCtrlType::CpuSet as u8, 1);
        assert_eq!(SubCtrlType::Cpu as u8, 2);
        assert_eq!(SubCtrlType::Pids as u8, 8);
    }

    #[test]
    fn test_sub_ctrl_type_to_set() {
        assert_eq!(SubCtrlType::CpuSet.to_set(), SubCtrlSet::CPUSET);
        assert_eq!(SubCtrlType::Cpu.to_set(), SubCtrlSet::CPU);
        assert_eq!(SubCtrlType::Pids.to_set(), SubCtrlSet::PIDS);
    }

    // -----------------------------------------------------------------------
    // SubCtrlSet tests (Task 4.1)
    // -----------------------------------------------------------------------

    #[test]
    fn test_sub_ctrl_set_bits() {
        assert_eq!(SubCtrlSet::CPUSET.bits(), 1);
        assert_eq!(SubCtrlSet::CPU.bits(), 2);
        assert_eq!(SubCtrlSet::PIDS.bits(), 8);
    }

    #[test]
    fn test_sub_ctrl_set_contains() {
        let set = SubCtrlSet::CPUSET | SubCtrlSet::PIDS;
        assert!(set.contains(SubCtrlSet::CPUSET));
        assert!(!set.contains(SubCtrlSet::CPU));
        assert!(set.contains(SubCtrlSet::PIDS));
    }

    #[test]
    fn test_sub_ctrl_set_all() {
        let all = SubCtrlSet::all_controllers();
        assert_eq!(all, SubCtrlSet::CPUSET | SubCtrlSet::CPU | SubCtrlSet::PIDS);
        assert_eq!(all.names_text(), "cpuset cpu pids");
        assert!(crate::cgroup::controller::controller_by_name("memory").is_none());
    }

    // -----------------------------------------------------------------------
    // AtomicSubCtrlSet tests (Task 4.1)
    // -----------------------------------------------------------------------

    #[test]
    fn test_atomic_sub_ctrl_set_empty() {
        let set = AtomicSubCtrlSet::empty();
        assert_eq!(set.load(Ordering::Relaxed), SubCtrlSet::empty());
    }

    #[test]
    fn test_atomic_sub_ctrl_set_store_load() {
        let set = AtomicSubCtrlSet::empty();
        let val = SubCtrlSet::CPUSET | SubCtrlSet::PIDS;
        set.store(val, Ordering::Relaxed);
        assert_eq!(set.load(Ordering::Relaxed), val);
    }

    #[test]
    fn test_atomic_sub_ctrl_set_fetch_or() {
        let set = AtomicSubCtrlSet::empty();
        set.fetch_or(SubCtrlSet::CPUSET, Ordering::Relaxed);
        assert!(set.load(Ordering::Relaxed).contains(SubCtrlSet::CPUSET));

        set.fetch_or(SubCtrlSet::PIDS, Ordering::Relaxed);
        assert!(set.load(Ordering::Relaxed).contains(SubCtrlSet::CPUSET));
        assert!(set.load(Ordering::Relaxed).contains(SubCtrlSet::PIDS));
    }

    #[test]
    fn test_atomic_sub_ctrl_set_fetch_and() {
        let set = AtomicSubCtrlSet::empty();
        let val = SubCtrlSet::CPUSET | SubCtrlSet::PIDS;
        set.store(val, Ordering::Relaxed);

        set.fetch_and(SubCtrlSet::CPUSET, Ordering::Relaxed);
        assert!(set.load(Ordering::Relaxed).contains(SubCtrlSet::CPUSET));
        assert!(!set.load(Ordering::Relaxed).contains(SubCtrlSet::PIDS));
    }

    // -----------------------------------------------------------------------
    // PidsController tests (Task 4.4)
    // -----------------------------------------------------------------------

    #[test]
    fn test_pids_controller_new_root() {
        let ctrl = PidsController::new(true, true);
        assert_eq!(ctrl.current(), 0);
        assert_eq!(ctrl.peak(), 0);
        assert_eq!(ctrl.max(), None); // Unlimited for root
    }

    #[test]
    fn test_pids_controller_new_child() {
        let ctrl = PidsController::new(false, true);
        assert_eq!(ctrl.current(), 0);
        assert_eq!(ctrl.max(), None); // Default: unlimited (Linux cgroup v2)
    }

    #[test]
    fn test_pids_controller_set_max() {
        let ctrl = PidsController::new(true, true);
        ctrl.set_max(Some(100));
        assert_eq!(ctrl.max(), Some(100));

        ctrl.set_max(None);
        assert_eq!(ctrl.max(), None);
    }

    #[test]
    fn test_pids_controller_charge_uncharge() {
        let ctrl = PidsController::new(true, true);
        ctrl.charge_local();
        assert_eq!(ctrl.current(), 1);
        assert_eq!(ctrl.peak(), 1);

        ctrl.charge_local();
        assert_eq!(ctrl.current(), 2);
        assert_eq!(ctrl.peak(), 2);

        ctrl.uncharge_local();
        assert_eq!(ctrl.current(), 1);
        assert_eq!(ctrl.peak(), 2); // Peak doesn't decrease
    }

    #[test]
    fn test_pids_controller_try_charge_at_limit() {
        let ctrl = PidsController::new(false, true);
        ctrl.set_max(Some(1));

        assert!(ctrl.try_charge_local().is_ok());
        assert_eq!(ctrl.current(), 1);

        assert!(ctrl.try_charge_local().is_err()); // Should fail at limit
        assert_eq!(ctrl.current(), 1);
    }

    #[test]
    fn test_pids_controller_uncharge_underflow() {
        use crate::cgroup::controller::SubController;
        let ctrl = PidsController::new(true, true);
        let sub = SubController::new(Some(ctrl), None);
        PidsController::uncharge(&sub);
        assert_eq!(sub.inner().unwrap().current(), 0);
    }

    // -----------------------------------------------------------------------
    // SubControl trait tests (Task 4.1)
    // -----------------------------------------------------------------------

    #[test]
    fn test_pids_controller_subcontrol_absent() {
        assert_eq!(Controller::attr_owner("pids.max"), Some(SubCtrlType::Pids));
        assert_eq!(Controller::attr_owner("pids.current"), Some(SubCtrlType::Pids));
        assert_eq!(Controller::attr_owner("pids.peak"), Some(SubCtrlType::Pids));
        assert_ne!(Controller::attr_owner("cpu.weight"), Some(SubCtrlType::Pids));
    }

    #[test]
    fn test_pids_controller_read_attr() {
        let ctrl = PidsController::new(true, true);
        let mut buf = [0u8; 32];

        let n = ctrl.read_attr_at("pids.max", 0, &mut buf).unwrap();
        let s = core::str::from_utf8(&buf[..n]).unwrap();
        assert_eq!(s, "max\n");

        ctrl.set_max(Some(100));
        let n = ctrl.read_attr_at("pids.max", 0, &mut buf).unwrap();
        let s = core::str::from_utf8(&buf[..n]).unwrap();
        assert_eq!(s, "100\n");
    }

    #[test]
    fn test_pids_controller_write_attr() {
        let ctrl = PidsController::new(true, true);

        ctrl.write_attr("pids.max", b"100").unwrap();
        assert_eq!(ctrl.max(), Some(100));

        ctrl.write_attr("pids.max", b"max").unwrap();
        assert_eq!(ctrl.max(), None);
    }

    // -----------------------------------------------------------------------
    // CpuSetController tests (Task 4.5)
    // -----------------------------------------------------------------------

    #[test]
    fn test_cpuset_controller_new() {
        let _ctrl = CpuSetController::new(true, true);
        assert_eq!(Controller::attr_owner("cpuset.cpus"), Some(SubCtrlType::CpuSet));
        assert_eq!(Controller::attr_owner("cpuset.cpus.effective"), Some(SubCtrlType::CpuSet));
        assert_eq!(Controller::attr_owner("cpuset.mems"), Some(SubCtrlType::CpuSet));
        assert_eq!(Controller::attr_owner("cpuset.mems.effective"), Some(SubCtrlType::CpuSet));
        assert_ne!(Controller::attr_owner("pids.max"), Some(SubCtrlType::CpuSet));
    }

    #[test]
    fn test_cpuset_controller_read_attr() {
        let ctrl = CpuSetController::new(true, true);
        let mut buf = [0u8; 32];

        let n = ctrl.read_attr_at("cpuset.cpus.effective", 0, &mut buf).unwrap();
        let s = core::str::from_utf8(&buf[..n]).unwrap();
        assert_eq!(s, "0\n");

        let n = ctrl.read_attr_at("cpuset.mems.effective", 0, &mut buf).unwrap();
        let s = core::str::from_utf8(&buf[..n]).unwrap();
        assert_eq!(s, "0\n");
    }

    // -----------------------------------------------------------------------
    // CpuController tests (Task 4.2)
    // -----------------------------------------------------------------------

    #[test]
    fn test_cpu_controller_new() {
        let _ctrl = CpuController::new(true, true);
        assert_eq!(Controller::attr_owner("cpu.stat"), Some(SubCtrlType::Cpu));
        assert_eq!(Controller::attr_owner("cpu.weight"), Some(SubCtrlType::Cpu));
        assert_eq!(Controller::attr_owner("cpu.max"), Some(SubCtrlType::Cpu));
        assert_ne!(Controller::attr_owner("pids.max"), Some(SubCtrlType::Cpu));
    }

    #[test]
    fn test_cpu_controller_weight() {
        let ctrl = CpuController::new(true, true);
        let mut buf = [0u8; 32];

        let n = ctrl.read_attr_at("cpu.weight", 0, &mut buf).unwrap();
        let s = core::str::from_utf8(&buf[..n]).unwrap();
        assert_eq!(s, "100\n");

        ctrl.write_attr("cpu.weight", b"200").unwrap();
        let n = ctrl.read_attr_at("cpu.weight", 0, &mut buf).unwrap();
        let s = core::str::from_utf8(&buf[..n]).unwrap();
        assert_eq!(s, "200\n");
    }

    #[test]
    fn test_cpu_controller_max() {
        let ctrl = CpuController::new(true, true);
        let mut buf = [0u8; 32];

        let n = ctrl.read_attr_at("cpu.max", 0, &mut buf).unwrap();
        let s = core::str::from_utf8(&buf[..n]).unwrap();
        assert_eq!(s, "max 100000\n");

        ctrl.write_attr("cpu.max", b"50000 100000").unwrap();
        let n = ctrl.read_attr_at("cpu.max", 0, &mut buf).unwrap();
        let s = core::str::from_utf8(&buf[..n]).unwrap();
        assert_eq!(s, "50000 100000\n");

        assert!(ctrl.write_attr("cpu.max", b"-5 100000").is_err());
        assert!(ctrl.write_attr("cpu.max", b"0 100000").is_err());
        assert!(ctrl.write_attr("cpu.max", b"100 1").is_err());
        assert!(ctrl.write_attr("cpu.max", b"100 1000001").is_err());
    }

    // -----------------------------------------------------------------------
    // Controller tests (Task 4.2)
    // -----------------------------------------------------------------------

    #[test]
    fn test_controller_new() {
        let ctrl = Controller::new(None);
        let active = ctrl.active_set();
        assert_eq!(active, SubCtrlSet::empty());
    }

    #[test]
    fn test_controller_activate_deactivate() {
        let ctrl = Controller::new(None);

        ctrl.activate(SubCtrlType::Pids);
        assert!(ctrl.active_set().contains(SubCtrlSet::PIDS));

        ctrl.activate(SubCtrlType::Cpu);
        assert!(ctrl.active_set().contains(SubCtrlSet::CPU));

        ctrl.deactivate(SubCtrlType::Pids);
        assert!(!ctrl.active_set().contains(SubCtrlSet::PIDS));
        assert!(ctrl.active_set().contains(SubCtrlSet::CPU));
    }

    #[test]
    fn test_controller_read_attr() {
        let ctrl = Controller::new(None);
        let mut buf = [0u8; 32];

        let n = ctrl.read_attr_at("pids.max", 0, &mut buf).unwrap();
        let s = core::str::from_utf8(&buf[..n]).unwrap();
        assert_eq!(s, "max\n");

        let n = ctrl.read_attr_at("cpu.weight", 0, &mut buf).unwrap();
        let s = core::str::from_utf8(&buf[..n]).unwrap();
        assert_eq!(s, "100\n");
    }

    #[test]
    fn test_controller_write_attr() {
        let ctrl = Controller::new(None);

        ctrl.write_attr("pids.max", b"100").unwrap();
        let pids = ctrl.pids();
        assert_eq!(pids.inner().unwrap().max(), Some(100));

        ctrl.write_attr("cpu.weight", b"200").unwrap();
        let cpu = ctrl.cpu();
        assert_eq!(cpu.inner().unwrap().weight.load(Ordering::Relaxed), 200);
    }

    #[test]
    fn test_controller_is_attr_absent() {
        let ctrl = Controller::new(None);
        assert!(!ctrl.is_attr_absent("pids.max"));
        assert!(!ctrl.is_attr_absent("cpu.weight"));
        assert!(ctrl.is_attr_absent("memory.max"));
        assert!(ctrl.is_attr_absent("nonexistent.attr"));
    }
}

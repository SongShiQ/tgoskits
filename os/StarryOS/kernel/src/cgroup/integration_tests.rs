//! Integration tests for the cgroup v2 subsystem.

#[cfg(test)]
mod tests {
    use alloc::vec;

    use crate::cgroup::systree_node::{CgroupNode, CgroupSystem};

    // -----------------------------------------------------------------------
    // Integration test: cgroup creation/deletion (Task 4.8)
    // -----------------------------------------------------------------------

    #[test]
    fn test_cgroup_create_child() {
        let root = CgroupNode::new_root();
        let child = root.add_child("test-cgroup").unwrap();

        assert_eq!(child.name(), "test-cgroup");
        assert_eq!(child.depth(), 1);
        assert_eq!(child.path(), "/test-cgroup");
    }

    #[test]
    fn test_cgroup_create_duplicate() {
        let root = CgroupNode::new_root();
        let _child = root.add_child("test-cgroup").unwrap();
        let result = root.add_child("test-cgroup");

        assert!(result.is_err());
    }

    #[test]
    fn test_cgroup_remove_child() {
        let root = CgroupNode::new_root();
        let _child = root.add_child("test-cgroup").unwrap();

        assert!(root.remove_child("test-cgroup"));
        assert!(root.get_child("test-cgroup").is_none());
    }

    #[test]
    fn test_cgroup_remove_nonexistent() {
        let root = CgroupNode::new_root();
        assert!(!root.remove_child("nonexistent"));
    }

    #[test]
    fn test_cgroup_nested_creation() {
        let root = CgroupNode::new_root();
        let child = root.add_child("level1").unwrap();
        let grandchild = child.add_child("level2").unwrap();

        assert_eq!(grandchild.depth(), 2);
        assert_eq!(grandchild.path(), "/level1/level2");
    }

    // -----------------------------------------------------------------------
    // Integration test: process management (Task 4.9)
    // -----------------------------------------------------------------------

    #[test]
    fn test_cgroup_add_remove_process() {
        let root = CgroupNode::new_root();

        root.add_process(1).unwrap();
        assert_eq!(root.process_count(), 1);
        assert_eq!(root.process_list(), vec![1]);

        root.add_process(2).unwrap();
        assert_eq!(root.process_count(), 2);

        let removed = root.remove_process(1);
        assert!(removed);
        assert_eq!(root.process_count(), 1);
        assert_eq!(root.process_list(), vec![2]);
    }

    #[test]
    fn test_cgroup_remove_nonexistent_process() {
        let root = CgroupNode::new_root();
        let removed = root.remove_process(999);
        assert!(!removed);
    }

    // -----------------------------------------------------------------------
    // Integration test: populated count propagation (Task 4.9)
    // -----------------------------------------------------------------------

    #[test]
    fn test_populated_count_propagation() {
        let root = CgroupNode::new_root();
        let child = root.add_child("test").unwrap();

        assert_eq!(root.populated_count(), 0);
        assert_eq!(child.populated_count(), 0);

        // Add process to child — should propagate to root.
        child.add_process(1).unwrap();
        child.propagate_add_populated();

        assert_eq!(child.populated_count(), 1);
        assert_eq!(root.populated_count(), 1);

        // Add another process.
        child.add_process(2).unwrap();
        child.propagate_add_populated();

        assert_eq!(child.populated_count(), 2);
        assert_eq!(root.populated_count(), 2);

        // Remove a process — should propagate to root.
        child.propagate_sub_populated();

        assert_eq!(child.populated_count(), 1);
        assert_eq!(root.populated_count(), 1);

        // Remove last process.
        child.propagate_sub_populated();

        assert_eq!(child.populated_count(), 0);
        assert_eq!(root.populated_count(), 0);
    }

    // -----------------------------------------------------------------------
    // Integration test: dead node handling (Task 4.9)
    // -----------------------------------------------------------------------

    #[test]
    fn test_dead_node_rejects_process() {
        let root = CgroupNode::new_root();
        root.mark_as_dead();

        assert!(root.is_dead());
        assert!(root.add_process(1).is_err());
    }

    // -----------------------------------------------------------------------
    // Integration test: attribute reading/writing (Task 4.8)
    // -----------------------------------------------------------------------

    #[test]
    fn test_read_cgroup_procs() {
        let root = CgroupNode::new_root();
        root.add_process(1).unwrap();
        root.add_process(2).unwrap();

        let mut buf = [0u8; 32];
        let n = root.read_attr_at("cgroup.procs", 0, &mut buf).unwrap();
        let s = core::str::from_utf8(&buf[..n]).unwrap();
        assert!(s.contains("1"));
        assert!(s.contains("2"));
    }

    #[test]
    fn test_read_cgroup_controllers() {
        let root = CgroupNode::new_root();
        let mut buf = [0u8; 64];
        let n = root.read_attr_at("cgroup.controllers", 0, &mut buf).unwrap();
        let s = core::str::from_utf8(&buf[..n]).unwrap();
        assert_eq!(s, "cpuset cpu pids");
        assert!(!s.split_whitespace().any(|name| name == "memory"));
    }

    #[test]
    fn test_write_subtree_control() {
        let root = CgroupNode::new_root();

        root.write_attr("cgroup.subtree_control", b"+pids").unwrap();
        assert_eq!(root.subtree_control_text(), "pids");

        root.write_attr("cgroup.subtree_control", b"+cpu -pids").unwrap();
        assert_eq!(root.subtree_control_text(), "cpu");

        assert!(root.write_attr("cgroup.subtree_control", b"+memory").is_err());
        assert_eq!(root.subtree_control_text(), "cpu");
    }

    // -----------------------------------------------------------------------
    // CgroupSystem tests (Task 4.8)
    // -----------------------------------------------------------------------

    #[test]
    fn test_cgroup_system_root() {
        let system = CgroupSystem::new();
        let root = system.root();

        assert_eq!(root.depth(), 0);
        assert_eq!(root.path(), "/");
    }

    #[test]
    fn test_cgroup_system_find_by_path() {
        let system = CgroupSystem::new();
        let root = system.root();
        let _child = root.add_child("test").unwrap();

        let found = system.find_by_path("/test").unwrap();
        assert_eq!(found.name(), "test");

        let root_found = system.find_by_path("/").unwrap();
        assert_eq!(root_found.depth(), 0);
    }

    #[test]
    fn test_cgroup_system_find_nonexistent() {
        let system = CgroupSystem::new();
        let result = system.find_by_path("/nonexistent");
        assert!(result.is_err());
    }
}

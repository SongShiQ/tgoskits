<h1 align="center">ax-cgroup</h1>

<p align="center">StarryOS 的 cgroup v2 子系统</p>

<div align="center">

[![Crates.io](https://img.shields.io/crates/v/ax-cgroup.svg)](https://crates.io/crates/ax-cgroup)
[![Docs.rs](https://docs.rs/ax-cgroup/badge.svg)](https://docs.rs/ax-cgroup)
[![Rust](https://img.shields.io/badge/edition-2021-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](./LICENSE)

</div>

[English](README.md) | 中文

# 介绍

`ax-cgroup` 为 StarryOS 提供一个与内核解耦的 cgroup v2 子系统。它负责维护
cgroup 层次结构、各控制器状态以及进程成员关系。该 crate 是 `no_std` 的，
不直接依赖内核任务层：内核通过实现 [`CgroupProvider`] trait 并在启动时注册，
来提供任务/进程相关的原语。

本 crate 是 TGOSKits 组件集合的一部分，可用于集成 ArceOS、StarryOS 及相关
底层系统软件的 Rust 项目。

## 设计

实现遵循 Linux cgroup v2 语义，并借鉴了
[Asterinas](https://github.com/asterinas/asterinas) cgroupfs 的若干思路
（domain 控制器规则、全局成员关系串行化、`subtree_control` 传播）。但它
**不是** Asterinas 基于 `SysTree` 架构的移植：StarryOS 没有 `aster_systree`
组件，使用的是 `axfs-ng-vfs`，因此这里的层次结构是一棵自管理的树，而非
`SysBranchNode` 图。

与 Asterinas 的具体差异：

| 方面       | Asterinas                              | ax-cgroup                                        |
| ---------- | -------------------------------------- | ------------------------------------------------ |
| 层次框架   | `SysTree`（`SysBranchNode` / `SysObj`） | 自管理的 `BTreeMap<String, Arc<CgroupNode>>`     |
| 控制器访问 | `Controller` + `SubControl` trait       | 基于注册表的 `BTreeMap<String, Arc<dyn CgroupController>>` |
| 属性读写   | trait 方法分发                          | 通过控制器实例进行 trait 方法分发                |
| 成员关系锁 | `CgroupMembership` 全局 `Mutex`         | `SpinNoIrq<MembershipState>`（`LazyInit`）       |
| 文件系统   | 基于 `SysTree` 的自定义 cgroupfs        | 内核侧的 `axfs-ng-vfs` 适配                       |

### 模块划分

| 模块          | 职责                                                         |
| ------------- | ------------------------------------------------------------ |
| `controller`  | `CgroupController` / `CgroupControllerFactory` trait、工厂注册表、`parse_attr_name`。 |
| `core`        | `CgroupNode`、全局根节点，以及 id 到节点的注册表。           |
| `pids`        | `PidsState` + `PidsController` —— 基于 CAS 充值路径的进程数计量。 |
| `cpu`         | `CpuState` + `CpuController` / `BandwidthState` —— `cpu.weight` 与 `cpu.max` 状态。 |
| `provider`    | `CgroupProvider` trait 与注册单元。                          |
| crate 根      | 成员关系、fork/migrate/exit 事务，以及属性分发。            |

### 控制器

控制器采用 **工厂 + 实例** 模式：

- **工厂**（`CgroupControllerFactory`）：在 `init()` 时全局注册。报告控制器
  名称、属性元数据，并创建每节点实例。
- **实例**（`CgroupController`）：存储在 `CgroupNode.controllers` 中，负责
  处理该控制器属性的 `read_attr` / `write_attr`。属性名使用**短名**（不含
  控制器前缀）：例如 `"max"` 而非 `"pids.max"`。

内置了两个控制器：

- **pids** —— `pids.max` / `pids.current`。充值会沿路径回溯到根节点，失败时
  回滚已充值部分；每个节点的计数器使用 CAS 循环，以避免 SMP 上的 TOCTOU 竞态。
  `PidsState` 也直接存储在 `CgroupNode.pids` 上，用于无锁的 fork 快速路径。
- **cpu** —— `cpu.weight`、`cpu.max`（quota/period）与 `cpu.stat`。带宽
  quota/period 状态在此维护；定时器 tick 的限流执行钩子位于内核侧，因为它
  需要访问 `ax_task` / `ax_hal`。

### 新增控制器

1. 定义状态结构体（如 `MemoryState`）和控制器结构体（`MemoryController`），
   实现 `CgroupController` trait。
2. 定义工厂结构体（`MemoryControllerFactory`），实现
   `CgroupControllerFactory` trait。
3. 在 `init()` 中注册工厂：
   ```rust
   controller::register_factory(Arc::new(MemoryControllerFactory));
   ```
4. `subtree_control` 机制、属性分发和接口文件检测均通过注册表自动完成。

## 快速开始

### 添加依赖

在 `Cargo.toml` 中加入：

```toml
[dependencies]
ax-cgroup = "0.1.0"
```

### 使用方式

```rust,ignore
use alloc::sync::Arc;
use ax_cgroup::{CgroupNode, CgroupProvider};

struct KernelProvider;

impl CgroupProvider for KernelProvider {
    fn is_zombie(&self, pid: u32) -> bool {
        // 查询内核进程表
        # false
    }
    fn get_cgroup(&self, pid: u32) -> Option<Arc<CgroupNode>> {
        // 返回该进程当前所属的 cgroup
        # None
    }
    fn set_cgroup(&self, pid: u32, cgroup: Arc<CgroupNode>) {
        // 保存该进程新的 cgroup
    }
}

static PROVIDER: KernelProvider = KernelProvider;

fn boot() {
    // init() 注册内置控制器工厂并创建根节点
    ax_cgroup::init();
    ax_cgroup::register_provider(&PROVIDER);
}
```

### 检查与测试

```bash
# 进入 crate 目录
cd components/ax-cgroup

# 代码格式化
cargo fmt --all

# 运行 clippy
cargo clippy --all-targets --all-features

# 生成文档
cargo doc --no-deps
```

# 贡献

1. Fork 仓库并创建分支
2. 在本地运行格式化与检查
3. 运行与该 crate 相关的测试
4. 提交 PR 并确保 CI 通过

# 许可证

本项目采用 Apache License 2.0 许可证。详情见 [LICENSE](../../LICENSE)。

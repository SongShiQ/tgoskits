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
| 控制器访问 | `Controller` + `SubControl` trait       | 动态注册表（工厂/实例模式）                      |
| 属性读写   | trait 方法分发                          | 通过 `CgroupController` trait 的注册表分发        |
| 成员关系锁 | `CgroupMembership` 全局 `Mutex`         | `SpinNoIrq<MembershipState>`（`LazyInit`）       |
| 文件系统   | 基于 `SysTree` 的自定义 cgroupfs        | 内核侧的 `axfs-ng-vfs` 适配                       |

### 控制器注册表

cgroup 子系统使用**工厂/实例注册表模式**支持动态控制器注册。这使得添加新控制器时无需修改 `lib.rs` 中的中心分发代码。

#### 架构

```
全局注册表（启动时一次性）
  ↓
CgroupControllerFactory (name, attr_names, new_instance)
  ↓
每个节点的实例
  ↓
CgroupController (read_attr, write_attr)
```

#### 添加新控制器

添加新控制器（例如 `memory`）的步骤：

**步骤 1：创建控制器文件** (`src/memory.rs`)：

```rust
use alloc::{format, sync::Arc};
use core::sync::atomic::{AtomicI64, Ordering};
use axfs_ng_vfs::{VfsError, VfsResult};
use super::controller::{AttrInfo, CgroupController, CgroupControllerFactory, write_to_buf};

// 定义状态结构
pub struct MemoryState {
    pub current: AtomicI64,
    pub max: AtomicI64,
}

impl MemoryState {
    pub fn new() -> Self {
        Self {
            current: AtomicI64::new(0),
            max: AtomicI64::new(-1),
        }
    }
}

// 定义控制器实例
const MEMORY_ATTRS: &[AttrInfo] = &[
    AttrInfo { name: "current", read_only: true },
    AttrInfo { name: "max", read_only: false },
];

pub struct MemoryController {
    state: Arc<MemoryState>,
}

impl MemoryController {
    pub fn new(state: Arc<MemoryState>) -> Self {
        Self { state }
    }
}

impl CgroupController for MemoryController {
    fn name(&self) -> &str { "memory" }
    
    fn read_attr(&self, name: &str, offset: usize, buf: &mut [u8]) -> VfsResult<usize> {
        let value = match name {
            "current" => format!("{}\n", self.state.current.load(Ordering::Acquire)),
            "max" => {
                let max = self.state.max.load(Ordering::Acquire);
                if max < 0 { "max\n".to_string() } else { format!("{}\n", max) }
            }
            _ => return Err(VfsError::NotFound),
        };
        write_to_buf(&value, offset, buf)
    }
    
    fn write_attr(&self, name: &str, data: &[u8]) -> VfsResult<usize> {
        let text = core::str::from_utf8(data)
            .map_err(|_| VfsError::InvalidInput)?
            .trim();
        match name {
            "max" => {
                let value = if text == "max" {
                    -1
                } else {
                    text.parse::<i64>().map_err(|_| VfsError::InvalidInput)?
                };
                self.state.max.store(value, Ordering::Release);
                Ok(data.len())
            }
            "current" => Err(VfsError::OperationNotPermitted),
            _ => Err(VfsError::NotFound),
        }
    }
    
    fn attr_names(&self) -> &[AttrInfo] { MEMORY_ATTRS }
    fn as_any(&self) -> &dyn core::any::Any { self }
}

// 定义工厂
pub struct MemoryControllerFactory;

impl CgroupControllerFactory for MemoryControllerFactory {
    fn name(&self) -> &str { "memory" }
    fn attr_names(&self) -> &[AttrInfo] { MEMORY_ATTRS }
    fn new_instance(&self) -> Arc<dyn CgroupController> {
        Arc::new(MemoryController {
            state: Arc::new(MemoryState::new()),
        })
    }
}
```

**步骤 2：注册工厂** 在 `src/lib.rs` 中：

```rust
pub mod memory;

pub fn init() {
    // ...
    controller::register_factory(Arc::new(pids::PidsControllerFactory));
    controller::register_factory(Arc::new(cpu::CpuControllerFactory));
    controller::register_factory(Arc::new(memory::MemoryControllerFactory));  // 添加这一行
}
```

**步骤 3：完成！** 无需修改 `lib.rs` 中的中心分发代码。

#### 设计原则

1. **工厂/实例分离**：`CgroupControllerFactory` 全局注册，`CgroupController` 是每个节点的实例
2. **Arc 生命周期管理**：使用 `Arc` 而非 `&'static` 以获得灵活性
3. **多点属性名**：通过 `parse_attr_name()` 支持 `cpu.stat.periods` 格式
4. **控制器名约束**：控制器名不能包含 `.` 字符
5. **不可变控制器映射**：`CgroupNode` 中的 `controllers` map 在节点创建后不可变（无锁读取路径）

### 模块划分

| 模块       | 职责                                                         |
| ---------- | ------------------------------------------------------------ |
| `controller` | `CgroupController` / `CgroupControllerFactory` trait 和全局注册表。 |
| `core`     | `CgroupNode`、全局根节点，以及 id 到节点的注册表。           |
| `pids`     | `PidsState` / `PidsController` —— 基于 CAS 充值路径的进程数计量。 |
| `cpu`      | `CpuState` / `CpuController` —— `cpu.weight` 与 `cpu.max` 状态。 |
| `provider` | `CgroupProvider` trait 与注册单元。                          |
| crate 根   | 成员关系、fork/migrate/exit 事务，以及属性解析。            |

### 支持的控制器

实现了五个控制器：

#### pids
- **属性**：`pids.max`（读写）、`pids.current`（只读）
- **用途**：限制 cgroup 中的进程数量
- **实现**：充值会沿路径回溯到根节点，失败时回滚已充值部分；每个节点的计数器使用 CAS 循环，以避免 SMP 上的 TOCTOU 竞态
- **快速路径**：`CgroupNode.pids` 字段提供直接访问，使 `begin_fork()` 避免注册表查找开销

#### cpu
- **属性**：
  - `cpu.weight`（读写）—— CPU 权重（1-10000，默认 100）
  - `cpu.max`（读写）—— CPU 带宽限制（微秒，格式：`"QUOTA PERIOD"` 或 `"max PERIOD"`）
  - `cpu.stat`（只读）—— CPU 使用统计（`nr_periods`、`nr_throttled`、`throttled_usec`）
- **用途**：控制 CPU 时间分配和带宽限流
- **实现**：带宽 quota/period 状态在此维护；定时器 tick 的限流执行钩子位于内核侧，因为它需要访问 `ax_task` / `ax_hal`
- **Domain 控制器**：返回 `is_domain() = true`（影响进程托管规则）

#### cpuset
- **属性**：
  - `cpuset.cpus`（读写）—— 允许的 CPU 列表（格式：`"0-3,5,7"` 或 `"0,1,2"`）
  - `cpuset.mems`（读写）—— 允许的内存节点列表（格式同 cpus）
  - `cpuset.cpus.effective`（只读）—— 经过层次结构交集后的有效 CPU 掩码
  - `cpuset.mems.effective`（只读）—— 经过层次结构交集后的有效内存节点掩码
- **用途**：限制进程只能在特定 CPU 和内存节点上运行
- **实现**：使用 64 位位图表示 CPU/内存掩码；子节点创建时继承父节点掩码（子集约束）
- **Domain 控制器**：返回 `is_domain() = true`（影响进程托管规则）
- **格式**：支持范围表示法（`0-3`）和逗号分隔列表（`0,2,4`）

#### memory
- **属性**：
  - `memory.current`（只读）—— 当前内存使用量（字节）
  - `memory.max`（读写）—— 硬内存限制（格式：`"1G"`、`"512M"`、`"1048576"` 或 `"max"`）
  - `memory.high`（读写）—— 内存回收压力的高水位线（格式同 max）
  - `memory.low`（读写）—— 内存保护阈值（字节，0 = 无保护）
  - `memory.min`（读写）—— 硬内存保护（字节，0 = 无保护）
  - `memory.events`（只读）—— 事件计数器（`max`、`high`、`oom`）
- **用途**：限制和跟踪内存使用
- **实现**：支持人类可读的大小后缀（K、M、G、T）；提供 charge/uncharge 方法用于内存计费
- **Domain 控制器**：返回 `is_domain() = true`（影响进程托管规则）
- **格式**：接受数字字节和大小后缀（不区分大小写）

#### io
- **属性**：
  - `io.weight`（读写）—— 默认 I/O 权重（1-10000，默认 100）
  - `io.max`（读写）—— 按设备的 I/O 限制（格式：`"主设备号:次设备号 rbps=N wbps=N riops=N wiops=N"`）
  - `io.stat`（只读）—— 按设备的 I/O 统计（格式：`"主设备号:次设备号 rbytes=N wbytes=N rios=N wios=N"`）
- **用途**：控制块设备的 I/O 带宽和优先级
- **实现**：按设备的限制和统计；同时支持带宽（bps）和 IOPS 限制
- **Domain 控制器**：返回 `is_domain() = true`（影响进程托管规则）
- **格式**：设备指定为 `主设备号:次设备号`（例如 `8:0` 表示 `/dev/sda`）；`max` 表示无限制

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

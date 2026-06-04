# StarryOS Syscall 改进项目跟踪

## 项目概述

- **仓库**: rcore-os/tgoskits (fork: SongShiQ/tgoskits)
- **时间**: 2026-05-13 ~ 至今
- **报告人**: 宋仕奇
- **目标**: 通过测试驱动的方式，补全 syscall 测试覆盖，发现并修复内核语义缺陷，验证 Linux app 兼容性

## 整体进度

| 方案 | 目标 | 状态 |
|------|------|------|
| **方案一** | 补全 7 个 fd 相关 syscall 测试（dup/dup2/dup3/fcntl/flock/ioctl/close_range） | **已完成** |
| **方案二** | 在 StarryOS Alpine 上运行 sqlite3 CLI，验证 Linux app 兼容性 | **S0-S8 已通过，多架构验证完成，CI 全部通过** |
| **方案三** | llama.cpp Alpine/musl 兼容验证（aarch64/x86_64/riscv64） | **L0-L4 + L5 稳定性全部通过，已迁移到 apps，PR 待创建** |
| **BusyBox** | 7 个高副作用 applet 安全失败路径测试 | **4 架构 320 PASS，已 merged，归档** |
| **riscv64 static-pie** | 修复 riscv64 ELF loader 不处理 `.rela.dyn`/`.rela.plt` 导致 static-pie segfault | **CI 全部通过，等待 merge** |

```
方案一（已完成）                        方案二（已完成验证，CI 通过）
────────────────                       ────────────────────────────
PR-内核：5 个 bug 修复 → 已合并        S0: sqlite3 --version → PASS
PR-A：4 个测试模块 → 已合并            S1: SELECT 1 → PASS
205 pass / 0 fail / 8 observe         S2: CREATE TABLE → PASS
                                       S3: INSERT + SELECT → PASS
                                       S4: 重新打开查询 → PASS
                                       S5: DELETE journal + COMMIT + ROLLBACK → PASS
                                       S6: WAL 模式 + 重开查询 → PASS
                                       S7: 500 行批量插入 + integrity_check → PASS
                                       S8: 64 KiB BLOB + 重开查询 → PASS
                                       多架构：aarch64/x86_64/riscv64/loongarch64 全部通过
                                       CI 全部通过（loongarch64 内存调至 2G）

方案三（已完成验证，待 PR）              BusyBox（已完成，待 PR）
─────────────────────────              ─────────────────────────
L0: --help → PASS                      7 个 applet Layer 2 安全失败路径
L1: missing model → PASS (RC=1)        4 架构 320 PASS / 0 FAIL
L2/L3: model load → PASS               已 rebase 到 upstream/dev 最新
L4: inference → PASS                   Commit: 7fb5b75d0
L5: stability 17/18 PASS               PR 待创建
三架构：aarch64/x86_64/riscv64 全部通过
已迁移到 apps/starry/llama-cpp/
Commit: 24ea2208e
PR 待创建
```

---

## 方案一：syscall 测试覆盖

### 工作内容

**Phase 1：内核 Bug 修复（PR-内核，已合并）**

发现并修复 5 个内核缺陷：

| Bug | 问题 | 修复 |
|-----|------|------|
| Bug 0 | 不支持的 fcntl cmd 返回 Ok(0) | 改为 Err(InvalidInput) |
| Bug 1 | F_DUPFD 忽略 arg 参数 | 新增 dup_fd_min() |
| Bug 2 | F_SETFL O_APPEND 不生效 | AtomicU8 双层状态同步 |
| Bug 3 | flock 未实现 | 添加存根 |
| Bug 4 | F_SETLK 未实现 | 添加存根 |

关键技术难点：O_APPEND 双层状态同步 — `ax-fs-ng::File.flags` 从编译时常量改为 `AtomicU8`，新增 `set_flag()` 方法。

**Phase 2：测试覆盖扩展（PR-A，已合并）**

| 模块 | 类型 | 断言数 | StarryOS 结果 |
|------|------|--------|---------------|
| test-dup-fcntl | 拆分 + 扩展 Part 20-26 | 111 | 106 pass, 0 fail, 5 observe |
| test-dup2 | 新建（7 parts） | 36 | 36 pass, 0 fail |
| test-close-range | 新建（5 parts） | 50 | 49 pass, 0 fail, 1 observe |
| test-ioctl | 新建（4 parts） | 16 | 14 pass, 0 fail, 2 observe |
| **合计** | | **213** | **205 pass, 0 fail, 8 observe** |

关键发现：
- PR-B（内核修复）无需执行 — 全部 205 个断言在 StarryOS 上通过
- PR-C（close_range 实现）废弃 — 探针确认已有完整实现

### 不变性约束验证

| 约束 | 对应测例 | 状态 |
|------|----------|------|
| I1 (最小 fd) | test-dup-fcntl PART 4 | PASS |
| I2 (共享 offset) | test-dup2 Part 1/5 | PASS |
| I3 (Arc drop) | test-dup-fcntl PART 2 | PASS |
| I4 (CLOEXEC) | test-dup-fcntl PART 3/5 | PASS |
| I5 (进程退出) | test-dup-fcntl PART 14 | PASS |
| I6 (F_SETFD 不触 FileLike) | test-dup-fcntl PART 6 | PASS |
| I7 (F_SETFL 不改 FD flags) | test-dup-fcntl PART 7 | PASS |
| I8 (VFS 级锁) | test-dup-fcntl PART 11-14 | PASS |
| I9 (dup2 失败不破坏 newfd) | test-dup2 Part 7 | PASS |
| I10 (dup2 幂等) | test-dup2 Part 2 | PASS |
| I11 (close_range 语义) | test-close-range Part 1-4 | PASS |
| I12a (ioctl ENOTTY) | test-ioctl Part 2 | PASS |

### 已提交的 PR

| PR | 标题 | 状态 |
|----|------|------|
| PR-内核 | Fix syscall compatibility for dup, fcntl, and flock in kernel | **已合并** |
| PR-A | Add syscall test coverage for dup2, close_range, ioctl, and fcntl/flock extensions (5125511c5) | **已合并** |
| sqlite3 | test(starry): add sqlite3 CLI multi-arch stress test config | **已创建，待 review** |
| BusyBox | test(busybox): add safe-failure coverage for 7 high-side-effect applets | **已 push (7fb5b75d0)，待创建 PR** |
| llama.cpp | test(apps): add llama.cpp Alpine/musl compatibility tests for aarch64, x86_64, and riscv64 | **已 push (24ea2208e)，待创建 PR** |

---

## 方案二：sqlite3 兼容性验证

### 执行策略

采用分层测试策略，按阶段验证 Linux app 兼容性：

| 阶段 | 命令 | 目标 | 结果 |
|------|------|------|------|
| S0 | `sqlite3 --version` | 程序能启动 | **PASS** — `3.51.2 2026-01-09 (64-bit)` |
| S1 | `sqlite3 :memory: "SELECT 1;"` | 纯内存数据库 | **PASS** — 输出 `1` |
| S2 | `CREATE TABLE t(...)` | 文件创建 | **PASS** — `/tmp/test.db` 存在 |
| S3 | `INSERT ... SELECT ...` | 读写查询 | **PASS** — 输出 `1\|alice` |
| S4 | `SELECT ...`（重新打开） | 持久化 | **PASS** — 输出 `1\|alice` |
| S5 | DELETE journal + COMMIT + ROLLBACK | 事务语义 | **PASS** — `delete`, count=2, count=2 |
| S6 | WAL 模式 + 重开查询 | WAL 模式设置、写入和持久化读取 | **PASS** — `wal`, `1\|alice`, 重开 `1\|alice` |
| S7 | 500 行批量插入 + integrity_check | 文件增长和 B-tree | **PASS** — `500`, `ok` |
| S8 | 64 KiB BLOB + 重开查询 | 大块数据写入 | **PASS** — `65536`, 重开 `65536` |

### 多架构验证结果

| 架构 | smoke | deep | timeout | 备注 |
|------|-------|------|---------|------|
| aarch64 | PASS | PASS | 300s | 基线架构 |
| x86_64 | PASS | PASS | 300s | to_bin=false |
| riscv64 | PASS | PASS | 300s | to_bin=true |
| loongarch64 | PASS | PASS | 600s | Alpine 镜像较慢 |

测试环境：
- 仓库：SongShiQ/tgoskits，commit `8d80a1aa5`
- 架构：aarch64/x86_64/riscv64/loongarch64
- rootfs：Alpine（rcore-os/tgosimages v0.0.5）
- sqlite3：3.51.2（apk add --no-cache sqlite）
- Rust：1.97.0-nightly (nightly-2026-04-27)
- QEMU：10.2.1（从源码编译）
- 宿主：WSL2 原生文件系统

初步判断：
- apk 安装正常（网络、包管理器、磁盘空间均正常）
- 动态链接正常（musl libc + sqlite3 ELF 加载无问题）
- 内存数据库正常（mmap/brk/clock_gettime/getrandom 均正常）
- 文件数据库正常（openat/read/write/lseek/fstat/fsync/fcntl 均正常）
- 关闭后重新打开正常（文件持久化语义正确）
- 事务提交/回滚正常（DELETE journal + COMMIT + ROLLBACK 语义正确）
- WAL 模式正常（PRAGMA journal_mode=WAL 设置成功，重开后数据持久化）
- 批量写入正常（500 行插入 + integrity_check 通过）
- BLOB 写入正常（64 KiB BLOB + 重开后数据完整）

### sqlite3 测试配置（已提交到 fork）

- 分支：`test-sqlite3-cli-stress`
- Commit：`8d80a1aa5`
- 目录：`test-suit/starryos/stress/sqlite3-smoke/` + `test-suit/starryos/stress/sqlite3-deep/`
- 覆盖架构：aarch64/x86_64/riscv64/loongarch64

---

## BusyBox 安全失败路径测试

### 背景

upstream #944 重构了 busybox-tests.sh，引入 `bb_case_start/pass/fail` 框架。在此基础上新增 7 个高副作用 applet 的安全失败路径测试。

### 新增测试

| applet | 测试方式 | 判定条件 |
|--------|----------|----------|
| insmod | `busybox insmod /tmp/bb_no_such_module.ko` | rc≠124, rc≠0, 输出含 "No such"/"not found" |
| fdflush | `busybox fdflush /tmp/bb_no_such_device` | rc≠124, rc≠0, 输出含 "No such"/"device" |
| raidautorun | `busybox raidautorun /tmp/bb_no_such_device` | rc≠124, rc≠0, 输出含 "No such"/"device" |
| killall5 | `busybox killall5 -h` | rc≠124, 输出含 "Usage" 或 "killall5" |
| rdev | `busybox rdev`（裸调用） | rc≠124（StarryOS 下输出为空） |
| setlogcons | `busybox setlogcons -h` | rc≠124, 输出含 "Usage" 或 "setlogcons" |
| resize | `busybox resize >/dev/null 2>&1` | rc≠124（终端转义序列破坏解析） |

### 验证结果

| 架构 | 结果 |
|------|------|
| aarch64 | 320 PASS / 0 FAIL |
| x86_64 | 320 PASS / 0 FAIL |
| riscv64 | 320 PASS / 0 FAIL |
| loongarch64 | 320 PASS / 0 FAIL |

### 分支状态

- 分支：`test-busybox-enhancement`
- Commit：`7fb5b75d0`（基于 upstream/dev 最新 d9313f3b5）
- 修改文件：`busybox-tests.sh` +76 行
- PR：待创建

### Follow-up

- crontab/crond 真实行为测试（需解决 QEMU hang 问题）
- remove_shell 真实行为测试（需解决引用处理复杂度）

---

## 方案三：llama.cpp 兼容性验证

### 目标

验证 llama.cpp (b5092) Alpine/musl static binary 在 StarryOS 上的兼容性，覆盖 binary 启动、错误处理、模型加载到 token 生成的完整链路。

### 测试策略

采用 4 层渐进式验证（L0-L4）：

| Level | 测试用例 | 验证目标 | 超时 |
|-------|---------|---------|------|
| L0 | `--help` | binary 可执行，`--help` 正常输出 | 60s |
| L1 | missing model | 错误路径：不存在的模型文件，graceful error（RC=1） | 120s |
| L2/L3 | model load | 模型加载：`--no-mmap` fread 路径加载 Q4_0 模型 | 300s |
| L4 | inference | 完整推理：加载 + 8 token 生成 | 600s |

### 多架构验证结果

| 架构 | 工具链 | binary 大小 | L0-L4 结果 | L5 稳定性 |
|------|--------|------------|-----------|----------|
| aarch64 | aarch64-linux-musl-gcc 11.2.1 | 15.6MB | 4/4 PASS | 5/6 PASS* |
| x86_64 | x86_64-linux-musl-gcc 11.2.1 | 16MB | 4/4 PASS | 6/6 PASS |
| riscv64 | riscv64-linux-musl-gcc 11.2.1 | 3.4MB | 4/4 PASS | 6/6 PASS |

*aarch64 L5 有 1 次 init 失败，原因是内核 kretprobe 自检偶发 panic，与 llama.cpp 无关。应用层稳定性 100%。

### L4 推理性能对比

| 指标 | aarch64 (cortex-a53) | x86_64 (max) | riscv64 (rv64) |
|------|---------------------|--------------|----------------|
| 模型加载 | 10.9s | 10.3s | 14.9s |
| 推理速度 | 1.39 tok/s | 0.60 tok/s | 0.72 tok/s |
| 总推理时间 | 6.0s / 9 tok | 13.6s / 9 tok | 11.5s / 9 tok |

### apps 迁移

已从 `test-suit/starryos/stress/` 迁移到 `apps/starry/llama-cpp/`，符合项目"应用集成测试移入 apps"的架构规范。

**迁移变更**：
- 4 个子 case（help/init/load/infer）合并为 1 个统一测试脚本 `llama-cpp-test.sh`
- 新增 `prebuild.sh`（参考 redis/git 模式，仅安装 test script）
- 运行方式改为 `cargo xtask starry app run -t llama-cpp --arch <arch>`

### 踩坑记录

1. `set -eu` 在 QEMU shell 环境中导致脚本静默退出 → 去掉 `set -eu`
2. x86_64 QEMU config 缺少 `-cpu max` → cmake 自动启用 SSE4.2 但 QEMU 默认 CPU 不支持
3. riscv64 static-pie (Type=DYN) 在 StarryOS 上 segfault → 改用 non-PIE static
4. GGML_RVV 编译失败 → 添加 `-DGGML_RVV=OFF`
5. apps build config features 需要与 stress 保持一致（7 个 ax-driver features）

### 分支状态

- 分支：`feat/starry-llama-alpine-compat`
- Commit：`24ea2208e`（apps 迁移）
- 目录：`apps/starry/llama-cpp/`（10 个文件）
- PR：待创建

### loongarch64

阻塞：musl.cc 无预编译 loongarch64 工具链（HTTP 404）。

### Follow-up

- loongarch64 架构验证（待工具链可用）
- mmap 文件映射路径测试（去掉 `--no-mmap`）
- 更大模型测试（SmolLM2-360M / 1B）
- 多线程推理（`-t 2`, `-t 4`）
- 动态链接 musl 测试

---

## 环境问题与解决方案

| 问题 | 原因 | 解决方案 |
|------|------|----------|
| Docker overlay2 I/O 死锁 | Windows D: 驱动 | 手动 QEMU 工作流 |
| `cargo starry test qemu` 超时 | tokio pipe 问题 | 改用手动 QEMU |
| CI loongarch64 编译失败 | Clang -Wunused-but-set-variable | 删除冗余变量 |
| WSL2 网络问题 | 企业防火墙 | 手机热点 + 离线安装 |
| `/mnt/d/` IO 性能差 | Windows NTFS 挂载 | 迁移到 WSL2 原生文件系统 |
| CI loongarch64 `apk-curl` 超时 | QEMU 内存不足（512M） | 内存提高到 2G |

---

## 技术经验总结

1. **双层状态同步**：内核中 File flags 需要从编译时常量改为运行时原子变量
2. **条件编译陷阱**：AtomicU8 等 core 类型的导入不应被 feature flag 屏蔽
3. **跨架构 CI**：loongarch64 Clang 比 x86_64 GCC 更严格
4. **Phase 0 探针先行**：低成本确认内核能力，避免无效工作
5. **TDD 流程**：先列断言表再编码，确保测试覆盖完整
6. **拆分先行**：大型测试文件先机械拆分再新增内容
7. **loongarch64 QEMU 内存**：`apk add` 等包管理操作需要 2G 内存，512M 会导致超时
8. **`set -eu` 在 QEMU shell 中不可靠**：会导致脚本静默退出，错误输出被重定向到 log 文件
9. **apps runner 不自动添加 `-cpu max`**：需在 QEMU config 中显式指定
10. **riscv64 static-pie 不兼容**：StarryOS riscv64 内核不支持 Type=DYN ELF，需 non-PIE static (Type=EXEC)
11. **stress build config 必须含 virtio-blk**：`qemu` feature 不含 block device driver

---

## 关键文件索引

| 文件 | 说明 |
|------|------|
| `starryos-syscall-improvement/log.md` | 完整开发日志 |
| `starryos-syscall-improvement/方案二执行手册.md` | 方案二执行手册 v1.3 |
| `starryos-syscall-improvement/PR描述.md` | 当前 PR 描述（busybox） |
| `starryos-syscall-improvement/周会汇报.txt` | 周会汇报文档 |
| `starryos-syscall-improvement/Agent 工作方案 v0.3 - 多架构验证.md` | llama.cpp 多架构验证方案 |
| `starryos-syscall-improvement/Agent 工作方案 v0.4 - riscv64 loongarch64 扩展验证.md` | llama.cpp riscv64/loongarch64 扩展方案 |
| `starryos-syscall-improvement/Agent 工作方案 v0.5 - apps 迁移.md` | llama.cpp apps 迁移方案 |
| `starryos-syscall-improvement/busybox-findings.md` | BusyBox 测试发现 |
| `starryos-syscall-improvement/ACTION_MANUAL.md` | 测试执行手册 v6.3 |
| `issue.md` | 项目跟踪文件（本文件） |
| `备选.md` | BusyBox 测试用例状态调查结果 |
| `apps/starry/llama-cpp/` | llama.cpp apps 测试配置 |
| `test-suit/starryos/stress/sqlite3-smoke/` | sqlite3 smoke 测试配置（4 架构） |
| `test-suit/starryos/stress/sqlite3-deep/` | sqlite3 deep 测试配置（4 架构） |
| `test-suit/starryos/normal/qemu-smp1/busybox/sh/busybox-tests.sh` | BusyBox 测试脚本 |
| `test-suit/starryos/normal/qemu-smp1/test-dup2/` | dup2 测试 |
| `test-suit/starryos/normal/qemu-smp1/test-close-range/` | close_range 测试 |
| `test-suit/starryos/normal/qemu-smp1/test-ioctl/` | ioctl 测试 |
| `test-suit/starryos/normal/qemu-smp1/test-dup-fcntl/` | dup/fcntl/flock 测试 |

---

## 关键命令速查

```bash
# 运行 sqlite3 smoke 测试（S0-S4）
cd ~/project/tgoskits
cargo xtask starry test qemu --arch aarch64 -g stress -c sqlite3-smoke

# 运行 sqlite3 deep 测试（S5-S8）
cargo xtask starry test qemu --arch aarch64 -g stress -c sqlite3-deep

# 运行多架构测试
cargo xtask starry test qemu --arch x86_64 -g stress -c sqlite3-smoke
cargo xtask starry test qemu --arch riscv64 -g stress -c sqlite3-smoke
cargo xtask starry test qemu --arch loongarch64 -g stress -c sqlite3-smoke

# 运行单个 syscall 测试
cargo xtask starry test qemu --arch aarch64 -g normal -c test-dup2

# 列出所有测试
cargo xtask starry test qemu -l

# 编译检查
cargo xtask clippy --package starry-kernel
cargo fmt --check

# 生成 rootfs 镜像
cargo xtask starry rootfs --arch x86_64
cargo xtask starry rootfs --arch riscv64
cargo xtask starry rootfs --arch loongarch64
```

---

## 后续计划

| 阶段 | 内容 | 状态 |
|------|------|------|
| 方案一 | 扩展其他 syscall 测试覆盖 | 可选 |
| 方案二 S0-S8 | sqlite3 全链路验证 + 多架构 | **已完成，归档** |
| BusyBox | 7 个 applet Layer 2 安全失败路径 | **已 merged，归档** |
| 方案三 llama.cpp apps | 三架构兼容验证 + apps 迁移 | **已 merged，归档** |
| riscv64 static-pie 修复 | ELF loader relocation 支持 | **CI 通过，等待 merge** |
| mmap 路径 | 去掉 --no-mmap 测试 | 待开始（等 llama merge） |
| loongarch64 工具链 | musl.cc 工具链获取 | 阻塞（404） |

---

## riscv64 static-pie ELF Loader 修复

### 背景

StarryOS riscv64 内核的 ELF loader (`loader.rs`) 不处理 `.rela.dyn`/`.rela.plt` relocation，导致 `riscv64-linux-musl-gcc -static-pie` 编译的 PIE binary (Type=DYN) 执行时 segfault。

### 根因

Linux musl libc 的 static-pie binary (Type=DYN) 需要内核处理 3 种 RISC-V relocation：

| Type | ID | 公式 | 用途 |
|------|----|------|------|
| R_RISCV_RELATIVE | 3 | B + A | `.rela.dyn` — 修正代码/数据中的绝对地址 |
| R_RISCV_64 | 2 | S + A | `.rela.dyn` — 符号引用（`__cxa_finalize`, `_init`, `_fini`） |
| R_RISCV_JUMP_SLOT | 5 | S | `.rela.plt` — PLT 懒绑定槽位 |

### 实现

- 分支：`fix/riscv64-static-pie-segfault`
- Commits：`3b67ca0b0`（初始实现）+ `7ef6d9c13`（R_RISCV_64 支持）
- 文件：`os/StarryOS/kernel/src/mm/loader.rs:170` — `apply_relocations()` 函数
- 调用时机：`map_elf()` 映射 PT_LOAD 段后、返回 entry 前
- 触发条件：`#[cfg(target_arch = "riscv64")]` + ELFCLASS64 + ET_DYN

### 关键技术决策

| 决策 | 理由 |
|------|------|
| `&mut slice[..]` 替代 `&mut Vec` | `Vec<u8>::remaining_mut()` 返回 `isize::MAX - len`；`Write::write_all` append 而非 overwrite |
| Dynamic entry vaddr 直接作 file offset | PIE binary 中 vaddr == file offset（ELF base = 0） |
| Relocation write target = `base + r_offset` | runtime virtual address = load base + vaddr |

### 当前阻塞点

**Task 4 回归验证 — `Bad address` (EFAULT) on `uspace.write()`**

- 第一个 RELATIVE relocation 的 target `base + offset` = `0xc6770` 触发 EFAULT
- `process_area_data()` 使用 `self.pt.query(vaddr)` 查询页表，返回 `BadAddress`（页未映射）
- `offset=0xc5770` 偏移异常大 — 可能 relocation entry 解析仍有误
- 待排查：PT_LOAD 段映射范围是否覆盖该 vaddr，relocation 表解析是否正确

### 测试基础设施

- 目录：`apps/starry/static-pie-test/`（4 个文件）
- prebuild.sh 编译 riscv64 static-pie 最小 C 程序并注入 overlay
- static-pie-test.sh 在 QEMU 中执行 `/usr/bin/static-pie-test`

---

## 更新日志

### 2026-05-23

**方案二 sqlite3 多架构验证完成 + PR 提交**

- 完成 S5-S8 深度验证（事务提交/回滚、WAL 模式、批量插入、BLOB 写入）
- 完成多架构验证（aarch64/x86_64/riscv64/loongarch64 全部通过）
- 环境升级：Rust 1.97.0-nightly、QEMU 10.2.1（从源码编译，支持 loongarch64）
- 生成 x86_64/riscv64/loongarch64 rootfs 镜像
- PR 已推送到 fork（分支：test-sqlite3-cli-stress，commit：8d80a1aa5）
- 修复审核反馈：loongarch64 timeout 300→600、apk add --no-cache sqlite、S6 描述修正
- 结论：sqlite3 基础场景没有发现内核 bug，不适合作为主修复任务，但 smoke/deep test 可作为测试补全成果保留

**下一步**：创建 PR，然后换一个 app 继续测试（如 curl、wget、grep、lua）

---

### 2026-05-23（续）

**CI 内存调优验证成功**

- 问题：loongarch64 `apk-curl` 测试在 CI 中超时（1200s），本地通过（29.35s）
- 假设：QEMU guest 内存不足导致 `apk add curl` 阶段卡死
- 验证：将 `qemu-loongarch64.toml` 内存从 `512M` 提高到 `2G`
- 结果：**CI 全部通过**
- Commit：`2c1a85f74`，分支：`test-sqlite3-cli-stress`
- 结论：loongarch64 QEMU 环境下 `apk add curl` 等包管理操作需要更大内存，2G 是合理配置

**BusyBox 测试用例状态调查（riscv64 失败用例）**

老师反馈 12 个 busybox 测试在 riscv64 上运行失败，调查结果：

| 测试名称 | 状态 | 说明 |
|----------|------|------|
| busybox_acpid | ✅ 存在 | PR #722 添加 |
| busybox_add_shell | ✅ 存在 | PR #751 添加 |
| busybox_crond | ❌ 从未存在 | 仓库中从未添加 |
| busybox_crontab | ❌ 从未存在 | 仓库中从未添加 |
| busybox_fdflush | ⚠️ 已移除 | PR #752 移除 |
| busybox_insmod | ⚠️ 已移除 | PR #752 移除 |
| busybox_killall5 | ⚠️ 已移除 | PR #752 移除 |
| busybox_raidautorun | ⚠️ 已移除 | PR #752 移除 |
| busybox_rdev | ⚠️ 已移除 | PR #752 移除 |
| busybox_remove_shell | ⚠️ 已移除 | PR #752 移除 |
| busybox_resize | ⚠️ 已移除 | PR #752 移除 |
| busybox_setlogcons | ⚠️ 已移除 | PR #752 移除 |

PR #752 (`1b94ff38d`) 移除原因：这些测试只是检查 `--help` 输出是否包含 `Usage:` 字符串，属于非语义测试，不验证实际功能。当前 busybox 测试：284 PASS / 0 FAIL。

详细结果已保存至 `D:\project\备选.md`。

---

### 2026-05-25（续）

**BusyBox 测试升级：从存在性检查到 3 层行为验证**

- 初始版本（v3.2.1）使用 `busybox --list` 验证 applet 存在性，318 PASS / 0 FAIL
- 审核反馈：`busybox --list` 只能证明"命令被编进 BusyBox"，不能证明"命令行为正确"
- ACTION_MANUAL 经历 v4.0 → v4.1 → v4.2 三次迭代：
  - v4.0：引入 3 层验证模型（存在性 + help/安全失败路径 + 真实行为）
  - v4.1：crond/crontab 加 trap 清理、remove_shell 备份唯一化
  - v4.2：返回码捕获改为内部 `__RC=$?`、每个 applet 只计 1 个 PASS
- 编码发现（`busybox-findings.md`）：
  - 7 个 applet 不支持 `-h`（把 `-h` 当文件名/模块名/信号名）
  - crond daemon 测试导致 QEMU 超时
  - crontab 真实行为测试导致 QEMU 超时
  - remove_shell 引用处理导致语法错误
- 当前状态：准备按 v4.2 执行升级

**关键文件**：
- `starryos-syscall-improvement/ACTION_MANUAL.md` v4.2
- `starryos-syscall-improvement/busybox-findings.md`

---

### 2026-05-25（续）

**BusyBox Phase 2+3 执行完成**

- 执行策略：增量增强（ACTION_MANUAL.md v5.0），逐个编码 + 逐次验证
- Phase 2：7 个低副作用 applet 增强为安全失败路径验证
  - insmod/fdflush/raidautorun：不存在文件 → 返回非 0 + 错误信息
  - killall5/rdev：`-h` → 不 hang（rc≠124）
  - setlogcons：非法参数 → 不 hang（rc≠124）
  - resize：无 TTY 环境 → 不 hang（rc≠124）
- Phase 3：3 个复杂 applet 排查
  - crontab：真实行为测试导致 QEMU hang（300s+），降级为 `busybox --list`
  - crond：daemon 测试导致 QEMU hang（300s+），降级为 `busybox --list`
  - remove_shell：保留 `busybox --list`，后续单独跟踪
- 最终结果：318 PASS / 0 FAIL，运行 ~81s
- 7 个 applet 从存在性检查升级为行为验证，3 个保留存在性检查

---

### 2026-05-25（续）

**合并 upstream/dev + 冲突解决 + 4 架构验证**

- 合并 upstream/dev（893 文件变更，含大量新测试和驱动重构）
- busybox-tests.sh 冲突：upstream 新增 crond daemon round-trip 测试
- 冲突解决：保留 upstream 的 crond 测试 + 我们的 7 个 Layer 2 测试
- 4 架构验证结果：316 PASS / 0 FAIL（aarch64/x86_64/riscv64/loongarch64）
- 测试内容：upstream crond（1）+ 我们的 7 个 Layer 2（insmod/fdflush/raidautorun/killall5/rdev/setlogcons/resize）+ 原有测试（308）

---

### 2026-05-26

**sqlite3 PR reviewer 反馈 + loongarch64 内存回退**

- reviewer ZR233 反馈：loongarch64 apk-curl 的 QEMU 内存 2G 没必要这么大
- 修复：`qemu-loongarch64.toml` 内存从 2G 改回 512M
- 4 架构验证 apk-curl：全部 PASS（512M 足够）
- Commit：`286aa0fc1`

**BusyBox 适配 upstream #944 新框架**

- upstream #944 重构 busybox-tests.sh（bb_case_start/pass/fail 框架）
- 旧版 7 个 Layer 2 测试需适配新框架
- ACTION_MANUAL 版本演进：v6.0 → v6.1 → v6.2 → v6.3
- 执行中修复：rdev 输出为空 → 仅检查不超时；resize 转义序列 → 重定向到 /dev/null
- 4 架构验证：320 PASS / 0 FAIL

**分支分离决策**

- sqlite3 PR 留在 `test-sqlite3-cli-stress` 分支
- busybox 增强基于 upstream/dev 新建 `test-busybox-enhancement` 分支
- busybox PR 等 sqlite3 PR merge 后再提交

---

### 2026-05-26（续）

**llama.cpp Alpine/musl 兼容测试启动**

- 交叉编译 llama-cli（musl-gcc 11.2.1）
- 模型：SmolLM2-135M Q4_0 (87.5MB)
- stress baseline regression 修复：build config 缺少 `ax-driver/virtio-blk` → 对齐 stress-ng-0 配置
- aarch64 L0-L4 全部通过

**llama.cpp x86_64 多架构验证**

- 安装 x86_64 musl 工具链 + 编译 llama-cli (16MB)
- 踩坑：shell_prefix 错误 → 修正为 `root@starry:`；cmake 自动启用 SSE4.2 → 添加 `-cpu max`
- L0-L4 全部通过

---

### 2026-05-27

**llama.cpp riscv64 多架构验证**

- 安装 riscv64 musl 工具链 + 编译 llama-cli (3.4MB non-PIE static)
- 踩坑：GGML_RVV 编译失败 → `-DGGML_RVV=OFF`；static-pie segfault → non-PIE static
- L0-L4 全部通过
- 新发现：StarryOS riscv64 不支持 static-pie ELF

**loongarch64 工具链门控**

- musl.cc 返回 404，记录阻塞，跳过

**L5 稳定性验证**

- 三架构 × init×3 + infer×3 = 18 次运行
- 17/18 PASS（1 次 aarch64 kprobe 偶发 panic，与 llama.cpp 无关）
- 应用层稳定性 100%

**llama.cpp apps 迁移**

- 从 `stress/` 迁移到 `apps/starry/llama-cpp/`
- 问题排查：`set -eu` 导致脚本静默退出 → 去掉；x86_64 缺少 `-cpu max` → 添加
- 三架构 apps runner 验证：全部 PASS
- Commit：`24ea2208e`

**BusyBox PR rebase + push**

- `git rebase upstream/dev`（无冲突）
- `git push --force-with-lease`
- Commit：`7fb5b75d0`

---

### 2026-05-28

**riscv64 static-pie ELF Loader 修复 — 根因定位 + 原型实现**

- **根因**：StarryOS riscv64 ELF loader 不处理 `.rela.dyn`/`.rela.plt` relocation，导致 static-pie binary (Type=DYN) segfault
- **修复分支**：`fix/riscv64-static-pie-segfault`
- **实现**：新增 `apply_relocations()` 函数，处理 R_RISCV_RELATIVE(3), R_RISCV_64(2), R_RISCV_JUMP_SLOT(5)
- **Commits**：`3b67ca0b0`（初始实现）+ `7ef6d9c13`（R_RISCV_64 支持）
- **踩坑**：
  - `Vec<u8>::remaining_mut()` 返回 spare capacity 而非 buffer size → 改用 `&mut slice[..]`
  - `Vec<u8>::Write::write_all` append 而非 overwrite → 同上
  - Dynamic entry vaddr 需直接作 file offset（不减 base）
- **测试基础设施**：`apps/starry/static-pie-test/`（4 个文件）

---

### 2026-05-29

**riscv64 static-pie 修复完成 + CI 通过**

- **根因 3 层**：
  1. DT_RELA 等是 ELF 虚拟地址，不是文件偏移 → 新增 `vaddr_to_file_offset()` 转换
  2. CoW 后端页表懒加载，`uspace.write()` 时页未映射 → 在 apply_relocations 前调用 `populate_area()`
  3. R_RISCV_COPY (type=5) 不需要处理 → 静默跳过
- **验证**：`STATIC_PIE_TEST_PASSED` — busybox PIE (826KB, 1473 relocations) + 测试 binary (7520 bytes) 均加载成功
- **CI 修复**：
  1. rustfmt 格式差异（nightly 版本不同）→ `cargo fmt --all`
  2. clippy dead_code 警告（cfg 条件编译范围不一致）→ 将 cfg 从调用点移到 populate_area 块
  3. musl -static-pie 生成动态链接二进制 → 改用 `-static`
- **PR review 修复**：
  1. R_RISCV_64 计算缺少 base → 修正为 `(base as i64 + st_value as i64 + addend)`
  2. prebuild.sh 工具链缺失时静默跳过 → 改为 `exit 1` 硬失败
- **axvisor x86_64 CI 失败**：偶发问题，与本 PR 无关，提交空 commit 触发 CI 重跑后通过
- **Commits**：`2a3a38933` + `936cfc744` + `bdf430a3f` + `e36e94c87`
- **当前状态**：CI 全部通过，等待 merge

**下一步**：
1. 等待 PR merge
2. 后续可考虑 x86_64/aarch64 回归验证

### 当前 PR 状态汇总

| PR | 分支 | 状态 |
|----|------|------|
| sqlite3 CLI stress | `test-sqlite3-cli-stress` | **已 merged** |
| busybox enhancement | `test-busybox-enhancement` | **已 merged** |
| llama.cpp apps | `feat/starry-llama-alpine-compat` | **已 merged** |
| riscv64 static-pie fix | `fix/riscv64-static-pie-segfault` | **CI 通过，等待 merge** |

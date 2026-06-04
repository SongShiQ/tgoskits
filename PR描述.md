## 概述

修复 riscv64 static-pie 二进制文件在 StarryOS 上启动时的 segfault 问题。

根本原因是 ELF loader 中的 `apply_relocations()` 函数使用 `checked_sub(base)` 计算文件偏移，这对于 PIE 二进制中虚拟地址从 0 开始的情况是错误的。同时，relocation 写入前未调用 `populate_area()` 导致页面未映射。

## 修复内容

### 1. 添加 `vaddr_to_file_offset()` 函数
- 正确地将虚拟地址转换为文件偏移：搜索 PT_LOAD 段，计算 `segment_offset + (vaddr - segment_vaddr)`
- 替换了原来错误的 `(addr as usize).checked_sub(base)` 逻辑
- 用于 `.rela.dyn`、`.rela.plt` 和 `.dynsym` 的偏移计算

### 2. 添加 `populate_area()` 调用
- 在 `apply_relocations()` 之前，对所有 PT_LOAD 段调用 `populate_area()`
- 确保 relocation 写入时页面已映射，避免 page fault

### 3. 改进 relocation 处理
- `R_RISCV_64` / `R_RISCV_JUMP_SLOT`：跳过 `st_value==0` 的符号（未定义符号不应覆写 GOT）
- `R_RISCV_COPY`：计数并跳过（不需要内核处理）
- 移除未使用的 `in_load_mem()` 函数

### 4. 修复 CI clippy dead_code 警告
- 将 `#[cfg(target_arch = "riscv64")]` 从调用点移到 `populate_area` 块
- `apply_relocations()` 在所有架构下都被调用，非 riscv64 的 stub 返回 `Ok(())`

### 5. 添加 static-pie-test 回归测试
- `apps/starry/static-pie-test/`：完整的测试应用配置
- 使用 musl 工具链编译真正的 ET_DYN static-pie 二进制（`-static` 选项）
- 验证标准：输出 `STATIC_PIE_TEST_PASSED`，RC=0，无 segfault/panic/EFAULT

## 验证结果

| 测试 | 结果 |
|------|------|
| `cargo xtask starry build --arch riscv64` | ✅ PASS |
| `cargo xtask starry app run -t static-pie-test --arch riscv64` | ✅ PASS (`STATIC_PIE_TEST_PASSED`, RC=0) |
| `cargo xtask starry test qemu --arch riscv64 -c busybox` | ✅ PASS |
| `cargo fmt --check` | ✅ PASS |
| `cargo clippy --package starry-kernel` | ✅ PASS |

**测试二进制验证**：
- ELF Type: `DYN (Position-Independent Executable file)`
- No INTERP segment（不依赖动态链接器）
- No NEEDED entries（不依赖外部共享库）
- 23 个 R_RISCV relocation entries

## 技术细节

### 问题根因

PIE 二进制的程序头中 `virtual_addr` 从 0 开始：
```
PT_LOAD: offset=0x0, vaddr=0x0, filesz=1608
PT_LOAD: offset=0xE30, vaddr=0x1E30, filesz=584
```

原始代码使用 `(rela_addr as usize).checked_sub(base)` 计算文件偏移，但 `base` 是加载地址（如 `USER_SPACE_BASE`），不是虚拟地址的起始位置，导致计算错误。

### 修复方案

`vaddr_to_file_offset()` 函数正确处理这个情况：
```rust
fn vaddr_to_file_offset(vaddr: u64, ph: &[ProgramHeader64]) -> Option<usize> {
    for seg in ph {
        if seg.get_type() != Ok(Type::Load) { continue; }
        let seg_vaddr = seg.virtual_addr as usize;
        let seg_filesz = seg.file_size as usize;
        if vaddr >= seg_vaddr && vaddr < seg_vaddr + seg_filesz {
            return Some(seg.offset as usize + (vaddr - seg_vaddr));
        }
    }
    None
}
```

## 已知问题

- 本次修复仅针对 riscv64 架构（`#[cfg(target_arch = "riscv64")]`）
- 其他架构（x86_64、aarch64、loongarch64）的 static-pie 支持未涉及

## 修改文件

- `os/StarryOS/kernel/src/mm/loader.rs` - 核心修复
- `apps/starry/static-pie-test/` - 回归测试应用（新增）

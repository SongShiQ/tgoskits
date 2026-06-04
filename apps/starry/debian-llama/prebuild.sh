#!/usr/bin/env bash
set -euo pipefail

app_dir="$(cd "$(dirname "$0")" && pwd)"
overlay_dir="${STARRY_OVERLAY_DIR:-}"
base_rootfs="${STARRY_BASE_ROOTFS:-}"
output_rootfs="${STARRY_OUTPUT_ROOTFS:-}"

if [[ -z "$overlay_dir" ]]; then
    echo "ERROR: STARRY_OVERLAY_DIR is required" >&2
    exit 1
fi
if [[ -z "$base_rootfs" ]]; then
    echo "ERROR: STARRY_BASE_ROOTFS is required" >&2
    exit 1
fi
if [[ -z "$output_rootfs" ]]; then
    echo "ERROR: STARRY_OUTPUT_ROOTFS is required" >&2
    exit 1
fi

# 安装测试脚本到 overlay
install -Dm0755 "$app_dir/llama-test.sh" "$overlay_dir/usr/bin/llama-test.sh"

# 生成 llama 专用 rootfs（app runner 已复制 base_rootfs 到 output_rootfs）

MNT=/tmp/mnt-debian-llama
sudo mkdir -p "$MNT"
sudo mount -o loop "$output_rootfs" "$MNT"
trap 'sudo umount "$MNT" 2>/dev/null || true' EXIT

# 注入 llama-cli
if [[ -f "$app_dir/llama-cli" ]]; then
    sudo install -Dm0755 "$app_dir/llama-cli" "$MNT/usr/bin/llama-cli"
fi

# 注入模型
if [[ -f "$app_dir/tiny-llm-q4_0.gguf" ]]; then
    sudo install -Dm0644 "$app_dir/tiny-llm-q4_0.gguf" "$MNT/opt/models/tiny-llm-q4_0.gguf"
fi

# 安装 glibc 运行时（多架构）
case "$STARRY_ARCH" in
    aarch64)
        LIBDIR="/usr/aarch64-linux-gnu/lib"
        sudo mkdir -p "$MNT/lib/aarch64-linux-gnu"
        for lib in ld-linux-aarch64.so.1 libc.so.6 libstdc++.so.6 libm.so.6 libgcc_s.so.1; do
            [[ -f "$LIBDIR/$lib" ]] && sudo cp -a "$LIBDIR/$lib" "$MNT/lib/aarch64-linux-gnu/"
        done
        sudo ln -sf aarch64-linux-gnu/ld-linux-aarch64.so.1 "$MNT/lib/ld-linux-aarch64.so.1"
        ;;
    riscv64)
        LIBDIR="/usr/riscv64-linux-gnu/lib"
        sudo mkdir -p "$MNT/lib/riscv64-linux-gnu"
        for lib in ld-linux-riscv64-lp64d.so.1 libc.so.6 libstdc++.so.6 libm.so.6 libgcc_s.so.1; do
            [[ -f "$LIBDIR/$lib" ]] && sudo cp -a "$LIBDIR/$lib" "$MNT/lib/riscv64-linux-gnu/"
        done
        sudo ln -sf riscv64-linux-gnu/ld-linux-riscv64-lp64d.so.1 "$MNT/lib/ld-linux-riscv64-lp64d.so.1"
        ;;
    x86_64)
        LIBDIR="/usr/x86_64-linux-gnu/lib"
        sudo mkdir -p "$MNT/lib64"
        for lib in ld-linux-x86-64.so.2 libc.so.6 libstdc++.so.6 libm.so.6 libgcc_s.so.1; do
            [[ -f "$LIBDIR/$lib" ]] && sudo cp -a "$LIBDIR/$lib" "$MNT/lib64/"
        done
        sudo ln -sf x86_64-linux-gnu/ld-linux-x86-64.so.2 "$MNT/lib64/ld-linux-x86-64.so.2"
        ;;
esac

sudo umount "$MNT"
trap - EXIT

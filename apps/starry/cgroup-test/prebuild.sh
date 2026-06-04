#!/usr/bin/env bash
set -euo pipefail

app_dir="$(cd "$(dirname "$0")" && pwd)"
overlay_dir="${STARRY_OVERLAY_DIR:-}"

if [[ -z "$overlay_dir" ]]; then
    echo "ERROR: STARRY_OVERLAY_DIR is required" >&2
    exit 1
fi

command -v gcc >/dev/null 2>&1 || { echo "ERROR: gcc not found" >&2; exit 1; }
command -v readelf >/dev/null 2>&1 || { echo "ERROR: readelf not found" >&2; exit 1; }

case "$STARRY_ARCH" in
    aarch64)
        CC="aarch64-linux-gnu-gcc"
        ;;
    riscv64)
        CC="riscv64-linux-gnu-gcc"
        ;;
    x86_64)
        CC="x86_64-linux-gnu-gcc"
        ;;
    *)
        echo "ERROR: unsupported arch: $STARRY_ARCH" >&2
        exit 1
        ;;
esac

command -v "$CC" >/dev/null 2>&1 || { echo "ERROR: $CC not found" >&2; exit 1; }

$CC -static -o "$app_dir/cgroup-test" "$app_dir/cgroup-test.c"

install -Dm0755 "$app_dir/cgroup-test" "$overlay_dir/usr/bin/cgroup-test"
install -Dm0755 "$app_dir/cgroup-test.sh" "$overlay_dir/usr/bin/cgroup-test.sh"

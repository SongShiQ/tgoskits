#!/usr/bin/env bash
# prebuild.sh - provision a minimal CPython 3 + numpy runtime and the RK3588
# SenseVoice RKNN assets (librknnrt.so + model + decode resources) for StarryOS.
#
# The StarryOS rootfs gets an overlay tree under /opt/sensevoice/ :
#   /opt/sensevoice/lib/librknnrt.so   (aarch64 RKNN runtime)
#   /opt/sensevoice/model/*.rknn, embedding.npy, am.mvn, *.bpe.model, tokens.txt
#   /opt/sensevoice/python/sensevoice_rknn_npu.py
#   /opt/sensevoice/testwavs/*.wav
#   /usr/bin/python3 + numpy closure (provisioned into the base overlay)
#
# Env from the app runner: STARRY_ARCH, STARRY_ROOTFS, STARRY_STAGING_ROOT,
# STARRY_OVERLAY_DIR, STARRY_APP_DIR, STARRY_WORKSPACE.
set -euo pipefail

app_dir="${STARRY_APP_DIR:-$(cd "$(dirname "$BASH_SOURCE")" && pwd)}"
arch="${STARRY_ARCH:?prebuild: STARRY_ARCH required}"
base_rootfs="${STARRY_ROOTFS:?prebuild: STARRY_ROOTFS required}"
staging_root="${STARRY_STAGING_ROOT:?prebuild: STARRY_STAGING_ROOT required}"
overlay_dir="${STARRY_OVERLAY_DIR:?prebuild: STARRY_OVERLAY_DIR required}"
ws="${STARRY_WORKSPACE:?prebuild: STARRY_WORKSPACE required}"

APK_BRANCH="${SV_APK_BRANCH:-v3.23}"
ALPINE_CDN="${ALPINE_CDN:-https://dl-cdn.alpinelinux.org/alpine}"
ROOTFS_SIZE="${SV_ROOTFS_SIZE:-3G}"
APK_CACHE="${SV_APK_CACHE:-}"
RKNPU2_LIB="$ws/apps/starry/orangepi-5-plus-uvc-rknn/rknn-yolov8-image/3rdparty/rknpu2/Linux/aarch64/librknnrt.so"
MODEL_CACHE="$app_dir/model"
WAV_SRC="$app_dir/testwavs"

case "$arch" in
    aarch64)     qemu_runner="qemu-aarch64-static" ;;
    riscv64)     qemu_runner="qemu-riscv64-static" ;;
    x86_64)      qemu_runner="qemu-x86_64-static" ;;
    loongarch64) qemu_runner="qemu-loongarch64-static" ;;
    *) echo "prebuild: unsupported arch: $arch" >&2; exit 1 ;;
esac

ensure_host_tools() {
    local missing=()
    command -v debugfs    >/dev/null 2>&1 || missing+=(e2fsprogs)
    command -v resize2fs  >/dev/null 2>&1 || missing+=(e2fsprogs)
    command -v e2fsck     >/dev/null 2>&1 || missing+=(e2fsprogs)
    command -v truncate   >/dev/null 2>&1 || missing+=(coreutils)
    command -v readelf   >/dev/null 2>&1 || missing+=(binutils)
    command -v "$qemu_runner" >/dev/null 2>&1 || missing+=(qemu-user-static)
    command -v curl      >/dev/null 2>&1 || missing+=(curl)
    if [[ ${#missing[@]} -gt 0 ]]; then
        if command -v apt-get >/dev/null 2>&1; then
            echo "prebuild: installing host tools: ${missing[*]}"
            apt-get update && apt-get install -y --no-install-recommends "${missing[@]}"
        else
            echo "prebuild: missing host tools and no apt-get: ${missing[*]}" >&2
            exit 1
        fi
    fi
}

grow_rootfs() {
    [[ -f "$base_rootfs" ]] || { echo "prebuild: rootfs image missing: $base_rootfs" >&2; exit 2; }
    local before
    before=$(stat -c %s "$base_rootfs")
    echo "prebuild: rootfs is $((before / 1024 / 1024)) MiB; growing to $ROOTFS_SIZE"
    truncate -s "$ROOTFS_SIZE" "$base_rootfs"
    e2fsck -f -y "$base_rootfs" >/dev/null 2>&1 || true
    resize2fs "$base_rootfs" >/dev/null 2>&1
}

extract_base_rootfs() {
    rm -rf "$staging_root"; mkdir -p "$staging_root"
    debugfs -R "rdump / $staging_root" "$base_rootfs" >/dev/null 2>&1
    [[ -x "$staging_root/sbin/apk" ]] || { echo "prebuild: base rootfs has no apk" >&2; exit 2; }
}

normalize_symlinks() {
    local link tgt rel
    while IFS= read -r link; do
        tgt="$(readlink "$link")"
        [[ "$tgt" == /* ]] || continue
        rel="$(realpath -m --relative-to="$(dirname "$link")" "$staging_root$tgt")"
        ln -sf "$rel" "$link"
    done < <(find "$staging_root/lib" "$staging_root/usr/lib" -type l 2>/dev/null)
}

install_python() {
    normalize_symlinks
    [[ -f /etc/resolv.conf ]] && cp -f /etc/resolv.conf "$staging_root/etc/resolv.conf" || true
    printf '%s/%s/main\n%s/%s/community\n' \
        "$ALPINE_CDN" "$APK_BRANCH" "$ALPINE_CDN" "$APK_BRANCH" \
        > "$staging_root/etc/apk/repositories"
    local cache_args=()
    if [[ -n "$APK_CACHE" ]]; then
        mkdir -p "$APK_CACHE"
        cache_args=(--cache-dir "$APK_CACHE")
    fi
    echo "prebuild: apk add python3 py3-numpy ($APK_BRANCH) via $qemu_runner..."
    QEMU_LD_PREFIX="$staging_root" \
    LD_LIBRARY_PATH="$staging_root/lib:$staging_root/usr/lib" \
        "$qemu_runner" -L "$staging_root" \
            "$staging_root/sbin/apk" \
            --root "$staging_root" \
            --repositories-file "$staging_root/etc/apk/repositories" \
            --keys-dir "$staging_root/etc/apk/keys" \
            "${cache_args[@]}" \
            --update-cache --no-progress --no-scripts \
            add python3 py3-numpy
    local pyver
    pyver="$(ls -d "$staging_root"/usr/lib/python3.* 2>/dev/null | grep -oE 'python3\.[0-9]+' | head -1)"
    case "$pyver" in
        python3.1[2-9]|python3.2[0-9]) echo "prebuild: provisioned $pyver" ;;
        *) echo "prebuild: need CPython >= 3.12 but got '$pyver'" >&2; exit 3 ;;
    esac
    echo "$pyver" > /tmp/.sv_pyver
}

copy_to_overlay() {
    local src="$staging_root$1" dst="$overlay_dir$1"
    [[ -e "$src" ]] || { echo "prebuild: missing $1 after install" >&2; exit 4; }
    [[ -L "$src" ]] && src="$(readlink -f "$src")"
    install -Dm"$2" "$src" "$dst"
}

copy_so_closure() {
    local pending=("$@") seen=" " gp lib d
    while [[ ${#pending[@]} -gt 0 ]]; do
        gp="${pending[0]}"; pending=("${pending[@]:1}")
        [[ "$seen" == *" $gp "* ]] && continue
        seen+="$gp "
        while IFS= read -r lib; do
            for d in lib usr/lib usr/local/lib; do
                if [[ -e "$staging_root/$d/$lib" ]]; then
                    copy_to_overlay "/$d/$lib" 0644
                    pending+=("/$d/$lib")
                    break
                fi
            done
        done < <(readelf -d "$staging_root$gp" 2>/dev/null | sed -n 's/.*Shared library: \[\(.*\)\].*/\1/p')
    done
}

fetch_model_assets() {
    mkdir -p "$MODEL_CACHE"
    if [[ ! -f "$MODEL_CACHE/librknnrt.so" ]]; then
        [[ -f "$RKNPU2_LIB" ]] || { echo "prebuild: librknnrt.so not found at $RKNPU2_LIB" >&2; exit 5; }
        cp -f "$RKNPU2_LIB" "$MODEL_CACHE/librknnrt.so"
        echo "prebuild: copied librknnrt.so from uvc-rknn 3rdparty"
    fi

    local hf="${HF_ENDPOINT:-https://huggingface.co}"
    local rk="harvestsu/sensevoice-rknn"
    local hk="happyme531/SenseVoiceSmall-RKNN2"

    # Authoritative sizes from the HF LFS blob metadata. A cached file whose size
    # differs is a truncated download and MUST be refetched: rknn_init rejects a
    # short model with -6 (RKNN_ERR_MODEL_INVALID) only much later, on the board.
    # Files without an entry here fall back to a plain existence check.
    expected_size() {
        case "$1" in
            sense-voice-encoder.rk3588.fp16-scaled.rknn) echo 490649722 ;;
            *) echo "" ;;
        esac
    }

    fetch() {
        local repo="$1" path="$2" out="$3" url attempt want have
        want="$(expected_size "$out")"
        if [[ -f "$MODEL_CACHE/$out" ]]; then
            have="$(stat -c %s "$MODEL_CACHE/$out" 2>/dev/null || echo 0)"
            if [[ -z "$want" || "$have" == "$want" ]]; then
                echo "prebuild: skip (exists): $out"
                return 0
            fi
            echo "prebuild: cached $out is $have bytes, expected $want -- refetching" >&2
            rm -f "$MODEL_CACHE/$out"
        fi
        url="$hf/$repo/resolve/main/$path"
        # HF can be flaky (SSL_ERROR_SYSCALL under load); retry up to 3 times.
        # No --max-time: a 468MiB model legitimately outruns any fixed deadline on a
        # slow link, and a deadline kill mid-transfer is exactly how truncation happens.
        # --speed-limit/--speed-time abort a genuinely stalled transfer instead.
        for attempt in 1 2 3; do
            echo "prebuild: downloading $out (attempt $attempt)"
            if curl -fL --speed-limit 20000 --speed-time 60 "$url" -o "$MODEL_CACHE/$out" 2>/dev/null; then
                have="$(stat -c %s "$MODEL_CACHE/$out" 2>/dev/null || echo 0)"
                if [[ -n "$want" && "$have" != "$want" ]]; then
                    echo "prebuild: $out downloaded $have bytes, expected $want" >&2
                    rm -f "$MODEL_CACHE/$out"
                    sleep 2
                    continue
                fi
                return 0
            fi
            rm -f "$MODEL_CACHE/$out"
            echo "prebuild: attempt $attempt failed for $out" >&2
            sleep 2
        done
        echo "prebuild: FAILED to download $out from $url after 3 attempts" >&2
        return 1
    }

    fetch "$rk" "sense-voice-encoder.rk3588.fp16-scaled.rknn" "sense-voice-encoder.rk3588.fp16-scaled.rknn"
    fetch "$rk" "embedding.npy" "embedding.npy"
    fetch "$rk" "am.mvn" "am.mvn"
    fetch "$rk" "chn_jpn_yue_eng_ko_spectok.bpe.model" "chn_jpn_yue_eng_ko_spectok.bpe.model"
    # tokens.txt: generate from bpe.model with sentencepiece on the host.
    # The runtime rootfs only has numpy, so we pre-generate id->surface here.
    # Fallback: extract from the sherpa-onnx tarball if sentencepiece is absent.
    if [[ ! -f "$MODEL_CACHE/tokens.txt" ]]; then
        if python3 -c "import sentencepiece" 2>/dev/null; then
            echo "prebuild: generating tokens.txt from bpe.model via sentencepiece"
            python3 "$app_dir/gen_tokens.py" "$MODEL_CACHE"
        elif fetch "$hk" "sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17.tar.bz2" "sherpa.tar.bz2" 2>/dev/null; then
            tar -xjf "$MODEL_CACHE/sherpa.tar.bz2" -C "$MODEL_CACHE" --wildcards "*/tokens.txt" 2>/dev/null || true
            local tf
            tf="$(find "$MODEL_CACHE" -name tokens.txt 2>/dev/null | head -1)"
            [[ -n "$tf" ]] && mv -f "$tf" "$MODEL_CACHE/tokens.txt"
            rm -rf "$MODEL_CACHE/sherpa.tar.bz2" "$MODEL_CACHE"/sherpa-onnx-* 2>/dev/null || true
        else
            echo "prebuild: WARNING cannot generate tokens.txt (no sentencepiece, no sherpa tarball)" >&2
        fi
    fi
}

populate_overlay() {
    local pyver
    pyver="$(cat /tmp/.sv_pyver)"

    copy_to_overlay /usr/bin/python3 0755
    copy_so_closure /usr/bin/python3
    if [[ -d "$staging_root/usr/lib/$pyver/lib-dynload" ]]; then
        for so in "$staging_root/usr/lib/$pyver/lib-dynload"/*.so; do
            [[ -e "$so" ]] && copy_so_closure "/usr/lib/$pyver/lib-dynload/$(basename "$so")"
        done
    fi
    local sp="$staging_root/usr/lib/$pyver/site-packages"
    mkdir -p "$overlay_dir/usr/lib/$pyver"
    cp -a "$staging_root/usr/lib/$pyver/." "$overlay_dir/usr/lib/$pyver/"
    ln -sf python3 "$overlay_dir/usr/bin/python" 2>/dev/null || true
    if [[ -d "$sp" ]]; then
        while IFS= read -r so; do
            copy_so_closure "/usr/lib/$pyver/site-packages/${so#"$sp"/}"
        done < <(find "$sp" -name '*.so' 2>/dev/null)
    fi

    install -Dm0644 "$MODEL_CACHE/librknnrt.so" "$overlay_dir/opt/sensevoice/lib/librknnrt.so"

    local m
    for m in sense-voice-encoder.rk3588.fp16-scaled.rknn embedding.npy am.mvn \
             chn_jpn_yue_eng_ko_spectok.bpe.model tokens.txt; do
        [[ -f "$MODEL_CACHE/$m" ]] && install -Dm0644 "$MODEL_CACHE/$m" "$overlay_dir/opt/sensevoice/model/$m"
    done

    [[ -f "$ws/sensevoice_rknn_npu.py" ]] && \
        install -Dm0755 "$ws/sensevoice_rknn_npu.py" "$overlay_dir/opt/sensevoice/python/sensevoice_rknn_npu.py"
    [[ -f "$ws/sensevoice-rknn-test.sh" ]] && \
        install -Dm0755 "$ws/sensevoice-rknn-test.sh" "$overlay_dir/opt/sensevoice/sensevoice-rknn-test.sh"

    if [[ -d "$WAV_SRC" ]]; then
        mkdir -p "$overlay_dir/opt/sensevoice/testwavs"
        cp -a "$WAV_SRC/." "$overlay_dir/opt/sensevoice/testwavs/" 2>/dev/null || true
    fi

    mkdir -p "$overlay_dir/opt/sensevoice/bin"
    ln -sf /usr/bin/python3 "$overlay_dir/opt/sensevoice/bin/python3" 2>/dev/null || true

    echo "prebuild: overlay populated under $overlay_dir/opt/sensevoice"
}

ensure_host_tools
grow_rootfs
extract_base_rootfs
install_python
fetch_model_assets
populate_overlay
echo "prebuild: done"
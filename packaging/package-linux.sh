#!/usr/bin/env bash
# 构建 Linux 便携包（tar.gz）。
#
# 必须在 Linux 上运行：jlink 只能生成“当前平台”的运行时，无法交叉生成。
# 依赖：bash、cargo、JDK 25（JAVA_HOME，需包含 jlink）
#
# 用法：
#   ./packaging/package-linux.sh
#   OUT=/tmp/out VERSION=0.1.0 TARGET=x86_64-unknown-linux-gnu ./packaging/package-linux.sh
#   SKIP_LIBMPV=1 ./packaging/package-linux.sh      # 不打内置播放器
#
# libmpv 默认不捆绑（应用内按需下载；Linux 也可 BUNDLE_LIBMPV=1 显式捆绑）。
#
# 产物目录结构（与 Windows 包保持一致，Rust 侧自动识别）：
#   TVRust-linux-x64/
#     tv-rust                主程序
#     libmpv.so.2            内置播放器（构建时下载）
#     libmpv/                它的依赖库（LD_LIBRARY_PATH 或 rpath 指向这里）
#     tv-rust.sh             依赖较多且无 patchelf 时的启动包装脚本
#     spider-host-all.jar    JVM 爬虫宿主
#     runtime/bin/java       jlink 裁剪运行时
#     version.txt / LICENSE.txt

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${OUT:-$ROOT/dist/TVRust-linux-x64}"
JAR="${JAR:-$ROOT/spider-host/build/libs/spider-host-all.jar}"
TARGET="${TARGET:-}"
MODULES_FILE="$ROOT/packaging/common/jlink-modules.txt"

VERSION="${VERSION:-$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$ROOT/tv-rust/Cargo.toml" | head -n1)}"
VERSION="${VERSION:-0.0.0}"

log() { printf '%s\n' "$*"; }
die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

# ---------- 前置检查 ----------
[ -f "$MODULES_FILE" ] || die "缺少模块清单: $MODULES_FILE"
[ -f "$JAR" ] || die "缺少 spider-host jar: $JAR（先执行 cd spider-host && ./gradlew fatJar）"
[ -n "${JAVA_HOME:-}" ] || die "未设置 JAVA_HOME（需要 JDK 25，且包含 jlink）"
JLINK="$JAVA_HOME/bin/jlink"
[ -x "$JLINK" ] || die "找不到 jlink: $JLINK"

MODULES="$(grep -v '^[[:space:]]*#' "$MODULES_FILE" | tr -d ' \r' | grep -v '^$' | paste -sd, -)"
[ -n "$MODULES" ] || die "模块清单为空: $MODULES_FILE"

log "== TVRust Linux 便携包 =="
log "repo   : $ROOT"
log "output : $OUT"
log "version: $VERSION"
log "jdk    : $JAVA_HOME"

# ---------- 1) 编译 ----------
log "building : cargo build --release ${TARGET:+(target $TARGET)} ..."
if [ -n "$TARGET" ]; then
    cargo build --manifest-path "$ROOT/tv-rust/Cargo.toml" --release --target "$TARGET"
    BIN="$ROOT/tv-rust/target/$TARGET/release/tv_rust"
else
    cargo build --manifest-path "$ROOT/tv-rust/Cargo.toml" --release
    BIN="$ROOT/tv-rust/target/release/tv_rust"
fi
[ -f "$BIN" ] || die "编译产物不存在: $BIN"

# ---------- 2) 组装 ----------
mkdir -p "$OUT"
log "jlink    : $MODULES"
rm -rf "$OUT/runtime"
"$JLINK" --add-modules "$MODULES" \
    --strip-debug --no-header-files --no-man-pages --compress=zip-6 \
    --output "$OUT/runtime"
[ -x "$OUT/runtime/bin/java" ] || die "jlink 运行时缺少 bin/java"

install -m 0755 "$BIN" "$OUT/tv-rust"
install -m 0644 "$JAR" "$OUT/spider-host-all.jar"

# ---------- 2b) libmpv（内置播放器）：构建时获取，连同依赖一起打包 ----------
# 默认：自动获取 libmpv.so.2 并把它的依赖闭包一起打进包，目标机无需预装 mpv。
#   LIBMPV_SO=<file>      直接指定已解压的 libmpv.so.2
#   LIBMPV_ARCHIVE=<file> 指定本地压缩包（.deb/.rpm/.tar.xz）
#   LIBMPV_URL=<url>      指定下载地址（.deb/.rpm/.tar.xz/裸 .so 均可）
#   SKIP_LIBMPV=1         不打内置播放器（回落到系统 libmpv.so.2）
# 说明：Linux 的 libmpv.so.2 是动态链接的（ffmpeg/libass 等），所以必须连依赖一起带；
#       默认取“构建机自己发行版仓库”的包（apt/dnf/pacman），保证 glibc 与依赖同族。
LIBMPV_CACHE="$ROOT/tv-rust/libmpv/cache"
LIBMPV_SO="${LIBMPV_SO:-}"
LIBMPV_NOTE="not bundled (uses system libmpv.so.2)"

# 在目录里找已解压的 libmpv（命中则设 LIBMPV_SO）
libmpv_pick_from_dir() {
    local d="$1" f
    [ -d "$d" ] || return 1
    for f in "$d/libmpv.so.2" "$d/libmpv.so"; do
        if [ -f "$f" ]; then LIBMPV_SO="$f"; return 0; fi
    done
    return 1
}

# 解包归档并抽出 libmpv.so.2 到缓存（成功则设 LIBMPV_SO）
libmpv_unpack() {
    local a="$1" tmp="$LIBMPV_CACHE/_unpack" found="" data=""
    [ -f "$a" ] || return 1
    rm -rf "$tmp"; mkdir -p "$tmp"
    case "$a" in
        *.deb)
            if command -v dpkg-deb >/dev/null 2>&1; then
                # Debian/Ubuntu 构建机必有 dpkg-deb，它能直接处理 data.tar.zst/xz
                dpkg-deb -x "$a" "$tmp" || return 1
            else
                command -v ar >/dev/null 2>&1 || { log "libmpv   : 需要 dpkg-deb 或 ar 解 .deb"; return 1; }
                ( cd "$tmp" && ar x "$a" ) || return 1
                data="$(ls "$tmp"/data.tar.* 2>/dev/null | head -n1)"
                [ -n "$data" ] || return 1
                case "$data" in
                    *.zst)
                        command -v zstd >/dev/null 2>&1 || {
                            log "libmpv   : data.tar.zst 需要 zstd（或安装 dpkg-deb）"
                            return 1
                        }
                        ;;
                esac
                tar -C "$tmp" -xf "$data" || return 1
            fi
            ;;
        *.rpm)
            command -v rpm2cpio >/dev/null 2>&1 && command -v cpio >/dev/null 2>&1 || { log "libmpv   : 需要 rpm2cpio+cpio 解 .rpm"; return 1; }
            ( cd "$tmp" && rpm2cpio "$a" | cpio -idm --quiet ) || return 1
            ;;
        *.tar.*|*.tgz|*.tar)
            case "$a" in
                *.zst) command -v zstd >/dev/null 2>&1 || { log "libmpv   : 解 .zst 需要 zstd"; return 1; } ;;
            esac
            tar -C "$tmp" -xf "$a" || return 1
            ;;
        *.so|*.so.*)
            cp -f "$a" "$tmp/libmpv.so.2" || return 1
            ;;
        *) return 1 ;;
    esac
    found="$(find "$tmp" -type f -name 'libmpv.so.2*' 2>/dev/null | head -n1)"
    [ -n "$found" ] || { log "libmpv   : 归档里没有 libmpv.so.2*: $a"; return 1; }
    cp -f "$found" "$LIBMPV_CACHE/libmpv.so.2"
    rm -rf "$tmp"
    LIBMPV_SO="$LIBMPV_CACHE/libmpv.so.2"
}

# 下载或从发行版仓库拉取（成功则设 LIBMPV_SO）
libmpv_fetch() {
    local d="$LIBMPV_CACHE" f
    if [ -n "${LIBMPV_URL:-}" ]; then
        f="$d/$(basename "${LIBMPV_URL%%\?*}")"
        log "libmpv   : downloading $LIBMPV_URL"
        if command -v curl >/dev/null 2>&1; then
            curl -fL --retry 3 -o "$f" "$LIBMPV_URL" || return 1
        elif command -v wget >/dev/null 2>&1; then
            wget -O "$f" "$LIBMPV_URL" || return 1
        else
            log "libmpv   : 需要 curl 或 wget"
            return 1
        fi
        libmpv_unpack "$f"
        return
    fi
    if command -v apt-get >/dev/null 2>&1; then
        for pkg in libmpv2 libmpv1; do
            if ( cd "$d" && apt-get download "$pkg" >/dev/null 2>&1 ); then
                f="$(ls "$d/${pkg}_"*.deb 2>/dev/null | head -n1)"
                if [ -n "$f" ] && libmpv_unpack "$f"; then return 0; fi
            fi
        done
    fi
    if command -v dnf >/dev/null 2>&1; then
        if dnf download --destdir "$d" mpv-libs >/dev/null 2>&1; then
            f="$(ls "$d"/mpv-libs-*.rpm 2>/dev/null | head -n1)"
            if [ -n "$f" ] && libmpv_unpack "$f"; then return 0; fi
        fi
    fi
    if command -v pacman >/dev/null 2>&1; then
        if pacman -Sw --noconfirm --cachedir "$d" mpv >/dev/null 2>&1; then
            f="$(ls "$d"/mpv-[0-9]*.pkg.tar.* 2>/dev/null | head -n1)"
            if [ -n "$f" ] && libmpv_unpack "$f"; then return 0; fi
        fi
    fi
    return 1
}

# 默认不捆绑：应用内「设置 → 播放器」可一键下载（Windows）；
# Linux 用户通常已有系统 libmpv，也不建议塞一个依赖不匹配的 .so。
WANT_LIBMPV="${BUNDLE_LIBMPV:-0}"
[ -n "${LIBMPV_SO:-}" ] && WANT_LIBMPV=1
[ -n "${LIBMPV_ARCHIVE:-}" ] && WANT_LIBMPV=1
[ -n "${LIBMPV_URL:-}" ] && WANT_LIBMPV=1

if [ "$WANT_LIBMPV" != "1" ] || [ "${SKIP_LIBMPV:-0}" = "1" ]; then
    log "libmpv   : not bundled (default; set BUNDLE_LIBMPV=1 to bundle)"
else
    mkdir -p "$LIBMPV_CACHE"

    # 1) 已解压的库（缓存 / 本地放置）
    if [ -z "$LIBMPV_SO" ]; then
        libmpv_pick_from_dir "$LIBMPV_CACHE" \
            || libmpv_pick_from_dir "$ROOT/tv-rust/libmpv/lib" \
            || libmpv_pick_from_dir "$ROOT/tv-rust/libmpv" || true
    fi

    # 2) 本地已有归档
    if [ -z "$LIBMPV_SO" ]; then
        for c in "${LIBMPV_ARCHIVE:-}" "$LIBMPV_CACHE"/*.deb "$LIBMPV_CACHE"/*.rpm \
                 "$ROOT"/tv-rust/libmpv/*.deb "$ROOT"/tv-rust/libmpv/*.rpm; do
            if [ -n "$c" ] && [ -f "$c" ]; then
                libmpv_unpack "$c" && break
            fi
        done
    fi

    # 3) 下载 / 发行版仓库
    if [ -z "$LIBMPV_SO" ]; then
        libmpv_fetch || die "libmpv 获取失败（无网络或无 apt/dnf/pacman）。
可用 LIBMPV_SO=<libmpv.so.2> 直接指定，或 LIBMPV_ARCHIVE=<deb/rpm> 离线，或 SKIP_LIBMPV=1 跳过。"
    fi

    install -m 0644 "$LIBMPV_SO" "$OUT/libmpv.so.2"
    log "libmpv   : $LIBMPV_SO -> $OUT/libmpv.so.2"

    # 依赖闭包：把 libmpv 依赖的非基础库也带上（ffmpeg/libass/placebo 等）
    DEPDIR="$OUT/libmpv"
    rm -rf "$DEPDIR"; mkdir -p "$DEPDIR"
    DEP_COUNT=0
    while IFS= read -r lib; do
        [ -n "$lib" ] || continue
        base="$(basename "$lib")"
        case "$base" in
            linux-vdso*|ld-linux*|libc.so*|libm.so*|libdl.so*|libpthread.so*|librt.so*|\
            libresolv.so*|libutil.so*|libnsl.so*|libgcc_s.so*|libstdc++.so*|\
            libatomic.so*|libgomp.so*) continue ;;
        esac
        [ -f "$DEPDIR/$base" ] && continue
        if cp -Lf "$lib" "$DEPDIR/$base" 2>/dev/null; then DEP_COUNT=$((DEP_COUNT + 1)); fi
    done < <(ldd "$OUT/libmpv.so.2" 2>/dev/null | awk '/=> \//{print $3}')

    if [ "$DEP_COUNT" -gt 0 ]; then
        rpath_ok=0
        if command -v patchelf >/dev/null 2>&1; then
            if patchelf --set-rpath '$ORIGIN/libmpv' "$OUT/libmpv.so.2" 2>/dev/null; then rpath_ok=1; fi
        fi
        # 兜底：提供包装脚本，用 LD_LIBRARY_PATH 指向依赖目录
        cat > "$OUT/tv-rust.sh" <<'WRAP'
#!/bin/sh
here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
LD_LIBRARY_PATH="$here/libmpv${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" exec "$here/tv-rust" "$@"
WRAP
        chmod 0755 "$OUT/tv-rust.sh"
        if [ "$rpath_ok" = "1" ]; then
            LIBMPV_NOTE="bundled libmpv.so.2 + $DEP_COUNT deps (rpath \$ORIGIN/libmpv)"
        else
            LIBMPV_NOTE="bundled libmpv.so.2 + $DEP_COUNT deps (run ./tv-rust.sh; no patchelf)"
        fi
        log "libmpv   : deps $DEP_COUNT -> $DEPDIR"
    else
        LIBMPV_NOTE="bundled libmpv.so.2 (no extra deps)"
    fi
    log "libmpv   : $LIBMPV_NOTE"
fi

[ -f "$ROOT/LICENSE" ] && install -m 0644 "$ROOT/LICENSE" "$OUT/LICENSE.txt"

cat > "$OUT/version.txt" <<EOF
TVRust portable package (Linux)
version : $VERSION
built   : $(date '+%Y-%m-%d %H:%M:%S')
runtime : jlink $MODULES
arch    : $(uname -m)
libmpv  : $LIBMPV_NOTE

Run ./tv-rust. The bundled runtime under runtime/bin is used automatically;
no system-wide Java installation is required.
Data (settings/history/config cache) is stored in ~/.cache/TV-Multiplatform.
EOF

# ---------- 3) 打包 ----------
ARCH_TAG="${ARCH_TAG:-linux-$(uname -m)}"
ZIP="$ROOT/dist/TVRust-${VERSION}-${ARCH_TAG}.tar.gz"
mkdir -p "$(dirname "$ZIP")"
rm -f "$ZIP"
log "tar      : $ZIP"
tar -C "$OUT" --owner=0 --group=0 -czf "$ZIP" .

log ""
log "done:"
log "  dir : $OUT"
log "  tar : $ZIP ($(du -h "$ZIP" | cut -f1))"

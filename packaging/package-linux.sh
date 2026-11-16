#!/usr/bin/env bash
# 构建 Linux 便携包（tar.gz）。
#
# 必须在 Linux 上运行：jlink 只能生成“当前平台”的运行时，无法交叉生成。
# 依赖：bash、cargo、JDK 25（JAVA_HOME，需包含 jlink）
#
# 用法：
#   ./packaging/package-linux.sh
#   OUT=/tmp/out VERSION=0.1.0 TARGET=x86_64-unknown-linux-gnu ./packaging/package-linux.sh
#
# 产物目录结构（与 Windows 包保持一致，Rust 侧自动识别）：
#   TVRust-linux-x64/
#     tv-rust                主程序
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
[ -f "$JAR" ] || die "缺少 spider-host jar: $JAR（先执行 gradlew :spider-host:fatJar）"
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
[ -f "$ROOT/LICENSE" ] && install -m 0644 "$ROOT/LICENSE" "$OUT/LICENSE.txt"

cat > "$OUT/version.txt" <<EOF
TVRust portable package (Linux)
version : $VERSION
built   : $(date '+%Y-%m-%d %H:%M:%S')
runtime : jlink $MODULES
arch    : $(uname -m)

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

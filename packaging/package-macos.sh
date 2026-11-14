#!/usr/bin/env bash
# 构建 macOS 应用包（TVRust.app + dmg）。
#
# 必须在 macOS 上运行：jlink 只能生成当前平台运行时，且 .app 的签名/公证只有 macOS 提供。
# 依赖：bash、cargo、JDK 21（JAVA_HOME，需包含 jlink）、Xcode 命令行工具（codesign/hdiutil）
#
# 用法：
#   ./packaging/package-macos.sh
#   VERSION=0.1.0 SIGN_IDENTITY="Developer ID Application: ..." ./packaging/package-macos.sh
#
# 目录结构（jar 与 runtime 放在 MacOS/ 下，Rust 侧按“exe 同级目录”自动发现，无需改代码）：
#   TVRust.app/Contents/
#     Info.plist
#     MacOS/TVRust
#     MacOS/spider-host-all.jar
#     MacOS/runtime/bin/java
#     Resources/icon.icns        （存在则打包）

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT="${OUT:-$ROOT/dist/TVRust-macos}"
JAR="${JAR:-$ROOT/spider-host/build/libs/spider-host-all.jar}"
TARGET="${TARGET:-}"
MODULES_FILE="$ROOT/packaging/common/jlink-modules.txt"

VERSION="${VERSION:-$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$ROOT/tv-rust/Cargo.toml" | head -n1)}"
VERSION="${VERSION:-0.0.0}"

log() { printf '%s\n' "$*"; }
die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Darwin" ] || die "此脚本只能在 macOS 上运行（jlink 不能交叉生成运行时）"
[ -f "$MODULES_FILE" ] || die "缺少模块清单: $MODULES_FILE"
[ -f "$JAR" ] || die "缺少 spider-host jar: $JAR（先执行 gradlew :spider-host:fatJar）"
[ -n "${JAVA_HOME:-}" ] || die "未设置 JAVA_HOME（需要 JDK 21，且包含 jlink）"
JLINK="$JAVA_HOME/bin/jlink"
[ -x "$JLINK" ] || die "找不到 jlink: $JLINK"

MODULES="$(grep -v '^[[:space:]]*#' "$MODULES_FILE" | tr -d ' \r' | grep -v '^$' | paste -sd, -)"
[ -n "$MODULES" ] || die "模块清单为空: $MODULES_FILE"

HOST_ARCH="$(uname -m)"          # arm64 / x86_64
log "== TVRust macOS 应用包 =="
log "repo   : $ROOT"
log "output : $OUT"
log "version: $VERSION"
log "arch   : $HOST_ARCH"
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

# ---------- 2) 组装 .app ----------
APP="$OUT/TVRust.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

log "jlink    : $MODULES"
"$JLINK" --add-modules "$MODULES" \
    --strip-debug --no-header-files --no-man-pages --compress=zip-6 \
    --output "$APP/Contents/MacOS/runtime"
[ -x "$APP/Contents/MacOS/runtime/bin/java" ] || die "jlink 运行时缺少 bin/java"

install -m 0755 "$BIN" "$APP/Contents/MacOS/TVRust"
install -m 0644 "$JAR" "$APP/Contents/MacOS/spider-host-all.jar"

ICON="$ROOT/packaging/common/icon.icns"
if [ -f "$ICON" ]; then
    install -m 0644 "$ICON" "$APP/Contents/Resources/icon.icns"
    ICON_KEY='<key>CFBundleIconFile</key><string>icon.icns</string>'
else
    ICON_KEY=''
fi

cat > "$APP/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>TVRust</string>
    <key>CFBundleDisplayName</key><string>TV Rust</string>
    <key>CFBundleIdentifier</key><string>dev.tvmultiplatform.tvrrust</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleExecutable</key><string>TVRust</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>LSMinimumSystemVersion</key><string>11.0</string>
    <key>NSHighResolutionCapable</key><true/>
    $ICON_KEY
</dict>
</plist>
EOF

cat > "$OUT/version.txt" <<EOF
TVRust portable package (macOS)
version : $VERSION
built   : $(date '+%Y-%m-%d %H:%M:%S')
runtime : jlink $MODULES
arch    : $HOST_ARCH

Open TVRust.app. The bundled runtime under Contents/MacOS/runtime is used
automatically; no system-wide Java installation is required.
Data (settings/history/config cache) is stored in ~/Library/Caches/TV-Multiplatform.
EOF

# ---------- 3) 签名（默认 ad-hoc，可用 SIGN_IDENTITY 覆盖） ----------
SIGN_IDENTITY="${SIGN_IDENTITY:--}"
if command -v codesign >/dev/null 2>&1; then
    log "codesign : $SIGN_IDENTITY"
    codesign --force --deep --sign "$SIGN_IDENTITY" "$APP" || die "codesign 失败"
else
    log "codesign : 未找到 codesign，跳过（产物只能在本机运行）"
fi

# ---------- 4) 打包 dmg ----------
ARCH_TAG="${ARCH_TAG:-macos-$HOST_ARCH}"
DMG="$ROOT/dist/TVRust-${VERSION}-${ARCH_TAG}.dmg"
mkdir -p "$(dirname "$DMG")"
rm -f "$DMG"
if command -v hdiutil >/dev/null 2>&1; then
    log "dmg      : $DMG"
    hdiutil create -volname "TVRust" -srcfolder "$OUT" -ov -format UDZO "$DMG" >/dev/null
    PACKAGED="$DMG ($(du -h "$DMG" | cut -f1))"
else
    ZIP="$ROOT/dist/TVRust-${VERSION}-${ARCH_TAG}.zip"
    rm -f "$ZIP"
    log "zip      : $ZIP（未找到 hdiutil）"
    ditto -c -k --sequesterRsrc --keepParent "$OUT" "$ZIP"
    PACKAGED="$ZIP"
fi

log ""
log "done:"
log "  app : $APP"
log "  pkg : $PACKAGED"
log ""
log "提示：对外分发需要 Developer ID 签名 + 公证（notarytool），否则用户首次打开会被 Gatekeeper 拦截。"

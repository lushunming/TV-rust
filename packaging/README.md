# 打包说明（Windows / macOS / Linux）

三个平台各出一个便携包，内部结构一致：**主程序 + `spider-host-all.jar` + jlink 运行时**。
Rust 侧按"exe 同级目录"自动查找 jar 与 `runtime/bin/java`，所以三种包都不需要用户安装 Java。

| 平台 | 脚本 | 产物 |
|---|---|---|
| Windows x64 | `package-windows.ps1` | `dist/TVRust-<ver>-win-x64.zip` |
| macOS (arm64/x64) | `package-macos.sh` | `dist/TVRust-<ver>-macos-<arch>.dmg`（内含 `TVRust.app`） |
| Linux x64 | `package-linux.sh` | `dist/TVRust-<ver>-linux-<arch>.tar.gz` |

## 重要前提：jlink 不能交叉

`jlink` 只能生成**运行它的那个平台**的运行时。因此：

- Windows 包必须在 Windows 上打；
- macOS 包必须在 macOS 上打（签名/公证同理）；
- Linux 包必须在 Linux 上打（或 Linux 容器）。

跨平台只能靠 CI 矩阵（见 `.github/workflows/package.yml`）或各自机器，不能用一台机器全部产出。

## 公共模块清单

`packaging/common/jlink-modules.txt` 是三平台唯一来源，改这里三边同时生效。**不要删减**：

- `jdk.crypto.ec`：ECDHE 握手，缺了所有 HTTPS 请求静默失败（表现为爬虫 init 报 `Not a JSON Object: null`、首页空白）；
- `jdk.charsets`：GBK/GB2312 解码，缺了中文站点解析失败。

## Windows

```powershell
powershell -ExecutionPolicy Bypass -File packaging\package-windows.ps1 -Jdk "D:\devtools\corretto-21"
# 只换 jar / 只重打 zip：
powershell -ExecutionPolicy Bypass -File packaging\package-windows.ps1 -SkipCargoBuild -SkipHostJar
# 改了模块清单，需要重建运行时：
powershell -ExecutionPolicy Bypass -File packaging\package-windows.ps1 -SkipCargoBuild -SkipHostJar -RecreateRuntime
```

参数：`-Output`、`-Jdk`、`-Version`、`-GradleCmd`、`-SkipHostJar`、`-SkipCargoBuild`、`-RecreateRuntime`。

## macOS

```bash
JAVA_HOME=$(/usr/libexec/java_home -v 21) ./packaging/package-macos.sh
# 正式分发（Developer ID + 后续公证）：
SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" ./packaging/package-macos.sh
```

- 结构：`TVRust.app/Contents/MacOS/{TVRust, spider-host-all.jar, runtime/}`；
  jar 与 runtime 放在 `MacOS/` 是为了让 Rust 侧"exe 同级目录"直接命中，无需改代码；
- 图标：放 `packaging/common/icon.icns` 会自动写入 `Contents/Resources` 与 `Info.plist`；
- 默认 ad-hoc 签名（`-`），只能本机运行；对外分发必须 Developer ID 签名 + `notarytool` 公证，否则 Gatekeeper 拦截。

## Linux

```bash
JAVA_HOME=/usr/lib/jvm/temurin-21-jdk ./packaging/package-linux.sh
```

- 建议在较老的发行版（如 Ubuntu 22.04）构建，避免 glibc 版本过高导致老系统跑不起来；
- 运行期依赖：GL（`libGL.so.1`）、`libxkbcommon`，以及 X11/Wayland 之一的桌面会话；
- `runtime/bin/java` 与 `tv-rust` 保持 0755 可执行位（脚本已处理）。

## CI

`.github/workflows/package.yml` 提供三平台矩阵：各自 `cargo build --release` + `jlink` + 打包，产物以 artifact 上传。
触发方式：手动 `workflow_dispatch`，或推送 `v*` tag。

## 数据目录

| 平台 | 位置 |
|---|---|
| Windows | `%AppData%\TV-Multiplatform\cache` |
| macOS | `~/Library/Caches/TV-Multiplatform` |
| Linux | `~/.cache/TV-Multiplatform` |

日志：`cache/rust/tv-rust.log`、`cache/rust/spider-host.log`、`cache/log/spider-host.log`。

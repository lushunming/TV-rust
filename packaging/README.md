# 打包说明（Windows / macOS / Linux）

三个平台各出一个便携包，内部结构一致：**主程序 + `spider-host-all.jar` + jlink 运行时**。
Rust 侧按"exe 同级目录"自动查找 jar 与 `runtime/bin/java`，所以三种包都不需要用户安装 Java。

**内置播放器依赖 libmpv，但默认不随包分发**：Windows 的 `mpv-2.dll` 有 118 MB，而多数用户用不到全部解码器。
应用内「设置 → 播放器 → 下载内置播放器组件」会从上游官方构建拉取（约 33 MB）并解包到用户缓存目录；
需要离线/内网分发时，再用 `-BundleLibMpv`（Windows）或 `BUNDLE_LIBMPV=1`（Linux）显式捆绑。

应用内下载会走 GitHub 加速镜像：按 `gitproxy → gh-proxy → ghfast → ghproxy.net` 依次探活，
用第一个通的下载；某个镜像下载失败会自动往后回退，全部失败才回退直连 `github.com`。
实测（同一网络）直连 33 MB 需 ~407 秒，经镜像 32 秒。

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
powershell -ExecutionPolicy Bypass -File packaging\package-windows.ps1 -Jdk "D:\devtools\corretto-25"
# 只换 jar / 只重打 zip：
powershell -ExecutionPolicy Bypass -File packaging\package-windows.ps1 -SkipCargoBuild -SkipHostJar
# 改了模块清单，需要重建运行时：
powershell -ExecutionPolicy Bypass -File packaging\package-windows.ps1 -SkipCargoBuild -SkipHostJar -RecreateRuntime
```

参数：`-Output`、`-Jdk`、`-Version`、`-GradleCmd`、`-SkipHostJar`、`-SkipCargoBuild`、`-RecreateRuntime`、`-BundleLibMpv`、`-LibMpv`、`-LibMpvArchive`、`-LibMpvUrl`、`-LibMpvTag`、`-SkipLibMpv`、`-SkipZip`。

### libmpv（Windows）

**默认不捆绑**（`mpv-2.dll` 约 118 MB，超过 GitHub 单文件 100 MB 上限，且属于第三方构建产物）。
包体积因此从 ~142 MB 降到 ~95 MB，用户首次使用内置播放器时在应用内一键下载（约 33 MB）。

默认行为：**跳过 libmpv**（不下载、不拷贝，包大小 ~95 MB）。
应用内点「下载内置播放器组件」即可，无需任何本地准备。

需要把 libmpv 打进包（离线部署/内网）时，才走下面的解析：

1. `-LibMpv <dll>` 显式指定
2. `tv-rust\libmpv\cache\libmpv-2.dll`（上次下载的缓存）
3. `tv-rust\libmpv\dev\libmpv-2.dll`、`tv-rust\libmpv\libmpv-2.dll`、`tv-rust\libmpv\mpv-2.dll`、`dist\mpv\mpv-2.dll`（本地已有）
4. 本地压缩包 `tv-rust\libmpv\mpv-dev.7z` 或 `tv-rust\libmpv\cache\mpv-dev.7z`
5. `-LibMpvArchive <7z>` 指定的压缩包
6. `-LibMpvUrl <url>` 指定的下载地址
7. 查 GitHub API 取 `zhongfly/mpv-winbuild` 最新 release 的 `mpv-dev-x86_64-*.7z`（可用 `-LibMpvTag <tag>` 锁定版本）

下载的压缩包存为 `tv-rust\libmpv\cache\mpv-dev.7z`，再从里面抽出 `libmpv-2.dll`；最终**改名复制为 `mpv-2.dll` 放到包根目录**（Rust 侧按 exe 同级查找）。

```powershell
# 默认：不捆绑 libmpv（应用内按需下载）
powershell -ExecutionPolicy Bypass -File packaging\package-windows.ps1

# 捆绑 libmpv（离线/内网部署）
powershell -ExecutionPolicy Bypass -File packaging\package-windows.ps1 -BundleLibMpv

# 离线：用已经下好的压缩包（隐含 -BundleLibMpv）
powershell -ExecutionPolicy Bypass -File packaging\package-windows.ps1 -LibMpvArchive D:\mpv-dev-x86_64-20261124-git-35efcf53d5.7z

# 锁定 mpv 版本，避免每次构建拿到不同版本
powershell -ExecutionPolicy Bypass -File packaging\package-windows.ps1 -BundleLibMpv -LibMpvTag 2026-11-24-35efcf53d5

# 用已解压的 dll
powershell -ExecutionPolicy Bypass -File packaging\package-windows.ps1 -LibMpv D:\mpv\libmpv-2.dll

# 只验证组装、不生成 zip
powershell -ExecutionPolicy Bypass -File packaging\package-windows.ps1 -SkipHostJar -SkipCargoBuild -SkipZip
```

依赖与失败行为：

- 解包 7z 需要 **7-Zip**（Windows 自带 `tar.exe` 不支持 LZMA，会报 `LZMA codec is unsupported`）；脚本会依次查 `%ProgramW6432%` / `%ProgramFiles%` / `%ProgramFiles(x86)%` / 注册表 `HKLM\SOFTWARE\7-Zip`，都没有则报错并提示改用 `-LibMpv`。
- 最终仍拿不到 dll 时脚本**直接报错退出**，不会打出一个"内置播放器不可用"的包。
- 下载走 `https://api.github.com` + `https://github.com`，被墙/内网时用 `-LibMpvArchive` 离线路径。

## macOS

```bash
JAVA_HOME=$(/usr/libexec/java_home -v 25) ./packaging/package-macos.sh
# 正式分发（Developer ID + 后续公证）：
SIGN_IDENTITY="Developer ID Application: Your Name (TEAMID)" ./packaging/package-macos.sh
```

- 结构：`TVRust.app/Contents/MacOS/{TVRust, spider-host-all.jar, runtime/}`；
  jar 与 runtime 放在 `MacOS/` 是为了让 Rust 侧"exe 同级目录"直接命中，无需改代码；
- libmpv：`LIBMPV_DYLIB=/opt/homebrew/lib/libmpv.dylib ./packaging/package-macos.sh` 可把 `libmpv.dylib` 放进 `Contents/MacOS/`；
  不指定时用系统/Homebrew 的 libmpv；
- 图标：放 `packaging/common/icon.icns` 会自动写入 `Contents/Resources` 与 `Info.plist`；
- 默认 ad-hoc 签名（`-`），只能本机运行；对外分发必须 Developer ID 签名 + `notarytool` 公证，否则 Gatekeeper 拦截。

## Linux

```bash
JAVA_HOME=/usr/lib/jvm/temurin-25-jdk ./packaging/package-linux.sh
# 显式捆绑 libmpv（连同依赖库一起打进包）
BUNDLE_LIBMPV=1 LIBMPV_ARCHIVE=/root/pkgs/libmpv2_0.42.0-1ubuntu1_amd64.deb ./packaging/package-linux.sh
```

**默认不捆绑 libmpv**：Linux 用户基本都装了 `mpv`（Debian/Ubuntu/Fedora/Arch 都有），
包内不带 `libmpv.so.2`，体积小 118 MB。应用启动时自动找系统的 `libmpv.so.2`。

需要捆绑时（`BUNDLE_LIBMPV=1` 或指定 `LIBMPV_SO` / `LIBMPV_ARCHIVE` / `LIBMPV_URL` 任一），获取顺序：

1. `LIBMPV_SO=<libmpv.so.2>` 显式指定已解压的库
2. `tv-rust/libmpv/cache/libmpv.so.2`（上次抽取的缓存）
3. `tv-rust/libmpv/lib/libmpv.so.2`、`tv-rust/libmpv/libmpv.so.2`
4. `LIBMPV_ARCHIVE=<deb/rpm>` 或缓存目录/`tv-rust/libmpv/` 下的 `.deb`/`.rpm`
5. `LIBMPV_URL=<url>`（.deb / .rpm / .tar.xz / 裸 .so）
6. 发行版仓库：`apt-get download libmpv2`（次选 `libmpv1`）→ `dnf download mpv-libs` → `pacman -Sw mpv`

**为什么 Linux 不能只拷一个 `libmpv.so.2`**：它是动态链接的（依赖 ffmpeg/libass/libplacebo 等）。脚本在拷完后会跑 `ldd`，
把非基础库（排除 glibc/libstdc++/libm 等）一并复制到包内 `libmpv/` 目录，然后：

- 有 `patchelf` → 给 `libmpv.so.2` 设 `rpath=$ORIGIN/libmpv`，直接 `./tv-rust` 即可；
- 无 `patchelf` → 包内附 `tv-rust.sh` 包装脚本，用 `LD_LIBRARY_PATH` 指向 `libmpv/`，运行 `./tv-rust.sh`。

其他要点：

- 建议在较老的发行版（如 Ubuntu 22.04）构建，避免 glibc 版本过高导致老系统跑不起来；
- 解包依赖：`.deb` 优先用 `dpkg-deb`（Debian/Ubuntu 构建机自带，可处理 `data.tar.zst`）；无则用 `ar`+`tar`，此时 `.zst` 需装 `zstd`；`.rpm` 需 `rpm2cpio`+`cpio`；
- 运行期依赖：GL（`libGL.so.1`）、`libxkbcommon`，以及 X11/Wayland 之一的桌面会话；
- `runtime/bin/java` 与 `tv-rust` 保持 0755 可执行位（脚本已处理）；
- 获取失败会直接 `die`（不会静默出发一个内置播放器不可用的包），`SKIP_LIBMPV=1` 仍可强制跳过。

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

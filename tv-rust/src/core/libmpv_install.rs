//! 内置播放器组件（libmpv）按需下载安装。
//!
//! 打包默认不再携带 libmpv：Windows 的 `mpv-2.dll` 有 118 MB，而绝大多数用户
//! 用不到全部解码器。首次使用内置播放器时，在「设置 → 播放器」点一下下载即可。
//!
//! 流程：
//! 1. 查 GitHub Release（zhongfly/mpv-winbuild）拿最新的 `mpv-dev-x86_64-*.7z`
//! 2. 流式下载到缓存目录，边下边把进度回传给 UI
//! 3. 用纯 Rust 的 sevenz 解包，取出 `libmpv-2.dll`
//! 4. 落到 `<cache>/libmpv/mpv-2.dll`，随后 `detect_libmpv()` 自动命中
//!
//! 之所以不用上游自己那套解包：应用内不能依赖 7-Zip，且不能要求用户装任何东西。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};

const USER_AGENT: &str = "TVRust/0.2 (+https://github.com/zhongfly/mpv-winbuild)";
const RELEASE_API: &str = "https://api.github.com/repos/zhongfly/mpv-winbuild/releases/latest";

/// GitHub 加速镜像（前缀式代理：把原始 GitHub URL 直接拼在后面）。
/// 数组顺序就是优先级：依次探测，用第一个通的；全都不通则回退直连。
struct Mirror {
    name: &'static str,
    base: &'static str,
    ping: &'static str,
}

const MIRRORS: &[Mirror] = &[
    Mirror {
        name: "gitproxy",
        base: "https://api.gitproxy.dev/",
        ping: "https://api.gitproxy.dev/https://raw.githubusercontent.com/octocat/Hello-World/master/README",
    },
    Mirror {
        name: "gh-proxy",
        base: "https://gh-proxy.com/",
        ping: "https://gh-proxy.com/https://raw.githubusercontent.com/octocat/Hello-World/master/README",
    },
    Mirror {
        name: "ghfast",
        base: "https://ghfast.top/",
        ping: "https://ghfast.top/https://raw.githubusercontent.com/octocat/Hello-World/master/README",
    },
    Mirror {
        name: "ghproxy.net",
        base: "https://ghproxy.net/",
        ping: "https://ghproxy.net/https://raw.githubusercontent.com/octocat/Hello-World/master/README",
    },
];

/// 镜像探测超时（探测只拉一个十几字节的文件，超时就当不可用）
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// 下载状态机（UI 直接按这个渲染）
#[derive(Clone, Debug, PartialEq)]
pub enum InstallState {
    /// 未开始
    Idle,
    /// 正在查最新 release
    Resolving,
    /// 正在下载
    Downloading { got: u64, total: u64 },
    /// 正在解包
    Extracting,
    /// 已安装（值为 dll 路径）
    Done(String),
    /// 失败（值为原因）
    Failed(String),
    /// 用户取消
    Cancelled,
}

impl InstallState {
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            InstallState::Resolving | InstallState::Downloading { .. } | InstallState::Extracting
        )
    }
}

/// libmpv 的动态库文件名（Windows 用带 `-2` 的名字，与 mpv 官方构建一致）
pub fn dll_name() -> &'static str {
    #[cfg(windows)]
    {
        "mpv-2.dll"
    }
    #[cfg(not(windows))]
    {
        "libmpv.so.2"
    }
}

/// 安装目录：`<缓存根>/libmpv/`
///
/// 放在用户缓存目录而不是安装目录，避免写 `Program Files` 触发 UAC。
pub fn install_dir() -> PathBuf {
    crate::core::paths::cache_root().join("libmpv")
}

/// 已安装的 dll（没有则 None）
pub fn installed_dll() -> Option<PathBuf> {
    let p = install_dir().join(dll_name());
    if p.is_file() { Some(p) } else { None }
}

// ========== 下载 + 安装（阻塞实现，跑在后台线程里）==========

/// 阻塞式安装：解析地址 → 下载 → 解包 → 落盘。
///
/// `progress` 会被反复调用来推送状态，`cancel` 置位后会尽快中止。
pub fn install_blocking(
    progress: &dyn Fn(InstallState),
    cancel: &AtomicBool,
) -> Result<PathBuf> {
    #[cfg(not(windows))]
    {
        let _ = (progress, cancel);
        return Err(anyhow!(
            "当前平台请用系统包管理器安装 mpv（如 apt install libmpv2 / brew install mpv）"
        ));
    }

    #[cfg(windows)]
    {
        let client = reqwest::blocking::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(120))
            .build()
            .context("创建 HTTP 客户端失败")?;

        let dir = install_dir();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("创建目录失败: {}", dir.display()))?;
        let archive = dir.join("mpv-dev.7z");

        // 已下载完整包则直接复用：解包失败重试时不必再拉 33 MB
        let cached = std::fs::metadata(&archive)
            .map(|m| m.len() > 20_000_000)
            .unwrap_or(false);
        if cached {
            crate::core::log::info("libmpv: 复用已下载的 mpv-dev.7z");
        } else {
            progress(InstallState::Resolving);
            let (url, size, tag) = resolve_asset(&client)?;
            crate::core::log::info(format!(
                "libmpv 下载: release={} asset={} size={} MB",
                tag,
                url.rsplit('/').next().unwrap_or(&url),
                size as f64 / 1024.0 / 1024.0
            ));
            progress(InstallState::Downloading { got: 0, total: size });
            download_with_mirrors(&client, &url, &archive, size, progress, cancel)?;
            if cancel.load(Ordering::SeqCst) {
                let _ = std::fs::remove_file(&archive);
                return Err(anyhow!("cancelled"));
            }
        }

        progress(InstallState::Extracting);
        let tmp = dir.join("_extract");
        let _ = std::fs::remove_dir_all(&tmp);
        if let Err(e) = sevenz_rust::decompress_file(&archive, &tmp) {
            // 包坏了就别留着，下次重新下
            let _ = std::fs::remove_file(&archive);
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(anyhow!(e)).context("解压 mpv-dev 包失败");
        }

        // 官方包里叫 `libmpv-2.dll`，随包分发时才改名 `mpv-2.dll`，两个名字都认
        let dll = find_file(&tmp, dll_name())
            .or_else(|| find_file(&tmp, "libmpv-2.dll"))
            .or_else(|| find_file(&tmp, "mpv-2.dll"))
            .ok_or_else(|| anyhow!("压缩包里没有 libmpv-2.dll"));
        let dll = match dll {
            Ok(p) => p,
            Err(e) => {
                let _ = std::fs::remove_file(&archive);
                let _ = std::fs::remove_dir_all(&tmp);
                return Err(e);
            }
        };

        let target = dir.join(dll_name());
        std::fs::copy(&dll, &target)
            .with_context(|| format!("写入失败: {}", target.display()))?;

        // 清理临时解包目录与下载的分卷（118MB，别留在磁盘上）
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_file(&archive);

        let bytes = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
        crate::core::log::info(format!(
            "libmpv 安装完成: {} ({} bytes)",
            target.display(),
            bytes
        ));
        Ok(target)
    }
}

/// 查最新 release，返回 (下载地址, 字节数, tag)
fn resolve_asset(client: &reqwest::blocking::Client) -> Result<(String, u64, String)> {
    let rel: serde_json::Value = client
        .get(RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .context("请求 GitHub Release 失败（网络不通或被限流）")?
        .error_for_status()
        .context("GitHub 返回错误状态")?
        .json()
        .context("解析 release JSON 失败")?;

    let tag = rel["tag_name"].as_str().unwrap_or("unknown").to_string();
    let assets = rel["assets"]
        .as_array()
        .ok_or_else(|| anyhow!("GitHub 返回格式异常：没有 assets"))?;

    let asset = assets
        .iter()
        .find(|a| {
            a["name"]
                .as_str()
                .map(|n| n.starts_with("mpv-dev-x86_64-") && n.ends_with(".7z"))
                .unwrap_or(false)
        })
        .ok_or_else(|| anyhow!("release {} 里没有 mpv-dev-x86_64-*.7z", tag))?;

    let url = asset["browser_download_url"]
        .as_str()
        .ok_or_else(|| anyhow!("资源缺少下载地址"))?
        .to_string();
    let size = asset["size"].as_u64().unwrap_or(0);
    Ok((url, size, tag))
}

/// 按已配置的镜像优先级下载：探活 → 用第一个通的下载 → 失败自动往后回退 → 最后直连。
///
/// 返回实际使用的下载源名称（用于日志）。
fn download_with_mirrors(
    client: &reqwest::blocking::Client,
    direct_url: &str,
    dest: &Path,
    size_hint: u64,
    progress: &dyn Fn(InstallState),
    cancel: &AtomicBool,
) -> Result<String> {
    let mut candidates: Vec<(String, String)> = Vec::new();
    for m in MIRRORS {
        if cancel.load(Ordering::SeqCst) {
            return Err(anyhow!("cancelled"));
        }
        if mirror_alive(client, m.ping) {
            crate::core::log::info(format!("libmpv: 镜像 {} 可用", m.name));
            candidates.push((m.name.to_string(), format!("{}{}", m.base, direct_url)));
        } else {
            crate::core::log::info(format!("libmpv: 镜像 {} 不可用，跳过", m.name));
        }
    }
    // 兜底：直连原始 GitHub 地址（镜像全部不可用或全部下载失败时）
    candidates.push(("direct".to_string(), direct_url.to_string()));

    let mut last_err: Option<anyhow::Error> = None;
    for (name, url) in candidates {
        if cancel.load(Ordering::SeqCst) {
            return Err(anyhow!("cancelled"));
        }
        crate::core::log::info(format!("libmpv: 尝试下载源 {name}"));
        progress(InstallState::Downloading { got: 0, total: size_hint });
        match download(client, &url, dest, size_hint, progress, cancel) {
            Ok(()) => {
                crate::core::log::info(format!("libmpv: 下载完成（源 {name}）"));
                return Ok(name);
            }
            Err(e) => {
                if cancel.load(Ordering::SeqCst) {
                    return Err(anyhow!("cancelled"));
                }
                crate::core::log::warn(format!("libmpv: 源 {name} 失败: {e}，尝试下一个"));
                let _ = std::fs::remove_file(dest);
                last_err = Some(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow!("没有可用的下载源")))
}

/// 镜像探活：拉一个十几字节的小文件，2xx 即认为可用
fn mirror_alive(client: &reqwest::blocking::Client, url: &str) -> bool {
    client
        .get(url)
        .timeout(PROBE_TIMEOUT)
        .send()
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// 流式下载（每 150ms 回报一次进度，避免刷爆 UI）
fn download(
    client: &reqwest::blocking::Client,
    url: &str,
    dest: &Path,
    size_hint: u64,
    progress: &dyn Fn(InstallState),
    cancel: &AtomicBool,
) -> Result<()> {
    let mut resp = client.get(url).send().context("下载请求失败")?;
    if !resp.status().is_success() {
        return Err(anyhow!("下载失败: HTTP {}", resp.status()));
    }
    let total = resp.content_length().unwrap_or(size_hint);

    let mut file = std::fs::File::create(dest)
        .with_context(|| format!("创建临时文件失败: {}", dest.display()))?;
    let mut buf = vec![0u8; 128 * 1024];
    let mut got: u64 = 0;
    let mut last = Instant::now();

    loop {
        if cancel.load(Ordering::SeqCst) {
            drop(file);
            let _ = std::fs::remove_file(dest);
            return Err(anyhow!("cancelled"));
        }
        let n = resp.read(&mut buf).context("读取响应流失败")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).context("写入临时文件失败")?;
        got += n as u64;
        if last.elapsed() >= Duration::from_millis(150) {
            progress(InstallState::Downloading { got, total });
            last = Instant::now();
        }
    }
    file.flush().ok();
    progress(InstallState::Downloading { got, total });

    if total > 0 && got != total {
        let _ = std::fs::remove_file(dest);
        return Err(anyhow!("下载不完整：{}/{} 字节", got, total));
    }
    Ok(())
}

/// 递归找文件名匹配的文件（7z 内可能是 `libmpv-2.dll` 或放在子目录里）
fn find_file(root: &Path, name: &str) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.eq_ignore_ascii_case(name))
                .unwrap_or(false)
            {
                return Some(p);
            }
        }
    }
    None
}

// ========== UI 侧状态holder ==========

/// 下载器：持有状态机 + 后台线程通道，由 `TVApp::update` 每帧 poll。
pub struct LibMpvInstaller {
    state: InstallState,
    rx: Option<Receiver<InstallState>>,
    cancel: Arc<AtomicBool>,
}

impl Default for LibMpvInstaller {
    fn default() -> Self {
        Self::new()
    }
}

impl LibMpvInstaller {
    pub fn new() -> Self {
        Self {
            state: InstallState::Idle,
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn state(&self) -> InstallState {
        self.state.clone()
    }

    /// 启动后台下载（重复点击无效）
    pub fn start(&mut self, ctx: &egui::Context) {
        if self.state.is_busy() {
            return;
        }
        self.cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        self.rx = Some(rx);
        self.state = InstallState::Resolving;

        let cancel = self.cancel.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let tx_progress = tx.clone();
            let ctx_progress = ctx.clone();
            let progress = move |s: InstallState| {
                let _ = tx_progress.send(s);
                ctx_progress.request_repaint();
            };
            let result = install_blocking(&progress, &cancel);
            let final_state = match result {
                Ok(p) => InstallState::Done(p.to_string_lossy().into_owned()),
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("cancelled") {
                        InstallState::Cancelled
                    } else {
                        crate::core::log::warn(format!("libmpv 安装失败: {msg}"));
                        InstallState::Failed(msg)
                    }
                }
            };
            let _ = tx.send(final_state);
            ctx.request_repaint();
        });
    }

    /// 请求取消（后台线程在下一个数据块处退出）
    pub fn cancel(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// 每帧调用：抽干通道里的状态
    pub fn poll(&mut self) {
        let mut latest: Option<InstallState> = None;
        if let Some(rx) = &self.rx {
            while let Ok(s) = rx.try_recv() {
                let finished = !s.is_busy();
                latest = Some(s);
                if finished {
                    break;
                }
            }
        }
        if let Some(s) = latest {
            self.state = s;
        }
        if !self.state.is_busy() {
            self.rx = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_dir_is_under_cache() {
        let d = install_dir();
        assert!(d.ends_with("libmpv"), "{}", d.display());
    }

    /// 已安装时应能被 detect_libmpv() 直接命中（不联网）
    #[test]
    fn detect_finds_installed_dll() {
        if installed_dll().is_some() {
            let p = crate::core::settings::detect_libmpv();
            println!("detect_libmpv -> {:?}", p);
            assert!(p.is_some(), "已安装但 detect_libmpv 未命中");
        }
    }

    /// 真实下载 + 解包（默认忽略，本地手动跑一次即可）
    /// `cargo test --release download_and_install_libmpv -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn download_and_install_libmpv() {
        let progress = |s: InstallState| match s {
            InstallState::Downloading { got, total } => {
                println!("下载中 {}/{} MB", got / 1_000_000, total / 1_000_000);
            }
            other => println!("{:?}", other),
        };
        let cancel = AtomicBool::new(false);
        let path = install_blocking(&progress, &cancel).expect("安装失败");
        let size = std::fs::metadata(&path).unwrap().len();
        println!("已安装: {} ({} MB)", path.display(), size / 1_000_000);
        assert!(size > 50_000_000, "dll 太小，可疑: {size}");
    }
}

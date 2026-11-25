//! 应用设置（Rust 侧自有存储 settings.json）+ 播放器/宿主自动探测

use crate::core::paths;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// 配置源列表（http URL / 本地文件路径 / 内联 JSON）
    pub config_sources: Vec<String>,
    /// 当前使用的配置源下标
    pub active_source: Option<usize>,
    /// 外部播放器可执行文件路径
    pub player_path: String,
    /// java 可执行文件路径（默认取 PATH 中的 java）
    pub java_path: String,
    /// spider-host fat jar 路径
    pub host_jar: Option<String>,
    /// 每批搜索的站点数
    pub search_batch: usize,
    /// 宿主 JVM 最大堆（MB）
    pub host_memory_mb: u32,
    /// 上次使用的站源 key（重启后恢复，避免总是回到配置里的第一个站）
    pub last_site_key: Option<String>,
    /// 内置播放器默认音量 0-100
    pub internal_volume: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            config_sources: Vec::new(),
            active_source: None,
            player_path: String::new(),
            java_path: "java".into(),
            host_jar: None,
            search_batch: 2,
            // JDK 25（LTS）下 G1 + 字符串去重让同样负载的堆占用更可控，512m 足够跑爬虫宿主
            host_memory_mb: 512,
            last_site_key: None,
            internal_volume: 100,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let dir = paths::ensure_dir(&paths::rust_data());
        let path = dir.join("settings.json");
        std::fs::read_to_string(&path)
            .ok()
            // Windows 记事本保存会加 BOM，serde_json 会直接报 “expected value”，
            // 那样用户手改过的设置会被静默丢弃并退回默认值（看起来像“配置全丢了”）。
            .map(|s| s.trim_start_matches('\u{feff}').to_string())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let dir = paths::ensure_dir(&paths::rust_data());
        let path = dir.join("settings.json");
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, text);
        }
    }

    pub fn active_source(&self) -> Option<&String> {
        self.active_source.and_then(|i| self.config_sources.get(i))
    }
}

/// 随包运行时可执行文件名
fn java_exe_name() -> &'static str {
    if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    }
}

/// 查找随包内置的 jlink 运行时（`<exe目录>/runtime/bin/java`）
pub fn bundled_java() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let mut candidates = vec![dir.join("runtime").join("bin").join(java_exe_name())];
    if let Some(parent) = dir.parent() {
        candidates.push(parent.join("runtime").join("bin").join(java_exe_name()));
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// Java 解析顺序：显式配置 → 随包 runtime → JAVA_HOME → PATH 中的 `java`。
/// 打包形态下用户无需安装 Java；开发形态仍可手动指定 java 路径。
pub fn resolve_java(configured: &str) -> String {
    let configured = configured.trim();
    if !configured.is_empty() && configured != "java" {
        return configured.to_string();
    }
    if let Some(p) = bundled_java() {
        return p.to_string_lossy().to_string();
    }
    if let Ok(home) = std::env::var("JAVA_HOME") {
        let exe = PathBuf::from(home).join("bin").join(java_exe_name());
        if exe.is_file() {
            return exe.to_string_lossy().to_string();
        }
    }
    "java".to_string()
}

/// 自动探测 spider-host fat jar
/// 顺序：环境变量 → 可执行文件同级目录（打包形态）→ 工作目录 → 工作目录父目录。
pub fn autodetect_host_jar() -> Option<PathBuf> {
    if let Ok(env_path) = std::env::var("SPIDER_HOST_JAR") {
        let p = PathBuf::from(env_path);
        if p.is_file() {
            return Some(p);
        }
    }
    const NAME: &str = "spider-host-all.jar";
    let mut candidates: Vec<PathBuf> = Vec::new();
    // 打包形态优先：jar 与 exe 放在同一目录
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(NAME));
            if let Some(parent) = dir.parent() {
                candidates.push(parent.join(NAME));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("spider-host").join("build").join("libs").join(NAME));
        candidates.push(cwd.join(NAME));
        if let Some(parent) = cwd.parent() {
            candidates.push(parent.join("spider-host").join("build").join("libs").join(NAME));
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

/// 探测结果：类型 + 路径
#[derive(Debug, Clone)]
pub struct DetectedPlayer {
    pub kind: String, // vlc / mpv / mpc / potplayer
    pub path: String,
}

/// 探测 libmpv 动态库（内置播放器依赖它）。
/// Windows 找 mpv-2.dll / libmpv-2.dll / mpv.dll；
/// Linux 找 libmpv.so.2 / libmpv.so。
pub fn detect_libmpv() -> Option<String> {
    #[cfg(windows)]
    {
        let candidates = [
            r"C:\Program Files\mpv\mpv-2.dll",
            r"C:\Program Files\mpv\libmpv-2.dll",
            r"C:\Program Files (x86)\mpv\mpv-2.dll",
            r"D:\Program Files\mpv\mpv-2.dll",
            r"D:\Program Files\mpv\libmpv-2.dll",
        ];
        for p in &candidates {
            if PathBuf::from(p).is_file() {
                return Some(p.to_string());
            }
        }
        // 随包分发：exe 同级目录 或 mpv/ 子目录
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                for name in &["mpv-2.dll", "libmpv-2.dll", "mpv.dll"] {
                    // 先查根目录
                    let root = dir.join(name);
                    if root.is_file() {
                        return Some(root.to_string_lossy().into_owned());
                    }
                    // 再查 mpv/ 子目录
                    let sub = dir.join("mpv").join(name);
                    if sub.is_file() {
                        return Some(sub.to_string_lossy().into_owned());
                    }
                }
            }
        }
        // 应用内下载安装的内置播放器组件（%AppData%\TV-Multiplatform\cache\libmpv）
        if let Some(p) = crate::core::libmpv_install::installed_dll() {
            return Some(p.to_string_lossy().into_owned());
        }
        // PATH 搜索
        if let Ok(paths) = std::env::var("PATH") {
            for dir in paths.split(';') {
                for name in &["mpv-2.dll", "libmpv-2.dll", "mpv.dll"] {
                    let p = PathBuf::from(dir).join(name);
                    if p.is_file() {
                        return Some(p.to_string_lossy().into_owned());
                    }
                }
            }
        }
    }

    #[cfg(not(windows))]
    {
        // 随包分发：exe 同级目录优先（便携包里直接放的 libmpv）
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                for name in &["libmpv.so.2", "libmpv.so", "libmpv.2.dylib", "libmpv.dylib"] {
                    let p = dir.join(name);
                    if p.is_file() {
                        return Some(p.to_string_lossy().into_owned());
                    }
                }
            }
        }
        // 应用内下载安装的内置播放器组件
        if let Some(p) = crate::core::libmpv_install::installed_dll() {
            return Some(p.to_string_lossy().into_owned());
        }
        let candidates = [
            "/usr/lib/x86_64-linux-gnu/libmpv.so.2",
            "/usr/lib/x86_64-linux-gnu/libmpv.so",
            "/usr/lib/libmpv.so.2",
            "/usr/lib/libmpv.so",
            "/usr/local/lib/libmpv.so.2",
            "/usr/local/lib/libmpv.so",
            "/opt/homebrew/lib/libmpv.dylib",
        ];
        for p in &candidates {
            if PathBuf::from(p).is_file() {
                return Some(p.to_string());
            }
        }
        // ldconfig / PATH 搜索
        if let Ok(output) = std::process::Command::new("ldconfig").arg("-p").output() {
            let text = String::from_utf8_lossy(&output.stdout);
            for line in text.lines() {
                if line.contains("libmpv.so") {
                    if let Some(path) = line.split("=>").nth(1) {
                        let path = path.trim();
                        if PathBuf::from(path).is_file() {
                            return Some(path.to_string());
                        }
                    }
                }
            }
        }
    }

    None
}

/// 自动探测外部播放器（PATH + 常见安装路径）
pub fn detect_players() -> Vec<DetectedPlayer> {
    let mut out = Vec::new();

    #[cfg(windows)]
    let common: &[(&str, &str)] = &[
        ("vlc", r"C:\Program Files\VideoLAN\VLC\vlc.exe"),
        ("vlc", r"C:\Program Files (x86)\VideoLAN\VLC\vlc.exe"),
        ("mpv", r"C:\Program Files\mpv\mpv.exe"),
        ("mpv", r"C:\Program Files (x86)\mpv\mpv.exe"),
        ("mpc", r"C:\Program Files\MPC-HC\mpc-hc64.exe"),
        ("mpc", r"C:\Program Files\MPC-BE\mpc-be64.exe"),
        ("mpc", r"C:\Program Files (x86)\MPC-HC\mpc-hc.exe"),
        ("potplayer", r"C:\Program Files\DAUM\PotPlayer\PotPlayerMini64.exe"),
        ("potplayer", r"C:\Program Files (x86)\DAUM\PotPlayer\PotPlayerMini64.exe"),
    ];
    #[cfg(not(windows))]
    let common: &[(&str, &str)] = &[
        ("vlc", "/usr/bin/vlc"),
        ("vlc", "/Applications/VLC.app/Contents/MacOS/VLC"),
        ("mpv", "/usr/bin/mpv"),
        ("mpv", "/opt/homebrew/bin/mpv"),
    ];

    for (kind, path) in common {
        if PathBuf::from(path).is_file() && !out.iter().any(|d: &DetectedPlayer| d.path == *path) {
            out.push(DetectedPlayer { kind: kind.to_string(), path: path.to_string() });
        }
    }

    // PATH 查找
    #[cfg(windows)]
    let which = "where";
    #[cfg(not(windows))]
    let which = "which";
    for name in ["vlc", "mpv", "mpc-hc64", "mpc-be64"] {
        if let Ok(output) = std::process::Command::new(which).arg(name).output() {
            if output.status.success() {
                let text = String::from_utf8_lossy(&output.stdout);
                if let Some(first) = text.lines().next() {
                    let first = first.trim();
                    if !first.is_empty() && PathBuf::from(first).is_file() && !out.iter().any(|d| d.path == first) {
                        let kind = if first.contains("vlc") {
                            "vlc"
                        } else if first.contains("mpv") {
                            "mpv"
                        } else {
                            "mpc"
                        };
                        out.push(DetectedPlayer { kind: kind.into(), path: first.into() });
                    }
                }
            }
        }
    }
    out
}

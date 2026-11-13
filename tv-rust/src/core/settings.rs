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
            host_memory_mb: 1024,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let dir = paths::ensure_dir(&paths::rust_data());
        let path = dir.join("settings.json");
        std::fs::read_to_string(&path)
            .ok()
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

/// 自动探测 spider-host fat jar
pub fn autodetect_host_jar() -> Option<PathBuf> {
    if let Ok(env_path) = std::env::var("SPIDER_HOST_JAR") {
        let p = PathBuf::from(env_path);
        if p.is_file() {
            return Some(p);
        }
    }
    const NAME: &str = "spider-host-all.jar";
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("spider-host").join("build").join("libs").join(NAME));
        candidates.push(cwd.join(NAME));
        candidates.push(cwd.parent().map(|p| p.join("spider-host").join("build").join("libs").join(NAME)).unwrap_or_default());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(NAME));
            candidates.push(dir.join("spider-host").join("build").join("libs").join(NAME));
            candidates.push(dir.parent().map(|p| p.join("spider-host").join("build").join("libs").join(NAME)).unwrap_or_default());
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

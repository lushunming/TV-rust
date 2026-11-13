//! 外部播放器启动（对应 Kotlin Play.kt / VLC / Mpv / MPC / Potplayer 的参数约定）

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::process::{Command, Stdio};

fn header_str(headers: &HashMap<String, String>) -> String {
    let mut s = String::new();
    for (k, v) in headers {
        if k.is_empty() || v.is_empty() {
            continue;
        }
        s.push_str(&format!("{}: {}\r\n", k, v));
    }
    s
}

/// 按播放器 exe 名称分发启动参数，立即返回（不等待播放器退出）
pub fn launch(player_path: &str, url: &str, title: &str, headers: &HashMap<String, String>) -> Result<()> {
    if player_path.trim().is_empty() {
        return Err(anyhow!("未配置外部播放器路径，请到设置页配置"));
    }
    if url.is_empty() {
        return Err(anyhow!("播放地址为空"));
    }

    let file_name = std::path::Path::new(player_path)
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    let mut cmd = Command::new(player_path);

    if file_name.contains("potplayer") {
        cmd.arg(url).arg("/current");
        let hs = header_str(headers);
        if !hs.is_empty() {
            cmd.arg(format!("/headers=\"{}\"", hs));
        }
        let t = title.replace([' ', '\t'], "");
        cmd.arg(format!("/title=\"{}\"", t));
    } else if file_name.contains("vlc") {
        cmd.arg(format!("--video-title={}", title)).arg(url);
    } else if file_name.contains("mpc") {
        cmd.arg(url).arg("/play");
    } else if file_name.contains("mpv") {
        // 允许 playerPath 后附带额外 mpv 参数（如 "mpv.exe --no-ytdl"）
        let mut args = player_path.split_whitespace();
        let exe = args.next().unwrap_or(player_path);
        cmd = Command::new(exe);
        for a in args {
            cmd.arg(a);
        }
        cmd.arg(url);
    } else {
        cmd.arg(url);
    }

    // 输出重定向到 playerLog（与 Kotlin 一致）
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(crate::core::paths::player_log()) {
        let _ = writeln!(f, "[{}] launch {} :: {}", chrono_millis(), file_name, url);
    }
    cmd.stdout(Stdio::null()).stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    cmd.spawn().map_err(|e| anyhow!("启动播放器失败 {}: {}", player_path, e))?;
    Ok(())
}

fn chrono_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

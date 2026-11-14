//! 外部播放器启动（对应 Kotlin Play.kt / VLC / Mpv / MPC / Potplayer 的参数约定）

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::process::{Command, Stdio};

/// 解析播放器命令：返回 (exe 路径, 额外参数)
/// 支持三种写法：
///   1. `D:\Program Files\mpv\mpv.exe`（带空格但无参数）
///   2. `"D:\Program Files\mpv\mpv.exe" --no-ytdl`（带引号）
///   3. `D:\mpv\mpv.exe --no-ytdl`（无引号 + 参数，在 .exe 处切分）
pub fn split_player_cmd(raw: &str) -> (String, Vec<String>) {
    let s = raw.trim();
    if s.is_empty() {
        return (String::new(), Vec::new());
    }
    // 整串就是一个存在的文件 → 路径本身含空格，不需要切分
    if std::path::Path::new(s).is_file() {
        return (s.to_string(), Vec::new());
    }
    if let Some(rest) = s.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            let exe = rest[..end].to_string();
            let args = rest[end + 1..]
                .split_whitespace()
                .map(|x| x.to_string())
                .collect();
            return (exe, args);
        }
    }
    let lower = s.to_lowercase();
    if let Some(pos) = lower.find(".exe") {
        let cut = pos + 4;
        let exe = s[..cut].to_string();
        let args = s[cut..].split_whitespace().map(|x| x.to_string()).collect();
        return (exe, args);
    }
    (s.to_string(), Vec::new())
}

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

    // 路径可能带额外参数或引号（如 "mpv.exe --no-ytdl"）。
    // 不能按空白切分：C:\Program Files\mpv\mpv.exe 会被截成 C:\Program，报 os error 2。
    let (mut exe, extra_args) = split_player_cmd(player_path);
    if !std::path::Path::new(&exe).is_file() {
        // 配置的播放器不存在时回退到自动探测，而不是直接抛“找不到指定的文件”
        if let Some(found) = crate::core::settings::detect_players().into_iter().next() {
            crate::core::log::warn(format!("配置的播放器不存在：{}，改用 {}", exe, found.path));
            exe = found.path;
        } else {
            return Err(anyhow!(
                "找不到播放器：{}（文件不存在），请到设置页选择或填写正确的 exe 路径",
                exe
            ));
        }
    }
    let file_name = std::path::Path::new(&exe)
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or(file_name);

    let mut cmd = Command::new(&exe);

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
        // 允许 playerPath 后附带额外 mpv 参数（如 "mpv.exe --no-ytdl"），
        // 参数已由 split_player_cmd 解析，不能再按空白切路径
        for a in &extra_args {
            cmd.arg(a);
        }
        cmd.arg(url);
    } else {
        cmd.arg(url);
    }

    // 输出重定向到 playerLog（与 Kotlin 一致）：播放失败时能直接看到解码/网络错误
    use std::io::Write;
    let log_path = crate::core::paths::player_log();
    if let Some(dir) = log_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut log_file = std::fs::OpenOptions::new().create(true).append(true).open(&log_path).ok();
    if let Some(f) = log_file.as_mut() {
        let _ = writeln!(
            f,
            "\n[{}] launch {} :: {} headers={}",
            crate::core::log::timestamp(),
            file_name,
            url,
            headers.len()
        );
        let _ = f.flush();
    }
    if let Some(f) = log_file {
        if let Ok(out) = f.try_clone() {
            cmd.stdout(Stdio::from(out));
        }
        if let Ok(err) = f.try_clone() {
            cmd.stderr(Stdio::from(err));
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }

    match cmd.spawn() {
        Ok(child) => {
            crate::core::log::info(format!(
                "播放器已启动 pid={} {} url={}",
                child.id(),
                file_name,
                url
            ));
            Ok(())
        }
        Err(e) => {
            crate::core::log::error(format!("启动播放器失败 {}: {}", exe, e));
            Err(anyhow!("启动播放器失败 {}: {}", exe, e))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::split_player_cmd;

    #[test]
    fn keeps_paths_with_spaces() {
        let (exe, args) = split_player_cmd(r"D:\Program Files\mpv\mpv.exe");
        assert_eq!(exe, r"D:\Program Files\mpv\mpv.exe");
        assert!(args.is_empty());
    }

    #[test]
    fn supports_quoted_path_with_args() {
        let (exe, args) = split_player_cmd(r#""D:\Program Files\mpv\mpv.exe" --no-ytdl --hwdec=auto"#);
        assert_eq!(exe, r"D:\Program Files\mpv\mpv.exe");
        assert_eq!(args, vec!["--no-ytdl", "--hwdec=auto"]);
    }

    #[test]
    fn supports_unquoted_exe_with_args() {
        let (exe, args) = split_player_cmd(r"D:\mpv\mpv.exe --no-ytdl");
        assert_eq!(exe, r"D:\mpv\mpv.exe");
        assert_eq!(args, vec!["--no-ytdl"]);
    }
}

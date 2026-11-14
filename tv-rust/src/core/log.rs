//! 本地日志：Rust 侧事件 + 宿主 stderr 落盘。
//!
//! 打包版是 windows 子系统程序，没有控制台，stderr 会被系统丢弃；
//! 出问题时必须有文件可查，因此所有关键链路（启动、配置、爬虫调用、播放）
//! 都写入 `%AppData%\TV-Multiplatform\cache\rust\tv-rust.log`，
//! 宿主日志写入同目录下的 `spider-host.log`。
//!
//! 时间使用本地时区：启动时通过 `cmd /c echo %TIME%` 探测一次偏移，
//! 避免为此引入额外依赖。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// 单个日志文件上限，超过后轮转为 `.1`
const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;

static FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();
static OFFSET_SECS: OnceLock<i64> = OnceLock::new();

pub fn log_path() -> PathBuf {
    crate::core::paths::rust_data().join("tv-rust.log")
}

pub fn host_log_path() -> PathBuf {
    crate::core::paths::rust_data().join("spider-host.log")
}

pub fn init() {
    let _ = offset_secs();
    info(format!(
        "===== tv-rust start pid={} version={} =====",
        std::process::id(),
        env!("CARGO_PKG_VERSION")
    ));
    info(format!("log file: {}", log_path().display()));
}

pub fn info(msg: impl AsRef<str>) {
    write("INFO ", msg.as_ref());
}

pub fn warn(msg: impl AsRef<str>) {
    write("WARN ", msg.as_ref());
}

pub fn error(msg: impl AsRef<str>) {
    write("ERROR", msg.as_ref());
}

/// 打开宿主 stderr 落盘文件（启动宿主时调用，顺带做一次轮转）
pub fn open_host_log() -> Option<File> {
    open_log(host_log_path())
}

fn write(level: &str, msg: &str) {
    let line = format!("{} {} {}\n", timestamp(), level, msg);
    if let Ok(mut guard) = file().lock() {
        if let Some(f) = guard.as_mut() {
            let _ = f.write_all(line.as_bytes());
            let _ = f.flush();
        }
    }
    // 开发态额外回显，方便直接看终端
    if cfg!(debug_assertions) {
        eprint!("{line}");
    }
}

fn file() -> &'static Mutex<Option<File>> {
    FILE.get_or_init(|| Mutex::new(open_log(log_path())))
}

fn open_log(path: PathBuf) -> Option<File> {
    let parent = path.parent()?;
    let _ = std::fs::create_dir_all(parent);
    if std::fs::metadata(&path).map(|m| m.len() > MAX_LOG_BYTES).unwrap_or(false) {
        let mut bak = path.clone();
        bak.set_file_name(format!(
            "{}.1",
            path.file_name()?.to_string_lossy()
        ));
        let _ = std::fs::remove_file(&bak);
        let _ = std::fs::rename(&path, &bak);
    }
    OpenOptions::new().create(true).append(true).open(&path).ok()
}

pub fn timestamp() -> String {
    let secs = now_secs() as i64 + offset_secs();
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        y,
        m,
        d,
        sod / 3600,
        (sod % 3600) / 60,
        sod % 60
    )
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn offset_secs() -> i64 {
    *OFFSET_SECS.get_or_init(detect_offset)
}

#[cfg(windows)]
fn detect_offset() -> i64 {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    let utc_day = (now_secs() % 86_400) as i64;
    let out = Command::new("cmd")
        .args(["/c", "echo %TIME%"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output();
    if let Ok(out) = out {
        let text = String::from_utf8_lossy(&out.stdout);
        if let Some(local) = parse_hms(text.trim()) {
            let mut off = local - utc_day;
            if off > 18 * 3600 {
                off -= 86_400;
            }
            if off < -18 * 3600 {
                off += 86_400;
            }
            return off;
        }
    }
    0
}

#[cfg(not(windows))]
fn detect_offset() -> i64 {
    0
}

/// 解析 `%TIME%`：兼容 " 9:23:45.67" 与 "09:23:45,67"
fn parse_hms(text: &str) -> Option<i64> {
    let cleaned = text.trim().replace(',', ".");
    let mut parts = cleaned.split(':');
    let h: i64 = parts.next()?.trim().parse().ok()?;
    let m: i64 = parts.next()?.trim().parse().ok()?;
    let s: i64 = parts.next()?.trim().split('.').next()?.parse().ok()?;
    if !(0..24).contains(&h) || !(0..60).contains(&m) || !(0..61).contains(&s) {
        return None;
    }
    Some(h * 3600 + m * 60 + s)
}

/// 从 1970-01-01 起的天数转换为公历年月日（Howard Hinnant 算法）
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

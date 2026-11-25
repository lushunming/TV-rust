//! 内嵌 mpv 播放器：通过 libmpv C API 动态加载，
//! Windows 下用 Win32 SetParent 嵌入，Linux 下通过 `wid` 选项让 mpv 自动嵌入 X11/Wayland 窗口。
//!
//! 与子进程方案相比：
//! - 没有 JSON IPC 序列化开销，控制延迟更低
//! - 没有额外进程，内存占用更小
//! - 事件直接通过 `mpv_wait_event` 获取，不需要解析 JSON
//! - 用户只需有 libmpv 库（Windows: mpv-2.dll，Linux: libmpv.so.2）

use crate::core::libmpv_ffi::*;
use crate::core::mpv_render::MpvRender;
use anyhow::{anyhow, Result};
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 主窗口标题（与 `main.rs` 的 `with_title` 一致）
pub const MAIN_WINDOW_TITLE: &str = "TV Rust — TV-Multiplatform";

// ========== Win32 raw FFI（Windows 专用）==========

#[cfg(windows)]
pub mod win {
    use std::ffi::c_void;
    pub type Handle = *mut c_void;

    pub const GW_CHILD: u32 = 5;
    pub const GW_HWNDNEXT: u32 = 2;
    pub const SWP_NOZORDER: u32 = 0x0004;
    pub const SWP_NOACTIVATE: u32 = 0x0010;
    pub const SWP_FRAMECHANGED: u32 = 0x0020;
    pub const SWP_ASYNCWINDOWPOS: u32 = 0x4000;
    pub const SW_HIDE: i32 = 0;
    pub const SW_SHOW: i32 = 5;
    pub const SW_MAXIMIZE: i32 = 3;
    pub const SW_RESTORE: i32 = 9;
    pub const GA_ROOT: u32 = 2;
    pub const GWL_STYLE: i32 = -16;
    pub const WS_POPUP: u32 = 0x8000_0000;
    pub const WS_CAPTION: u32 = 0x00C0_0000;
    pub const WS_THICKFRAME: u32 = 0x0004_0000;
    pub const WS_MINIMIZEBOX: u32 = 0x0002_0000;
    pub const WS_MAXIMIZEBOX: u32 = 0x0001_0000;
    pub const SM_CXSCREEN: i32 = 0;
    pub const SM_CYSCREEN: i32 = 1;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct RECT {
        pub left: i32,
        pub top: i32,
        pub right: i32,
        pub bottom: i32,
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn FindWindowW(class: *const u16, title: *const u16) -> Handle;
        pub fn EnumWindows(cb: unsafe extern "system" fn(Handle, isize) -> i32, lparam: isize) -> i32;
        pub fn GetClassNameW(hwnd: Handle, buf: *mut u16, max: i32) -> i32;
        pub fn IsWindowVisible(hwnd: Handle) -> i32;
        pub fn GetAncestor(hwnd: Handle, flags: u32) -> Handle;
        pub fn SetWindowPos(hwnd: Handle, after: Handle, x: i32, y: i32, cx: i32, cy: i32, flags: u32) -> i32;
        pub fn ShowWindow(hwnd: Handle, cmd: i32) -> i32;
        pub fn GetWindow(hwnd: Handle, cmd: u32) -> Handle;
        pub fn GetForegroundWindow() -> Handle;
        pub fn GetAsyncKeyState(vk: i32) -> i16;
        pub fn GetWindowLongPtrW(hwnd: Handle, index: i32) -> isize;
        pub fn SetWindowLongPtrW(hwnd: Handle, index: i32, value: isize) -> isize;
        pub fn GetWindowRect(hwnd: Handle, rect: *mut RECT) -> i32;
        pub fn GetSystemMetrics(index: i32) -> i32;
    }

    pub fn is_key_pressed(vk: i32) -> bool {
        unsafe { (GetAsyncKeyState(vk) & 0x8000u16 as i16) != 0 }
    }

    pub const VK_ESCAPE: i32 = 0x1B;
    pub const VK_F11: i32 = 0x7A;
    pub const VK_SPACE: i32 = 0x20;
    pub const VK_LEFT: i32 = 0x25;
    pub const VK_RIGHT: i32 = 0x27;
}

// ========== 全局 HWND 存储 ==========

#[cfg(windows)]
static HOST_HWND: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

#[cfg(windows)]
pub fn set_host_hwnd(hwnd: isize) {
    HOST_HWND.store(hwnd, Ordering::SeqCst);
}

#[cfg(windows)]
pub fn host_hwnd() -> Option<isize> {
    let h = HOST_HWND.load(Ordering::SeqCst);
    if h == 0 { None } else { Some(h) }
}

// ========== 事件 ==========

#[derive(Debug, Clone, PartialEq)]
pub enum MpvEvent {
    /// 已开始出画面
    Playing,
    /// 播放结束
    Ended,
    /// 播放失败
    Failed(String),
}

// ========== 播放器 ==========

pub struct InternalPlayer {
    dll: Arc<MpvDll>,
    /// mpv 实例指针（null 表示已销毁）
    ctx: Arc<Mutex<*mut mpv_handle>>,
    alive: Arc<AtomicBool>,
    /// mpv 的渲染窗口句柄（仅 Windows）
    pub mpv_hwnd: Option<isize>,
    /// 当前是否已暂停（由 toggle/set_pause 维护，也受事件修正）
    pub paused: AtomicBool,
    /// 是否已显示（避免重复 ShowWindow 抖动）
    visible: bool,
    /// mpv 渲染上下文（把帧直接画进 egui 的 GL 画布）
    pub render: Arc<MpvRender>,
    /// 积压事件（由 poll_events 消费）
    events: Arc<Mutex<Vec<MpvEvent>>>,
}

static NEXT_REQ_ID: AtomicI64 = AtomicI64::new(1);

impl InternalPlayer {
    /// 创建 libmpv 实例并开始播放。
    ///
    /// - `dll_path`：libmpv 库路径（Windows: mpv-2.dll，Linux: libmpv.so.2）
    /// - `url`：播放地址
    /// - `headers`：HTTP 请求头
    /// - `parent_hwnd`：承载窗口句柄（Windows HWND，Linux X11 Window ID）
    pub fn new(
        dll_path: &str,
        url: &str,
        headers: &[(String, String)],
        parent_hwnd: isize,
    ) -> Result<Self> {
        let dll = Arc::new(MpvDll::load(dll_path).map_err(|e| {
            anyhow!(
                "加载 libmpv 失败: {}（路径: {}，请确认已安装 mpv 开发库）",
                e,
                dll_path
            )
        })?);

        let ctx = unsafe { (dll.mpv_create)() };
        if ctx.is_null() {
            return Err(anyhow!("mpv_create 返回 null"));
        }

        // 辅助闭包：设置 mpv 选项
        let set_opt = |name: &str, val: &str| -> Result<()> {
            let n = CString::new(name)?;
            let v = CString::new(val)?;
            let r = unsafe { (dll.mpv_set_option_string)(ctx, n.as_ptr(), v.as_ptr()) };
            if r < 0 {
                return Err(anyhow!("设置选项 {}={} 失败: {}", name, val, r));
            }
            Ok(())
        };

        // 嵌入到父窗口（wid 在 Windows 是 HWND，Linux 是 X11 Window ID）
        // 注意：改用渲染 API 后 mpv 不再创建任何窗口，`wid` 与 `force-window` 都不能设置，
        // 否则 mpv 会去建子窗口，导致"没有画面/输入被抢"。
        let _ = parent_hwnd;
        set_opt("vo", "libmpv")?;
        set_opt("osc", "no")?;
        set_opt("osd-level", "0")?;
        set_opt("input-default-bindings", "no")?;
        set_opt("input-vo-keyboard", "no")?;
        set_opt("network-timeout", "15")?;
        set_opt("keep-open", "no")?;
        set_opt("idle", "no")?;
        // 渲染 API + OpenGL：硬解只做"解码后拷回内存"，避免和宿主 GL 上下文做互操作
        set_opt("gpu-api", "opengl")?;
        set_opt("hwdec", "auto-copy")?;

        // HTTP headers：mpv 的 http-header-fields 支持逗号分隔的多条 header
        // 把所有 header 合并成 "Key1: Val1,Key2: Val2" 格式
        let header_str: String = headers
            .iter()
            .filter(|(k, v)| !k.is_empty() && !v.is_empty())
            .map(|(k, v)| format!("{}: {}", k, v))
            .collect::<Vec<_>>()
            .join(",");
        if !header_str.is_empty() {
            set_opt("http-header-fields", &header_str)?;
        }

        let r = unsafe { (dll.mpv_initialize)(ctx) };
        if r < 0 {
            unsafe { (dll.mpv_terminate_destroy)(ctx) };
            return Err(anyhow!("mpv_initialize 失败: {}（可能缺少解码器）", r));
        }

        // 加载视频：用 command_async 传参数数组，避免 command_string 解析 URL 特殊字符
        let loadfile_args = [
            CString::new("loadfile")?,
            CString::new(url)?,
            CString::new("replace")?,
        ];
        let ptrs: Vec<*const std::ffi::c_char> = loadfile_args
            .iter()
            .map(|s| s.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect();
        let r = unsafe { (dll.mpv_command_async)(ctx, 0, ptrs.as_ptr()) };
        if r < 0 {
            unsafe { (dll.mpv_terminate_destroy)(ctx) };
            return Err(anyhow!("加载视频失败: {}", r));
        }

        let alive = Arc::new(AtomicBool::new(true));
        let events = Arc::new(Mutex::new(Vec::new()));
        let ctx_arc = Arc::new(Mutex::new(ctx));
        let render = MpvRender::new(dll.clone(), ctx);

        let mut player = Self {
            dll,
            ctx: ctx_arc,
            alive,
            mpv_hwnd: None,
            paused: AtomicBool::new(false),
            visible: false,
            events,
            render,
        };

        // 渲染 API 模式：不等待子窗口（mpv 不建窗口了）
        #[cfg(windows)]
        if parent_hwnd != 0 {
            player.mpv_hwnd = None;
        }

        Ok(player)
    }

    /// 发送 command_async（参数数组，避免字符串解析问题）
    fn cmd_async(&self, args: &[&str]) {
        let ctx = self.ctx.lock().unwrap();
        if ctx.is_null() {
            return;
        }
        let c_args: Vec<CString> = match args.iter().map(|s| CString::new(*s)).collect::<Result<Vec<_>, _>>() {
            Ok(v) => v,
            Err(_) => return,
        };
        let ptrs: Vec<*const std::ffi::c_char> = c_args
            .iter()
            .map(|s| s.as_ptr())
            .chain(std::iter::once(std::ptr::null()))
            .collect();
        unsafe { (self.dll.mpv_command_async)(*ctx, 0, ptrs.as_ptr()) };
    }

    /// 设置属性字符串
    fn set_prop(&self, name: &str, val: &str) {
        let ctx = self.ctx.lock().unwrap();
        if ctx.is_null() {
            return;
        }
        if let (Ok(n), Ok(v)) = (CString::new(name), CString::new(val)) {
            unsafe { (self.dll.mpv_set_property_string)(*ctx, n.as_ptr(), v.as_ptr()) };
        }
    }

    /// 获取属性字符串并解析为 f64
    fn get_prop_f64(&self, name: &str) -> Option<f64> {
        let ctx = self.ctx.lock().unwrap();
        if ctx.is_null() {
            return None;
        }
        let n = CString::new(name).ok()?;
        let ptr = unsafe { (self.dll.mpv_get_property_string)(*ctx, n.as_ptr()) };
        parse_f64(&self.dll, ptr)
    }

    // ---------- 控制 ----------

    pub fn set_pause(&mut self, pause: bool) {
        self.paused.store(pause, Ordering::SeqCst);
        self.set_prop("pause", if pause { "yes" } else { "no" });
    }

    pub fn toggle_pause(&mut self) {
        self.set_pause(!self.paused.load(Ordering::SeqCst));
    }

    pub fn seek_relative(&self, delta: f64) {
        self.cmd_async(&["seek", &format!("{:.1}", delta), "relative"]);
    }

    pub fn seek_absolute(&self, sec: f64) {
        self.cmd_async(&["seek", &format!("{:.1}", sec), "absolute"]);
    }

    pub fn set_volume(&self, vol: u8) {
        self.set_prop("volume", &format!("{}", vol.min(100)));
    }

    // ---------- 查询 ----------

    pub fn position_secs(&self) -> Option<f64> {
        self.get_prop_f64("time-pos")
    }

    pub fn duration_secs(&self) -> Option<f64> {
        self.get_prop_f64("duration")
    }

    pub fn video_size(&self) -> Option<(u32, u32)> {
        let w = self.get_prop_f64("video-params/w")? as u32;
        let h = self.get_prop_f64("video-params/h")? as u32;
        if w == 0 || h == 0 {
            None
        } else {
            Some((w, h))
        }
    }

    /// 当前缓冲进度 0.0-1.0（已缓存到的**绝对位置**占总时长的比例）
    ///
    /// 注意：`demuxer-cache-duration` 是“播放点之后还能播多久”，不是文件已下载了多少，
    /// 所以必须加上当前位置：缓冲终点 = time-pos + demuxer-cache-duration。
    /// 之前直接拿它除以总时长，40 分钟的片子缓存 10 秒只有 0.4%，绿条几乎看不见。
    pub fn cache_progress(&self) -> Option<f64> {
        let cached = self.get_prop_f64("demuxer-cache-duration")?;
        let dur = self.get_prop_f64("duration")?;
        if dur <= 0.0 {
            return None;
        }
        let pos = self.get_prop_f64("time-pos").unwrap_or(0.0).max(0.0);
        Some(((pos + cached.max(0.0)) / dur).clamp(0.0, 1.0))
    }

    // ---------- 事件 ----------

    /// 非阻塞轮询所有积压事件。
    /// 由 UI 线程每 100~500ms 调用一次。
    pub fn poll_events(&self) -> Vec<MpvEvent> {
        if !self.is_alive() {
            return Vec::new();
        }
        let ctx = self.ctx.lock().unwrap();
        if ctx.is_null() {
            return Vec::new();
        }
        let mut evts = Vec::new();
        loop {
            let event = unsafe { (self.dll.mpv_wait_event)(*ctx, 0.0) };
            if event.is_null() {
                break;
            }
            let event_id = unsafe { (*event).event_id };
            match event_id {
                MPV_EVENT_NONE => break,
                MPV_EVENT_SHUTDOWN => {
                    self.alive.store(false, Ordering::SeqCst);
                    break;
                }
                MPV_EVENT_START_FILE => {
                    crate::core::log::info("内置播放器: 开始加载文件");
                }
                MPV_EVENT_FILE_LOADED => {
                    crate::core::log::info("内置播放器: 文件已加载");
                    evts.push(MpvEvent::Playing);
                }
                MPV_EVENT_PLAYBACK_RESTART => {
                    crate::core::log::info("内置播放器: 开始播放");
                    evts.push(MpvEvent::Playing);
                }
                MPV_EVENT_END_FILE => {
                    let data = unsafe { (*event).data as *mut crate::core::libmpv_ffi::mpv_event_end_file };
                    if !data.is_null() {
                        let reason = unsafe { (*data).reason };
                        let err = unsafe { (*data).error };
                        crate::core::log::warn(format!(
                            "内置播放器: 播放结束 reason={} error={}",
                            reason, err
                        ));
                        if reason == crate::core::libmpv_ffi::MPV_END_FILE_REASON_ERROR {
                            evts.push(MpvEvent::Failed(format!(
                                "加载失败 (reason={}, error={})",
                                reason, err
                            )));
                        } else {
                            evts.push(MpvEvent::Ended);
                        }
                    } else {
                        evts.push(MpvEvent::Ended);
                    }
                }
                MPV_EVENT_IDLE => {
                    evts.push(MpvEvent::Ended);
                }
                MPV_EVENT_PAUSE => {
                    self.paused.store(true, Ordering::SeqCst);
                }
                MPV_EVENT_UNPAUSE => {
                    self.paused.store(false, Ordering::SeqCst);
                }
                _ => {}
            }
        }
        evts
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    // ---------- 窗口管理 ----------

    #[cfg(windows)]
    pub fn place(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let Some(hwnd) = self.mpv_hwnd else { return };
        move_child(hwnd, x, y, w, h);
        if !self.visible {
            show_window(hwnd, true);
            self.visible = true;
        }
    }

    #[cfg(not(windows))]
    pub fn place(&mut self, _x: i32, _y: i32, _w: i32, _h: i32) {
        // Linux 下 libmpv 通过 wid 自动嵌入，不需要手动移动子窗口
        self.visible = true;
    }

    #[cfg(windows)]
    pub fn hide(&mut self) {
        if let Some(hwnd) = self.mpv_hwnd {
            if self.visible {
                show_window(hwnd, false);
                self.visible = false;
            }
        }
    }

    #[cfg(not(windows))]
    pub fn hide(&mut self) {
        self.visible = false;
    }

    /// 停止播放并销毁 mpv 实例
    pub fn stop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
        // 必须先释放渲染上下文，再销毁 mpv 核心
        self.render.shutdown();
        let mut ctx = self.ctx.lock().unwrap();
        if !ctx.is_null() {
            unsafe { (self.dll.mpv_terminate_destroy)(*ctx) };
            *ctx = std::ptr::null_mut();
        }
    }
}

impl Drop for InternalPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

// ========== Win32 辅助（Windows）==========

#[cfg(windows)]
pub fn find_main_window() -> Option<isize> {
    let wide: Vec<u16> = MAIN_WINDOW_TITLE
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let h = win::FindWindowW(std::ptr::null(), wide.as_ptr());
        if h.is_null() { None } else { Some(h as isize) }
    }
}

#[cfg(not(windows))]
pub fn find_main_window() -> Option<isize> {
    None
}

#[cfg(windows)]
fn wait_child_window(parent: isize, timeout: Duration) -> Option<isize> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(h) = find_child_window(parent) {
            return Some(h);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(not(windows))]
fn wait_child_window(_parent: isize, _timeout: Duration) -> Option<isize> {
    None
}

#[cfg(windows)]
fn find_child_window(parent: isize) -> Option<isize> {
    unsafe {
        let mut child = win::GetWindow(parent as win::Handle, win::GW_CHILD);
        while !child.is_null() {
            if window_class(child) == "mpv" {
                return Some(child as isize);
            }
            child = win::GetWindow(child, win::GW_HWNDNEXT);
        }
    }
    None
}

#[cfg(windows)]
fn window_class(hwnd: win::Handle) -> String {
    let mut buf = [0u16; 64];
    let len = unsafe { win::GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if len <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..len as usize])
}

#[cfg(windows)]
pub fn move_child(hwnd: isize, x: i32, y: i32, w: i32, h: i32) {
    unsafe {
        let _ = win::SetWindowPos(
            hwnd as win::Handle,
            std::ptr::null_mut(),
            x, y, w.max(1), h.max(1),
            win::SWP_NOZORDER | win::SWP_NOACTIVATE,
        );
    }
}

#[cfg(not(windows))]
pub fn move_child(_hwnd: isize, _x: i32, _y: i32, _w: i32, _h: i32) {}

#[cfg(windows)]
pub fn show_window(hwnd: isize, show: bool) {
    unsafe {
        let _ = win::ShowWindow(
            hwnd as win::Handle,
            if show { win::SW_SHOW } else { win::SW_HIDE },
        );
    }
}

#[cfg(not(windows))]
pub fn show_window(_hwnd: isize, _show: bool) {}

#[cfg(windows)]
pub fn find_mpv_top_window() -> Option<isize> {
    unsafe extern "system" fn cb(hwnd: win::Handle, lparam: isize) -> i32 {
        let class = unsafe { window_class(hwnd) };
        if win::IsWindowVisible(hwnd) != 0 && class == "mpv" {
            let root = win::GetAncestor(hwnd, win::GA_ROOT);
            let target = if root.is_null() { hwnd } else { root };
            let slot = unsafe { &mut *(lparam as *mut Option<isize>) };
            *slot = Some(target as isize);
            return 0;
        }
        1
    }

    unsafe {
        let mut slot: Option<isize> = None;
        let ret = win::EnumWindows(cb, &mut slot as *mut _ as isize);
        let _ = ret;
        slot
    }
}

#[cfg(not(windows))]
pub fn find_mpv_top_window() -> Option<isize> {
    None
}

#[cfg(windows)]
pub fn foreground_hwnd() -> Option<isize> {
    unsafe {
        let h = win::GetForegroundWindow();
        if h.is_null() { None } else { Some(h as isize) }
    }
}

#[cfg(not(windows))]
pub fn foreground_hwnd() -> Option<isize> {
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn main_window_title_matches_main_rs() {
        assert_eq!(super::MAIN_WINDOW_TITLE, "TV Rust — TV-Multiplatform");
    }
}

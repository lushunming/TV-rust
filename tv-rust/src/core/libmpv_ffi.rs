//! libmpv C API 的 Rust FFI 绑定（动态加载）。
//!
//! 通过 `libloading` 在运行时加载 mpv-2.dll / libmpv-2.dll / mpv.dll，
//! 避免编译时链接，用户只需放置 DLL 即可使用内置播放器。
//!
//! 为什么不把 libmpv 静态链接进 TVRust.exe：
//! libmpv 本身依赖 FFmpeg、OpenGL/Vulkan、各种解码器，
//! 静态链接会让 exe 体积暴增到 100MB+，且升级 mpv 需重新编译。
//! 动态加载让用户可以单独替换 mpv-2.dll 来升级解码能力。

use std::ffi::{c_char, c_double, c_int, c_void, CStr, CString};
use std::path::Path;

// ============================================================
// Opaque 类型
// ============================================================

/// libmpv 实例句柄（opaque，只能通过 API 操作）
#[repr(C)]
pub struct mpv_handle {
    _private: [u8; 0],
}

/// libmpv 事件
#[repr(C)]
#[derive(Debug)]
pub struct mpv_event {
    pub event_id: i32,
    pub error: i32,
    pub reply_userdata: u64,
    pub data: *mut c_void,
}

/// MPV_EVENT_END_FILE 的 data 指针
#[repr(C)]
#[derive(Debug)]
pub struct mpv_event_end_file {
    pub reason: i32,
    pub error: i32,
}

// 结束原因常量
pub const MPV_END_FILE_REASON_EOF: i32 = 0;
pub const MPV_END_FILE_REASON_STOP: i32 = 2;
pub const MPV_END_FILE_REASON_QUIT: i32 = 3;
pub const MPV_END_FILE_REASON_ERROR: i32 = 4;

// ============================================================
// 事件 ID 常量（与 libmpv client.h 保持一致）
// ============================================================

pub const MPV_EVENT_NONE: i32 = 0;
pub const MPV_EVENT_SHUTDOWN: i32 = 1;
pub const MPV_EVENT_LOG_MESSAGE: i32 = 2;
pub const MPV_EVENT_GET_PROPERTY_REPLY: i32 = 3;
pub const MPV_EVENT_SET_PROPERTY_REPLY: i32 = 4;
pub const MPV_EVENT_COMMAND_REPLY: i32 = 5;
pub const MPV_EVENT_START_FILE: i32 = 6;
pub const MPV_EVENT_END_FILE: i32 = 7;
pub const MPV_EVENT_FILE_LOADED: i32 = 8;
pub const MPV_EVENT_TRACKS_CHANGED: i32 = 9;
pub const MPV_EVENT_TRACK_SWITCHED: i32 = 10;
pub const MPV_EVENT_IDLE: i32 = 11;
pub const MPV_EVENT_PAUSE: i32 = 12;
pub const MPV_EVENT_UNPAUSE: i32 = 13;
pub const MPV_EVENT_TICK: i32 = 14;
pub const MPV_EVENT_SCRIPT_INPUT_DISPATCH: i32 = 15;
pub const MPV_EVENT_CLIENT_MESSAGE: i32 = 16;
pub const MPV_EVENT_VIDEO_RECONFIG: i32 = 17;
pub const MPV_EVENT_AUDIO_RECONFIG: i32 = 18;
pub const MPV_EVENT_SEEK: i32 = 19;
pub const MPV_EVENT_PLAYBACK_RESTART: i32 = 20;
pub const MPV_EVENT_PROPERTY_CHANGE: i32 = 21;
pub const MPV_EVENT_QUEUE_OVERFLOW: i32 = 22;

// ============================================================
// 渲染 API（render.h / render_gl.h）
// ============================================================

/// mpv_render_param：一个 type + 一个数据指针
#[repr(C)]
pub struct mpv_render_param {
    pub type_: i32,
    pub data: *mut c_void,
}

pub const MPV_RENDER_PARAM_INVALID: i32 = 0;
pub const MPV_RENDER_PARAM_API_TYPE: i32 = 1;
pub const MPV_RENDER_PARAM_OPENGL_INIT_PARAMS: i32 = 2;
pub const MPV_RENDER_PARAM_OPENGL_FBO: i32 = 3;
pub const MPV_RENDER_PARAM_FLIP_Y: i32 = 4;
pub const MPV_RENDER_PARAM_ADVANCED_CONTROL: i32 = 10;

/// MPV_RENDER_PARAM_API_TYPE 的取值
pub const MPV_RENDER_API_TYPE_OPENGL: &str = "opengl";

/// OpenGL 初始化参数：由 mpv 回调拿 GL 函数指针
#[repr(C)]
pub struct mpv_opengl_init_params {
    pub get_proc_address: Option<unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void>,
    pub get_proc_address_ctx: *mut c_void,
}

/// 渲染目标（默认帧缓冲传 fbo=0）
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct mpv_opengl_fbo {
    pub fbo: i32,
    pub w: i32,
    pub h: i32,
    pub internal_format: i32,
}

// ============================================================
// DLL 动态加载封装
// ============================================================

/// 动态加载的 libmpv DLL。
///
/// 内部持有 `libloading::Library` 防止 DLL 被卸载，
/// 所有函数指针指向 DLL 中的导出函数，在 `Library` drop 之前始终有效。
pub struct MpvDll {
    _lib: libloading::Library,
    pub mpv_create: unsafe extern "C" fn() -> *mut mpv_handle,
    pub mpv_initialize: unsafe extern "C" fn(*mut mpv_handle) -> c_int,
    pub mpv_set_option_string: unsafe extern "C" fn(*mut mpv_handle, *const c_char, *const c_char) -> c_int,
    pub mpv_command_string: unsafe extern "C" fn(*mut mpv_handle, *const c_char) -> c_int,
    pub mpv_wait_event: unsafe extern "C" fn(*mut mpv_handle, c_double) -> *mut mpv_event,
    pub mpv_terminate_destroy: unsafe extern "C" fn(*mut mpv_handle),
    pub mpv_set_wakeup_callback: unsafe extern "C" fn(
        *mut mpv_handle,
        Option<unsafe extern "C" fn(*mut c_void)>,
        *mut c_void,
    ),
    pub mpv_free: unsafe extern "C" fn(*mut c_void),
    pub mpv_get_property_string: unsafe extern "C" fn(*mut mpv_handle, *const c_char) -> *mut c_char,
    pub mpv_set_property_string: unsafe extern "C" fn(*mut mpv_handle, *const c_char, *const c_char) -> c_int,
    pub mpv_observe_property: unsafe extern "C" fn(*mut mpv_handle, u64, *const c_char, c_int) -> c_int,
    pub mpv_command_async: unsafe extern "C" fn(*mut mpv_handle, u64, *const *const c_char) -> c_int,
    // ---- 渲染 API（把帧画到宿主自己的 GL 画布）----
    pub mpv_render_context_create: unsafe extern "C" fn(
        *mut *mut c_void,
        *mut mpv_handle,
        *mut mpv_render_param,
    ) -> c_int,
    pub mpv_render_context_render: unsafe extern "C" fn(*mut c_void, *mut mpv_render_param) -> c_int,
    pub mpv_render_context_update: unsafe extern "C" fn(*mut c_void) -> u64,
    pub mpv_render_context_set_update_callback: unsafe extern "C" fn(
        *mut c_void,
        Option<unsafe extern "C" fn(*mut c_void)>,
        *mut c_void,
    ),
    pub mpv_render_context_report_swap: unsafe extern "C" fn(*mut c_void),
    pub mpv_render_context_free: unsafe extern "C" fn(*mut c_void),
}

impl MpvDll {
    /// 从指定路径加载 libmpv DLL。
    ///
    /// 如果加载失败，返回 `libloading::Error`（文件不存在、不是有效的 DLL、缺少导出函数等）。
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, libloading::Error> {
        unsafe {
            let lib = libloading::Library::new(path.as_ref())?;

            // 辅助宏：从 DLL 中读取符号并转成裸函数指针
            macro_rules! get_sym {
                ($name:ident) => {{
                    let sym: libloading::Symbol<
                        unsafe extern "C" fn $( $name )?
                    > = lib.get(concat!(stringify!($name), "\0").as_bytes())?;
                    std::mem::transmute_copy(&*sym)
                }};
            }

            // 由于宏在复杂签名上不好写，直接逐个 get
            // 每个符号单独用块包裹，确保 Symbol 在 lib 被 move 之前已 drop
            let mpv_create = {
                let s: libloading::Symbol<unsafe extern "C" fn() -> *mut mpv_handle> =
                    lib.get(b"mpv_create\0")?;
                unsafe { std::mem::transmute_copy(&*s) }
            };
            let mpv_initialize = {
                let s: libloading::Symbol<unsafe extern "C" fn(*mut mpv_handle) -> c_int> =
                    lib.get(b"mpv_initialize\0")?;
                unsafe { std::mem::transmute_copy(&*s) }
            };
            let mpv_set_option_string = {
                let s: libloading::Symbol<
                    unsafe extern "C" fn(*mut mpv_handle, *const c_char, *const c_char) -> c_int,
                > = lib.get(b"mpv_set_option_string\0")?;
                unsafe { std::mem::transmute_copy(&*s) }
            };
            let mpv_command_string = {
                let s: libloading::Symbol<
                    unsafe extern "C" fn(*mut mpv_handle, *const c_char) -> c_int,
                > = lib.get(b"mpv_command_string\0")?;
                unsafe { std::mem::transmute_copy(&*s) }
            };
            let mpv_wait_event = {
                let s: libloading::Symbol<
                    unsafe extern "C" fn(*mut mpv_handle, c_double) -> *mut mpv_event,
                > = lib.get(b"mpv_wait_event\0")?;
                unsafe { std::mem::transmute_copy(&*s) }
            };
            let mpv_terminate_destroy = {
                let s: libloading::Symbol<unsafe extern "C" fn(*mut mpv_handle)> =
                    lib.get(b"mpv_terminate_destroy\0")?;
                unsafe { std::mem::transmute_copy(&*s) }
            };
            let mpv_set_wakeup_callback = {
                let s: libloading::Symbol<
                    unsafe extern "C" fn(
                        *mut mpv_handle,
                        Option<unsafe extern "C" fn(*mut c_void)>,
                        *mut c_void,
                    ),
                > = lib.get(b"mpv_set_wakeup_callback\0")?;
                unsafe { std::mem::transmute_copy(&*s) }
            };
            let mpv_free = {
                let s: libloading::Symbol<unsafe extern "C" fn(*mut c_void)> =
                    lib.get(b"mpv_free\0")?;
                unsafe { std::mem::transmute_copy(&*s) }
            };
            let mpv_get_property_string = {
                let s: libloading::Symbol<
                    unsafe extern "C" fn(*mut mpv_handle, *const c_char) -> *mut c_char,
                > = lib.get(b"mpv_get_property_string\0")?;
                unsafe { std::mem::transmute_copy(&*s) }
            };
            let mpv_set_property_string = {
                let s: libloading::Symbol<
                    unsafe extern "C" fn(*mut mpv_handle, *const c_char, *const c_char) -> c_int,
                > = lib.get(b"mpv_set_property_string\0")?;
                unsafe { std::mem::transmute_copy(&*s) }
            };
            let mpv_observe_property = {
                let s: libloading::Symbol<
                    unsafe extern "C" fn(*mut mpv_handle, u64, *const c_char, c_int) -> c_int,
                > = lib.get(b"mpv_observe_property\0")?;
                unsafe { std::mem::transmute_copy(&*s) }
            };
            let mpv_command_async = {
                let s: libloading::Symbol<
                    unsafe extern "C" fn(*mut mpv_handle, u64, *const *const c_char) -> c_int,
                > = lib.get(b"mpv_command_async\0")?;
                unsafe { std::mem::transmute_copy(&*s) }
            };
            let mpv_render_context_create = {
                let s: libloading::Symbol<
                    unsafe extern "C" fn(*mut *mut c_void, *mut mpv_handle, *mut mpv_render_param) -> c_int,
                > = lib.get(b"mpv_render_context_create\0")?;
                unsafe { std::mem::transmute_copy(&*s) }
            };
            let mpv_render_context_render = {
                let s: libloading::Symbol<
                    unsafe extern "C" fn(*mut c_void, *mut mpv_render_param) -> c_int,
                > = lib.get(b"mpv_render_context_render\0")?;
                unsafe { std::mem::transmute_copy(&*s) }
            };
            let mpv_render_context_update = {
                let s: libloading::Symbol<unsafe extern "C" fn(*mut c_void) -> u64> =
                    lib.get(b"mpv_render_context_update\0")?;
                unsafe { std::mem::transmute_copy(&*s) }
            };
            let mpv_render_context_set_update_callback = {
                let s: libloading::Symbol<
                    unsafe extern "C" fn(
                        *mut c_void,
                        Option<unsafe extern "C" fn(*mut c_void)>,
                        *mut c_void,
                    ),
                > = lib.get(b"mpv_render_context_set_update_callback\0")?;
                unsafe { std::mem::transmute_copy(&*s) }
            };
            let mpv_render_context_report_swap = {
                let s: libloading::Symbol<unsafe extern "C" fn(*mut c_void)> =
                    lib.get(b"mpv_render_context_report_swap\0")?;
                unsafe { std::mem::transmute_copy(&*s) }
            };
            let mpv_render_context_free = {
                let s: libloading::Symbol<unsafe extern "C" fn(*mut c_void)> =
                    lib.get(b"mpv_render_context_free\0")?;
                unsafe { std::mem::transmute_copy(&*s) }
            };

            Ok(Self {
                _lib: lib,
                mpv_create,
                mpv_initialize,
                mpv_set_option_string,
                mpv_command_string,
                mpv_wait_event,
                mpv_terminate_destroy,
                mpv_set_wakeup_callback,
                mpv_free,
                mpv_get_property_string,
                mpv_set_property_string,
                mpv_observe_property,
                mpv_command_async,
                mpv_render_context_create,
                mpv_render_context_render,
                mpv_render_context_update,
                mpv_render_context_set_update_callback,
                mpv_render_context_report_swap,
                mpv_render_context_free,
            })
        }
    }

    /// 尝试多个常见 DLL 名称，返回第一个成功加载的。
    pub fn load_any<P: AsRef<Path>>(dir: P) -> Option<Self> {
        let dir = dir.as_ref();
        for name in &["mpv-2.dll", "libmpv-2.dll", "mpv.dll"] {
            let p = dir.join(name);
            if p.exists() {
                if let Ok(dll) = Self::load(&p) {
                    return Some(dll);
                }
            }
        }
        None
    }
}

// ============================================================
// 辅助函数
// ============================================================

/// 把 Rust 字符串转成 C 字符串，失败时返回错误信息。
pub fn cstr(s: &str) -> Result<CString, std::ffi::NulError> {
    CString::new(s)
}

/// 读取 `mpv_get_property_string` 返回的字符串并释放内存。
pub fn read_string(dll: &MpvDll, ptr: *mut c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    let s = unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned();
    unsafe { (dll.mpv_free)(ptr as *mut c_void) };
    Some(s)
}

/// 解析属性字符串为 f64。
pub fn parse_f64(dll: &MpvDll, ptr: *mut c_char) -> Option<f64> {
    read_string(dll, ptr)?.parse().ok()
}

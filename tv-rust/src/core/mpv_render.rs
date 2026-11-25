//! libmpv 渲染 API 封装：让 mpv 直接把视频帧画进宿主（egui/glow）的 OpenGL 画布。
//!
//! 为什么不再用 `wid` 嵌入子窗口：
//! - 子窗口会盖住视频区域，egui 收不到鼠标事件，全屏按钮/双击/悬停全部失效；
//! - 窗口全屏/还原时宿主窗口样式变化，子窗口会丢失或错位（画面变黑）；
//! - 跨线程 SetWindowPos 容易和 mpv 渲染线程互相等锁，出现"无响应"。
//!
//! 渲染 API 下 mpv 不创建任何窗口，帧直接渲染到当前 GL 帧缓冲的指定视口，
//! 于是全屏、缩放、叠加控件全部由 egui 自己掌控。

use crate::core::libmpv_ffi::*;
use eframe::glow;
use glow::HasContext;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// 离屏渲染目标（贴图 + FBO），尺寸跟随视频区变化
struct Target {
    tex: glow::Texture,
    fbo: glow::Framebuffer,
    w: i32,
    h: i32,
}

/// mpv 渲染上下文 + 相关状态。
///
/// 创建必须在**当前 GL 上下文有效**的线程上完成，因此这里延迟到第一次绘制
/// （egui 的 paint callback 内）才真正调用 `mpv_render_context_create`。
pub struct MpvRender {
    dll: Arc<MpvDll>,
    /// mpv_handle 指针（仅用于创建渲染上下文）
    mpv: usize,
    /// mpv_render_context 指针（null 表示尚未创建）
    rctx: AtomicPtr<c_void>,
    /// 初始化失败标记（避免每帧重试刷屏）
    failed: AtomicBool,
    /// 失败原因（展示给用户）
    error: Mutex<Option<String>>,
    /// mpv 报告"有新帧"时置位
    pub needs_render: Arc<AtomicBool>,
    /// 视频帧率刷新时用它触发重绘
    repaint_ctx: Mutex<Option<egui::Context>>,
    /// 离屏渲染目标（mpv 先画到这里，再按矩形贴回窗口）
    target: Mutex<Option<Target>>,
}

impl MpvRender {
    pub fn new(dll: Arc<MpvDll>, mpv: *mut mpv_handle) -> Arc<Self> {
        Arc::new(Self {
            dll,
            mpv: mpv as usize,
            rctx: AtomicPtr::new(std::ptr::null_mut()),
            failed: AtomicBool::new(false),
            error: Mutex::new(None),
            needs_render: Arc::new(AtomicBool::new(true)),
            repaint_ctx: Mutex::new(None),
            target: Mutex::new(None),
        })
    }

    /// 记录 egui 上下文，供 mpv 线程请求重绘
    pub fn set_repaint_ctx(&self, ctx: egui::Context) {
        let mut g = self.repaint_ctx.lock().unwrap();
        if g.is_none() {
            *g = Some(ctx);
        }
    }

    /// 初始化/绘制是否失败，失败时返回原因
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }

    pub fn is_failed(&self) -> bool {
        self.failed.load(Ordering::SeqCst)
    }

    /// 创建渲染上下文（必须由持有当前 GL 上下文的线程调用）
    fn ensure_ready(&self) {
        if !self.rctx.load(Ordering::SeqCst).is_null() {
            return;
        }
        if self.failed.load(Ordering::SeqCst) {
            return;
        }

        // API_TYPE 必须是 "opengl" 的 C 字符串，且窗口期间保持有效
        let api_type = CString::new(MPV_RENDER_API_TYPE_OPENGL).unwrap();
        let mut init = mpv_opengl_init_params {
            get_proc_address: Some(gl_get_proc),
            get_proc_address_ctx: std::ptr::null_mut(),
        };

        let mut params = [
            mpv_render_param {
                type_: MPV_RENDER_PARAM_API_TYPE,
                data: api_type.as_ptr() as *mut c_void,
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                data: &mut init as *mut _ as *mut c_void,
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];

        let mut rctx: *mut c_void = std::ptr::null_mut();
        let ret = unsafe {
            (self.dll.mpv_render_context_create)(
                &mut rctx,
                self.mpv as *mut mpv_handle,
                params.as_mut_ptr(),
            )
        };
        if ret < 0 || rctx.is_null() {
            self.failed.store(true, Ordering::SeqCst);
            let msg = format!("mpv_render_context_create 失败: {}", ret);
            crate::core::log::error(&msg);
            *self.error.lock().unwrap() = Some(msg);
            return;
        }

        self.rctx.store(rctx, Ordering::SeqCst);
        crate::core::log::info("libmpv 渲染上下文已创建（OpenGL）");

        // 注册新帧回调：mpv 渲染线程有新帧时请求 egui 重绘
        unsafe {
            (self.dll.mpv_render_context_set_update_callback)(
                rctx,
                Some(on_update),
                self as *const Self as *mut c_void,
            );
        }
    }

    /// 把当前视频帧渲染到离屏 FBO，再按像素矩形贴回窗口帧缓冲。
    ///
    /// 不直接渲染到默认帧缓冲的原因：mpv 会把 GL 视口设成 (0,0,w,h)，
    /// 忽略 egui 给它的偏移，导致画面整体下移、压住进度条和底部按钮栏。
    pub fn paint(&self, gl: &glow::Context, left_px: i32, bottom_px: i32, w: i32, h: i32) {
        self.ensure_ready();
        let rctx = self.rctx.load(Ordering::SeqCst);
        if rctx.is_null() || w <= 0 || h <= 0 {
            return;
        }

        let mut guard = match self.target.lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        // 尺寸变化（窗口缩放/全屏切换）时重建贴图
        if guard.as_ref().map(|t| t.w != w || t.h != h).unwrap_or(true) {
            if let Some(old) = guard.take() {
                unsafe {
                    gl.delete_framebuffer(old.fbo);
                    gl.delete_texture(old.tex);
                }
            }
            match unsafe { create_target(gl, w, h) } {
                Ok(t) => *guard = Some(t),
                Err(e) => {
                    if !self.failed.swap(true, Ordering::SeqCst) {
                        crate::core::log::error(format!("创建视频渲染目标失败: {e}"));
                        *self.error.lock().unwrap() = Some(format!("创建视频渲染目标失败: {e}"));
                    }
                    return;
                }
            }
        }
        let Some(target) = guard.as_ref() else { return };

        self.needs_render.store(false, Ordering::SeqCst);
        unsafe {
            // 让 mpv 准备下一帧
            (self.dll.mpv_render_context_update)(rctx);
        }

        let mut fbo_info = mpv_opengl_fbo {
            fbo: target.fbo.0.get() as i32,
            w,
            h,
            internal_format: glow::RGBA8 as i32,
        };
        let mut flip_y: c_int = 1; // 输出上屏时的方向

        let mut params = [
            mpv_render_param {
                type_: MPV_RENDER_PARAM_OPENGL_FBO,
                data: &mut fbo_info as *mut _ as *mut c_void,
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_FLIP_Y,
                data: &mut flip_y as *mut _ as *mut c_void,
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_INVALID,
                data: std::ptr::null_mut(),
            },
        ];

        let ret = unsafe { (self.dll.mpv_render_context_render)(rctx, params.as_mut_ptr()) };
        if ret < 0 {
            // 只记录一次，避免每帧刷日志
            if !self.failed.swap(true, Ordering::SeqCst) {
                let msg = format!("mpv 渲染帧失败: {}", ret);
                crate::core::log::error(&msg);
                *self.error.lock().unwrap() = Some(msg);
            }
            return;
        }

        // 把离屏画面贴回窗口的指定矩形（这里才是真正的定位，不会被栏位挡住）
        unsafe {
            gl.disable(glow::SCISSOR_TEST);
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(target.fbo));
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, None);
            gl.blit_framebuffer(
                0,
                0,
                w,
                h,
                left_px,
                bottom_px,
                left_px + w,
                bottom_px + h,
                glow::COLOR_BUFFER_BIT,
                glow::NEAREST,
            );
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        }
    }

    /// 释放渲染上下文（必须在销毁 mpv 核心之前调用，且在同一线程）
    pub fn shutdown(&self) {
        let rctx = self.rctx.swap(std::ptr::null_mut(), Ordering::SeqCst);
        if !rctx.is_null() {
            unsafe { (self.dll.mpv_render_context_free)(rctx) };
        }
    }
}

impl Drop for MpvRender {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ========== 离屏渲染目标 ==========

/// 创建一块 RGBA8 贴图 + 完整 FBO（mpv 往上画，随后 blit 到窗口）
unsafe fn create_target(gl: &glow::Context, w: i32, h: i32) -> Result<Target, String> {
    let tex = unsafe { gl.create_texture() }?;
    unsafe {
        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            w,
            h,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
        gl.bind_texture(glow::TEXTURE_2D, None);
    }

    let fbo = unsafe { gl.create_framebuffer() }?;
    unsafe {
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
        gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(tex),
            0,
        );
        let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
        gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        if status != glow::FRAMEBUFFER_COMPLETE {
            gl.delete_framebuffer(fbo);
            gl.delete_texture(tex);
            return Err(format!("FBO 不完整: 0x{status:x}"));
        }
    }

    Ok(Target { tex, fbo, w, h })
}

// ========== mpv 渲染线程 → egui 重绘 ==========

unsafe extern "C" fn on_update(ctx: *mut c_void) {
    if ctx.is_null() {
        return;
    }
    let this = unsafe { &*(ctx as *const MpvRender) };
    this.needs_render.store(true, Ordering::SeqCst);
    if let Ok(g) = this.repaint_ctx.lock() {
        if let Some(c) = g.as_ref() {
            c.request_repaint();
        }
    }
}

// ========== OpenGL 函数指针解析 ==========

#[cfg(windows)]
unsafe extern "C" fn gl_get_proc(_ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    type WglGetProc = unsafe extern "system" fn(*const c_char) -> *mut c_void;
    static API: OnceLock<Option<(libloading::Library, Option<WglGetProc>)>> = OnceLock::new();

    let api = API.get_or_init(|| {
        let lib = libloading::Library::new("opengl32.dll").ok()?;
        let wgl = unsafe {
            lib.get::<WglGetProc>(b"wglGetProcAddress\0")
                .ok()
                .map(|s| *s)
        };
        Some((lib, wgl))
    });

    let Some((lib, wgl)) = api else {
        return std::ptr::null_mut();
    };

    // 优先 wglGetProcAddress（GL 1.2+ 扩展函数只有它认得）
    if let Some(f) = wgl {
        let p = unsafe { f(name) };
        // wglGetProcAddress 失败时会返回 1/2/3/-1 这类哨兵值
        let bad = p.is_null() || p as isize == 1 || p as isize == 2 || p as isize == 3 || p as isize == -1;
        if !bad {
            return p;
        }
    }

    // GL 1.1 的函数是 opengl32.dll 的直接导出
    let cname = unsafe { CStr::from_ptr(name) };
    if let Ok(sym) = unsafe { lib.get::<unsafe extern "C" fn()>(cname.to_bytes_with_nul()) } {
        return unsafe { *sym } as usize as *mut c_void;
    }
    std::ptr::null_mut()
}

#[cfg(not(windows))]
unsafe extern "C" fn gl_get_proc(_ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    type GetProc = unsafe extern "system" fn(*const u8) -> *mut c_void;
    static API: OnceLock<Option<GetProc>> = OnceLock::new();

    let f = API.get_or_init(|| {
        // EGL 优先（Wayland/EGL 环境），再退回 GLX
        for lib_name in ["libEGL.so.1", "libGL.so.1"] {
            if let Ok(lib) = unsafe { libloading::Library::new(lib_name) } {
                // ManuallyDrop 避免 borrow + move 冲突：函数指针提取后
                // Library 故意不释放（GL 函数指针要在整个进程内有效）。
                let lib = std::mem::ManuallyDrop::new(lib);
                for sym in [b"eglGetProcAddress\0".as_slice(), b"glXGetProcAddressARB\0".as_slice()] {
                    if let Ok(s) = unsafe { lib.get::<GetProc>(sym) } {
                        return Some(unsafe { *s });
                    }
                }
            }
        }
        None
    });

    match f {
        Some(f) => unsafe { f(name as *const u8) },
        None => std::ptr::null_mut(),
    }
}

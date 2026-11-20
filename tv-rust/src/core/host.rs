//! spider-host 子进程管理：本地 HTTP JSON API 客户端。
//! - 启动 `java -jar spider-host-all.jar`
//! - stdout 仅承载握手：首行 `hello` 内含 `api`/`apiPort`、`proxy` 等实际地址
//! - 业务调用：`POST http://127.0.0.1:<apiPort>/api/rpc`，报文 `{"method":"...","params":{...}}`
//! - 响应：`{"ok":true,"result":...,"error":null,"elapsedMs":N}`（错误时 `ok=false`）
//! - stdin 仅用于优雅退出（`quit`）；stderr 日志落盘到 spider-host.log
//!
//! 与旧 stdio 协议相比：不再需要按 id 匹配乱序响应，超时/并发由 HTTP 客户端与宿主线程池处理。

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct SpiderHost {
    child: Mutex<Child>,
    sink: Mutex<ChildStdin>,
    http: reqwest::blocking::Client,
    api_base: Arc<Mutex<Option<String>>>,
    alive: Arc<AtomicBool>,
    pub hello: Arc<Mutex<Option<String>>>,
    /// 宿主内置代理服务地址（hello 中下发），如 http://127.0.0.1:9978
    proxy_base: Arc<Mutex<Option<String>>>,
}

impl SpiderHost {
    pub fn spawn(java: &str, jar: &str, jvm_args: &[String]) -> Result<Arc<SpiderHost>> {
        let mut cmd = Command::new(java);
        // JVM 参数必须在 -jar 之前，否则会被当作应用程序参数
        for a in jvm_args {
            cmd.arg(a);
        }
        cmd.arg("-jar").arg(jar);
        // 宿主日志写 stderr。打包版是无控制台的 GUI 程序，inherit 会让日志全部丢失，
        // 因此改为管道读取并落盘到 %AppData%\TV-Multiplatform\cache\rust\spider-host.log
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NO_WINDOW：避免宿主进程弹出控制台窗口
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn().map_err(|e| anyhow!("启动 spider-host 失败 (java={}): {}", java, e))?;
        let stdin = child.stdin.take().expect("stdin piped");
        let stdout = child.stdout.take().expect("stdout piped");

        // 宿主 stderr -> 本地日志文件（同时记录 spawn 命令便于复现）
        crate::core::log::info(format!("spider-host spawn: {} {} -jar {}", java, jvm_args.join(" "), jar));
        if let Some(stderr) = child.stderr.take() {
            std::thread::Builder::new()
                .name("spider-host-stderr".into())
                .spawn(move || {
                    let mut file = crate::core::log::open_host_log();
                    let mut reader = BufReader::new(stderr);
                    let mut line = String::new();
                    loop {
                        line.clear();
                        match reader.read_line(&mut line) {
                            Ok(0) | Err(_) => break,
                            Ok(_) => {
                                let text = line.trim_end();
                                if text.is_empty() {
                                    continue;
                                }
                                if let Some(f) = file.as_mut() {
                                    let _ = writeln!(f, "{}", text);
                                    let _ = f.flush();
                                }
                                if cfg!(debug_assertions) {
                                    eprintln!("[host] {}", text);
                                }
                            }
                        }
                    }
                    crate::core::log::warn("spider-host stderr 流已结束（进程退出或崩溃）");
                })
                .ok();
        }

        let http = reqwest::blocking::Client::builder()
            // 宿主只监听回环地址，绝不能走系统代理
            .no_proxy()
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(|e| anyhow!("创建宿主 HTTP 客户端失败: {}", e))?;

        let alive = Arc::new(AtomicBool::new(true));
        let hello = Arc::new(Mutex::new(None));
        let proxy_base = Arc::new(Mutex::new(None));
        let api_base: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

        // 读取握手行；stdout 仅用于 hello/退出信号
        {
            let alive = Arc::clone(&alive);
            let hello = Arc::clone(&hello);
            let proxy_base = Arc::clone(&proxy_base);
            let api_base = Arc::clone(&api_base);
            std::thread::Builder::new()
                .name("spider-host-reader".into())
                .spawn(move || {
                    let mut reader = BufReader::new(stdout);
                    let mut buf: Vec<u8> = Vec::with_capacity(8 * 1024);
                    loop {
                        buf.clear();
                        match reader.read_until(b'\n', &mut buf) {
                            Ok(0) => break, // EOF：宿主退出
                            Ok(_) => {}
                            Err(_) => break,
                        }
                        let line = String::from_utf8_lossy(&buf);
                        let line = line.trim();
                        if line.is_empty() {
                            continue;
                        }
                        let v: Value = match serde_json::from_str(line) {
                            Ok(v) => v,
                            Err(_) => continue, // 忽略非协议输出
                        };
                        let id = v.get("id").and_then(|x| x.as_u64()).unwrap_or(u64::MAX);
                        let is_hello = id == 0
                            || v.get("method").and_then(|x| x.as_str()) == Some("hello");
                        if is_hello {
                            let version = v.get("version").and_then(|x| x.as_str()).unwrap_or("?");
                            let java = v.get("java").and_then(|x| x.as_str()).unwrap_or("?");
                            *hello.lock().unwrap() =
                                Some(format!("spider-host {} (java {})", version, java));

                            let api = v
                                .get("api")
                                .and_then(|x| x.as_str())
                                .filter(|s| !s.is_empty())
                                .map(str::to_string)
                                .or_else(|| {
                                    v.get("apiPort")
                                        .and_then(|x| x.as_i64())
                                        .filter(|p| *p > 0)
                                        .map(|p| format!("http://127.0.0.1:{}", p))
                                });
                            if let Some(api) = api {
                                crate::core::log::info(format!("spider-host api: {}", api));
                                *api_base.lock().unwrap() = Some(api);
                            } else {
                                crate::core::log::error(
                                    "spider-host 未下发 api 地址（需要 1.2.0+ 宿主，请重新构建 spider-host-all.jar）",
                                );
                            }

                            if let Some(p) = v.get("proxy").and_then(|x| x.as_str()) {
                                if !p.is_empty() {
                                    *proxy_base.lock().unwrap() = Some(p.to_string());
                                }
                            }
                            continue;
                        }
                    }
                    alive.store(false, Ordering::SeqCst);
                    crate::core::log::warn("spider-host stdout 已关闭（宿主进程退出）");
                })?;
        }

        Ok(Arc::new(SpiderHost {
            child: Mutex::new(child),
            sink: Mutex::new(stdin),
            http,
            api_base,
            alive,
            hello,
            proxy_base,
        }))
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    pub fn hello(&self) -> Option<String> {
        self.hello.lock().unwrap().clone()
    }

    /// 宿主 HTTP API 根地址（如 http://127.0.0.1:9980），未就绪时返回 None。
    /// 主要供诊断/设置页显示，业务调用请用 [`SpiderHost::request`]。
    #[allow(dead_code)]
    pub fn api_base(&self) -> Option<String> {
        self.api_base.lock().unwrap().clone()
    }

    /// 宿主内置代理服务根地址（如 http://127.0.0.1:9978），未就绪时返回 None
    pub fn proxy_base(&self) -> Option<String> {
        self.proxy_base.lock().unwrap().clone()
    }

    /// 阻塞请求（请在工作线程调用，不要在 UI 线程调用）
    pub fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let deadline = Instant::now() + timeout;
        let base = self.wait_api_base(deadline)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(anyhow!("请求超时 ({}): {}s", method, timeout.as_secs()));
        }

        let body = json!({"method": method, "params": params});
        let resp = self
            .http
            .post(format!("{}/api/rpc", base))
            .json(&body)
            .timeout(remaining)
            .send()
            .map_err(|e| {
                if e.is_timeout() {
                    anyhow!("请求超时 ({}): {}s", method, timeout.as_secs())
                } else if e.is_connect() {
                    anyhow!("连接宿主 HTTP API 失败 ({}): {}", method, e)
                } else {
                    anyhow!("请求宿主失败 ({}): {}", method, e)
                }
            })?;

        let status = resp.status();
        let text = resp.text().unwrap_or_default();
        let parsed: Value = serde_json::from_str(&text).map_err(|e| {
            let snippet: String = text.chars().take(200).collect();
            anyhow!("宿主响应不是 JSON ({}): {} — {}", status, e, snippet)
        })?;

        let ok = parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        if !status.is_success() || !ok {
            let err = parsed
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("宿主返回错误");
            return Err(anyhow!("{}", err));
        }
        Ok(parsed.get("result").cloned().unwrap_or(Value::Null))
    }

    /// 等待 hello 下发 api 地址；HTTP API 未启动时不要在 UI 线程阻塞过久
    fn wait_api_base(&self, deadline: Instant) -> Result<String> {
        loop {
            if let Some(base) = self.api_base.lock().unwrap().clone() {
                return Ok(base);
            }
            if !self.is_alive() {
                return Err(anyhow!("宿主进程已退出（HTTP API 未就绪）"));
            }
            if Instant::now() >= deadline {
                return Err(anyhow!(
                    "spider-host HTTP API 未就绪（请确认宿主版本 >= 1.2.0 且端口未被防火墙拦截）"
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// 便捷方法：Spider 系列方法的 result 是内嵌 JSON 字符串，直接解析为 SpiderResult
    pub fn request_spider_result(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<crate::core::models::SpiderResult> {
        let raw = self.request(method, params, timeout)?;
        match raw {
            Value::String(s) => {
                if s.trim().is_empty() {
                    return Ok(crate::core::models::SpiderResult::default());
                }
                crate::core::models::SpiderResult::from_raw(&s)
            }
            // 站点 init 失败时宿主返回 {data: "<json>", warning: "..."}
            Value::Object(map) => {
                let warning = map.get("warning").and_then(|v| v.as_str()).map(|s| s.to_string());
                let data = map.get("data").cloned().unwrap_or(Value::Null);
                let mut result = match data {
                    Value::String(s) if !s.trim().is_empty() => {
                        crate::core::models::SpiderResult::from_raw(&s)?
                    }
                    Value::Null => crate::core::models::SpiderResult::default(),
                    other => crate::core::models::SpiderResult::from_raw(&other.to_string())?,
                };
                result.warning = warning;
                Ok(result)
            }
            Value::Null => Ok(crate::core::models::SpiderResult::default()),
            other => crate::core::models::SpiderResult::from_raw(&other.to_string()),
        }
    }

    pub fn shutdown(&self) {
        // 1) 优先走 HTTP 优雅退出（宿主导出接口先回包，再 halt）
        if let Some(base) = self.api_base.lock().unwrap().clone() {
            let _ = self
                .http
                .post(format!("{}/api/shutdown", base))
                .timeout(Duration::from_millis(800))
                .send();
        }
        // 2) stdin quit 兜底（旧版宿主/HTTP 端口异常时）
        if let Ok(mut stdin) = self.sink.lock() {
            let _ = writeln!(stdin, "quit");
            let _ = stdin.flush();
        }
        std::thread::sleep(Duration::from_millis(350));
        if let Ok(mut child) = self.child.lock() {
            // /api/shutdown 会在 250ms 后自行 halt；已退出则不再强杀
            if let Ok(Some(_)) = child.try_wait() {
                return;
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl Drop for SpiderHost {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

//! spider-host 子进程管理：stdio JSON 行协议客户端。
//! - 启动 java -jar spider-host-all.jar
//! - 请求 {"id":N,"method":"...","params":{...}}，响应 {"id":N,"ok":...,"result":...,"error":...}
//! - 响应可能乱序返回（宿主并发处理），按 id 匹配
//! - stdout 专用于协议；宿主日志走 stderr（inherit 到本进程 stderr）

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct HostResp {
    pub ok: bool,
    pub result: Value,
    pub error: Option<String>,
}

pub struct SpiderHost {
    child: Mutex<Child>,
    stdin: Mutex<ChildStdin>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, SyncSender<HostResp>>>>,
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

        let pending: Arc<Mutex<HashMap<u64, SyncSender<HostResp>>>> = Arc::new(Mutex::new(HashMap::new()));
        let alive = Arc::new(AtomicBool::new(true));
        let hello = Arc::new(Mutex::new(None));
        let proxy_base = Arc::new(Mutex::new(None));

        // 读取线程
        {
            let pending = Arc::clone(&pending);
            let alive = Arc::clone(&alive);
            let hello = Arc::clone(&hello);
            let proxy_base = Arc::clone(&proxy_base);
            std::thread::Builder::new()
                .name("spider-host-reader".into())
                .spawn(move || {
                    let mut reader = BufReader::new(stdout);
                    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
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
                        let id = v.get("id").and_then(|x| x.as_u64()).unwrap_or(0);
                        if id == 0 {
                            let info = format!(
                                "spider-host {} (java {})",
                                v.get("version").and_then(|x| x.as_str()).unwrap_or("?"),
                                v.get("java").and_then(|x| x.as_str()).unwrap_or("?")
                            );
                            *hello.lock().unwrap() = Some(info);
                            if let Some(p) = v.get("proxy").and_then(|x| x.as_str()) {
                                if !p.is_empty() {
                                    *proxy_base.lock().unwrap() = Some(p.to_string());
                                }
                            }
                            continue;
                        }
                        let resp = HostResp {
                            ok: v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false),
                            result: v.get("result").cloned().unwrap_or(Value::Null),
                            error: v.get("error").and_then(|x| x.as_str()).map(|s| s.to_string()),
                        };
                        let sender = pending.lock().unwrap().remove(&id);
                        if let Some(tx) = sender {
                            let _ = tx.try_send(resp);
                        }
                    }
                    alive.store(false, Ordering::SeqCst);
                    // 唤醒所有等待者
                    let mut map = pending.lock().unwrap();
                    for (_, tx) in map.drain() {
                        let _ = tx.try_send(HostResp {
                            ok: false,
                            result: Value::Null,
                            error: Some("宿主进程已退出".into()),
                        });
                    }
                })?;
        }

        Ok(Arc::new(SpiderHost {
            child: Mutex::new(child),
            stdin: Mutex::new(stdin),
            next_id: AtomicU64::new(1),
            pending,
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

    /// 宿主内置代理服务根地址（如 http://127.0.0.1:9978），未就绪时返回 None
    pub fn proxy_base(&self) -> Option<String> {
        self.proxy_base.lock().unwrap().clone()
    }

    /// 阻塞请求（请在工作线程调用，不要在 UI 线程调用）
    pub fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        if !self.is_alive() {
            return Err(anyhow!("spider-host 未运行"));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = sync_channel::<HostResp>(1);
        self.pending.lock().unwrap().insert(id, tx);

        let line = json!({"id": id, "method": method, "params": params});
        {
            let mut stdin = self.stdin.lock().unwrap();
            writeln!(stdin, "{}", line).map_err(|e| {
                self.pending.lock().unwrap().remove(&id);
                anyhow!("发送请求失败: {}", e)
            })?;
            stdin.flush().ok();
        }

        match rx.recv_timeout(timeout) {
            Ok(resp) => {
                if resp.ok {
                    Ok(resp.result)
                } else {
                    Err(anyhow!(resp.error.unwrap_or_else(|| "宿主返回错误".into())))
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                self.pending.lock().unwrap().remove(&id);
                Err(anyhow!("请求超时 ({}): {}s", method, timeout.as_secs()))
            }
            Err(_) => Err(anyhow!("与宿主的连接已断开")),
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
        // 尝试优雅退出
        let line = json!({"id": 0, "method": "quit"});
        {
            let mut stdin = self.stdin.lock().unwrap();
            let _ = writeln!(stdin, "{}", line);
            let _ = stdin.flush();
        }
        std::thread::sleep(Duration::from_millis(150));
        if let Ok(mut child) = self.child.lock() {
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

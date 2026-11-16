package com.corner.host

import com.corner.catvodcore.util.Jsons
import kotlinx.serialization.json.*
import java.io.BufferedReader
import java.io.File
import java.io.FileDescriptor
import java.io.FileOutputStream
import java.io.FilterOutputStream
import java.io.InputStreamReader
import java.io.OutputStream
import java.io.PrintStream
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

// spider-host 入口：
//   stdin  接收 JSON 行请求： {"id":N,"method":"...","params":{...}}
//   stdout 回写 JSON 行响应：{"id":N,"ok":true,"result":...,"error":null}
//   stderr 日志输出（logback 配置 target=System.err）
// 启动后先输出一行 hello（id=0）作为握手信号。
fun main() {
    // 必须先于任何 logger 初始化：logback 用它决定日志文件目录
    System.setProperty("tv.log.dir", resolveLogDir())

    // 保留真实 stdout 专用于协议；爬虫/依赖库对 System.out 的直接输出一律重定向到 stderr
    // 注意：fd 1/2 的生命周期属于整个进程，PrintStream.close() 会连带关掉底层 FileDescriptor。
    // 旧写法把 System.out 又包了一层 fd 2，任何第三方库调用 System.out.close() 都会把 stderr
    // 一起关掉——Rust 侧读到管道 EOF，误判“宿主退出或崩溃”，同时丢掉之后所有宿主日志。
    val stdout = PrintStream(shield(FileDescriptor.out), true, Charsets.UTF_8)
    val stderr = PrintStream(shield(FileDescriptor.err), true, Charsets.UTF_8)
    System.setOut(stderr)
    System.setErr(stderr)

    val cores = Runtime.getRuntime().availableProcessors()
    val pool = Executors.newFixedThreadPool(maxOf(4, cores - 2))

    // 先起本地代理端口：jar 爬虫生成的图片/播放地址里硬编码了 127.0.0.1:9978/proxy
    val proxyPort = ProxyServer.start()

    fun respond(id: Long, ok: Boolean, result: JsonElement? = null, error: String? = null): String {
        return buildJsonObject {
            put("id", id)
            put("ok", ok)
            put("result", result ?: JsonNull)
            put("error", error?.let { JsonPrimitive(it) } ?: JsonNull)
        }.toString()
    }

    stdout.println(
        buildJsonObject {
            put("id", 0L)
            put("ok", true)
            put("method", JsonPrimitive("hello"))
            put("result", JsonPrimitive("spider-host ready"))
            put("version", JsonPrimitive("1.1.0"))
            put("java", JsonPrimitive(System.getProperty("java.version")))
            put("proxyPort", JsonPrimitive(proxyPort))
            put("proxy", JsonPrimitive(ProxyServer.baseUrl))
        }.toString()
    )

    val reader = BufferedReader(InputStreamReader(System.`in`, Charsets.UTF_8))
    while (true) {
        val line = try {
            reader.readLine() ?: break
        } catch (e: Exception) {
            break
        }
        if (line.isBlank()) continue

        val req = runCatching { Jsons.parseToJsonElement(line).jsonObject }.getOrNull()
        if (req == null) {
            stdout.println(respond(-1L, false, error = "bad request line"))
            continue
        }

        val id = (req["id"] as? JsonPrimitive)?.content?.toLongOrNull() ?: -1L
        val method = (req["method"] as? JsonPrimitive)?.content ?: ""
        val params = req["params"] as? JsonObject ?: JsonObject(emptyMap())

        if (method == "quit") {
            stdout.println(respond(id, true, JsonPrimitive("bye")))
            break
        }
        if (method == "ping") {
            stdout.println(respond(id, true, JsonPrimitive("pong java=" + System.getProperty("java.version"))))
            continue
        }

        pool.submit {
            val t0 = System.currentTimeMillis()
            val siteKey = (params["siteKey"] as? JsonPrimitive)?.content.orEmpty()
            val rpcLog = org.slf4j.LoggerFactory.getLogger("host.rpc")
            // 入参与结果都落盘（截断），用于判断“没内容”是站点返回空还是解析失败
            rpcLog.info("rpc >> {} id={} site={} params={}", method, id, siteKey, describeParams(method, params))
            val response = try {
                val result = Host.handle(method, params)
                rpcLog.info(
                    "rpc << {} ok {}ms site={} result={}",
                    method, System.currentTimeMillis() - t0, siteKey, clip(result?.toString(), 2000)
                )
                respond(id, true, result)
            } catch (e: Throwable) {
                rpcLog.error(
                    "rpc << {} failed {}ms site={} error={}",
                    method, System.currentTimeMillis() - t0, siteKey, "${e.javaClass.simpleName}: ${e.message}"
                )
                rpcLog.debug("rpc failure detail {}", method, e)
                respond(id, false, error = "${e.javaClass.simpleName}: ${e.message}")
            }
            synchronized(stdout) { stdout.println(response) }
        }
    }

    pool.shutdown()
    pool.awaitTermination(3, TimeUnit.SECONDS)
    ProxyServer.stop()
    Runtime.getRuntime().halt(0)
}

/// 入参可能很大（loadConfig 携带整份配置），日志里做截断，避免日志爆掉
private fun describeParams(method: String, params: JsonObject): String {
    if (method == "loadConfig") {
        val data = (params["data"] as? JsonPrimitive)?.content.orEmpty()
        val url = (params["url"] as? JsonPrimitive)?.content.orEmpty()
        return "url=$url dataLen=${data.length}"
    }
    return clip(params.toString(), 800)
}

private fun clip(text: String?, max: Int): String = when {
    text == null -> "null"
    text.length <= max -> text
    else -> text.take(max) + "…(共 ${text.length} 字符)"
}

/// 包一层「close 只 flush」的输出流：fd 属于进程，任何库把它关掉都会连带废掉
/// System.out / System.err，导致宿主与 UI 的日志通道彻底断开。
private fun shield(fd: FileDescriptor): OutputStream = object : FilterOutputStream(FileOutputStream(fd)) {
    override fun close() {
        flush()
    }

    // 默认实现按单字节 write，日志量大时会明显拖慢
    override fun write(b: ByteArray, off: Int, len: Int) {
        out.write(b, off, len)
    }
}

/// 日志目录：与缓存同根（%AppData%\TV-Multiplatform\cache\log），
/// 不能走 logger，否则会与 logback 初始化形成递归。
private fun resolveLogDir(): String {
    val os = System.getProperty("os.name").orEmpty().lowercase()
    val cache = when {
        os.contains("win") -> {
            val roaming = System.getenv("AppData")
                ?: File(System.getProperty("user.home"), "AppData").resolve("Roaming").path
            File(roaming, "TV-Multiplatform/cache")
        }
        os.contains("mac") || os.contains("darwin") -> File(System.getProperty("user.home"), "Library/Caches/TV-Multiplatform")
        else -> File(System.getProperty("user.home"), ".cache/TV-Multiplatform")
    }
    val dir = File(cache, "log")
    runCatching { dir.mkdirs() }
    return dir.absolutePath
}

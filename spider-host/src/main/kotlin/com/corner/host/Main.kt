package com.corner.host

import com.corner.catvodcore.util.Jsons
import kotlinx.serialization.json.*
import org.slf4j.LoggerFactory
import java.io.BufferedReader
import java.io.File
import java.io.FileDescriptor
import java.io.FileOutputStream
import java.io.FilterOutputStream
import java.io.InputStreamReader
import java.io.OutputStream
import java.io.PrintStream

// spider-host 入口：
//   HTTP 业务 API：http://127.0.0.1:<apiPort>/api/**（Swagger: /swagger）
//   stdout 仅用于进程生命周期：启动后输出一行 hello（含 api/proxy 实际端口）
//   stdin 仅用于生命周期：收到 quit 或 EOF 时退出（业务请求不再走 stdio）
//   stderr 日志输出（logback 配置 target=System.err）
fun main() {
    // 必须先于任何 logger 初始化：logback 用它决定日志文件目录
    System.setProperty("tv.log.dir", resolveLogDir())

    // 保留真实 stdout 专用于握手；爬虫/依赖库对 System.out 的直接输出一律重定向到 stderr
    // 注意：fd 1/2 的生命周期属于整个进程，PrintStream.close() 会连带关掉底层 FileDescriptor。
    // 旧写法把 System.out 又包了一层 fd 2，任何第三方库调用 System.out.close() 都会把 stderr
    // 一起关掉——Rust 侧读到管道 EOF，误判“宿主退出或崩溃”，同时丢掉之后所有宿主日志。
    val stdout = PrintStream(shield(FileDescriptor.out), true, Charsets.UTF_8)
    val stderr = PrintStream(shield(FileDescriptor.err), true, Charsets.UTF_8)
    System.setOut(stderr)
    System.setErr(stderr)
    val log = LoggerFactory.getLogger("host.main")

    // 先起本地代理端口：jar 爬虫生成的图片/播放地址里硬编码了 127.0.0.1:9978/proxy
    val proxyPort = ProxyServer.start()

    // 业务 API 只监听回环地址；端口被占用时自动顺延，最终端口写进 hello
    val apiPort = HttpApiServer.start()
    HttpApiServer.shutdownHook = { shutdownHost() }

    fun helloPayload(): String = buildJsonObject {
        put("id", 0L)
        put("ok", true)
        put("method", "hello")
        put("result", "spider-host ready")
        put("version", HOST_VERSION)
        put("java", System.getProperty("java.version"))
        put("apiPort", apiPort)
        put("api", HttpApiServer.baseUrl)
        put("swagger", "${HttpApiServer.baseUrl}/swagger")
        put("openapi", "${HttpApiServer.baseUrl}/openapi.json")
        put("proxyPort", proxyPort)
        put("proxy", ProxyServer.baseUrl)
    }.toString()

    stdout.println(helloPayload())
    stdout.flush()

    if (apiPort <= 0) {
        log.error("HTTP API 启动失败，宿主无法提供业务能力，退出")
        shutdownHost(1)
    }

    log.info("spider-host {} ready: api={} swagger={}/swagger proxy={}",
        HOST_VERSION, HttpApiServer.baseUrl, HttpApiServer.baseUrl, ProxyServer.baseUrl)

    // 只处理生命周期：quit 或 stdin EOF（父进程退出）。业务请求已全部迁移到 HTTP。
    val reader = BufferedReader(InputStreamReader(System.`in`, Charsets.UTF_8))
    while (true) {
        val line = try {
            reader.readLine() ?: break
        } catch (_: Exception) {
            break
        }
        val trimmed = line.trim()
        if (trimmed.isEmpty()) continue
        val quit = trimmed == "quit" || runCatching {
            val obj = Jsons.parseToJsonElement(trimmed).jsonObject
            (obj["method"] as? JsonPrimitive)?.content == "quit"
        }.getOrDefault(false)
        if (quit) break
        log.warn("忽略 stdio 请求（业务 API 已迁移到 HTTP）: {}", trimmed.take(200))
    }

    shutdownHost()
}

/// 统一退出路径：停 HTTP API、停代理、halt。
/// halt 而不是 exitProcess：避免爬虫/第三方库注册的 shutdown hook 卡住进程。
private fun shutdownHost(code: Int = 0) {
    runCatching { HttpApiServer.stop() }
    runCatching { ProxyServer.stop() }
    Runtime.getRuntime().halt(code)
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

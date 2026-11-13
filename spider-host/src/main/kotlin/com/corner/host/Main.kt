package com.corner.host

import com.corner.catvodcore.util.Jsons
import kotlinx.serialization.json.*
import java.io.BufferedReader
import java.io.FileDescriptor
import java.io.FileOutputStream
import java.io.InputStreamReader
import java.io.PrintStream
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

// spider-host 入口：
//   stdin  接收 JSON 行请求： {"id":N,"method":"...","params":{...}}
//   stdout 回写 JSON 行响应：{"id":N,"ok":true,"result":...,"error":null}
//   stderr 日志输出（logback 配置 target=System.err）
// 启动后先输出一行 hello（id=0）作为握手信号。
fun main() {
    // 保留真实 stdout 专用于协议；爬虫/依赖库对 System.out 的直接输出一律重定向到 stderr
    val stdout = PrintStream(FileOutputStream(FileDescriptor.out), true, Charsets.UTF_8)
    System.setOut(PrintStream(FileOutputStream(FileDescriptor.err), true, Charsets.UTF_8))

    val cores = Runtime.getRuntime().availableProcessors()
    val pool = Executors.newFixedThreadPool(maxOf(4, cores - 2))

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
            put("version", JsonPrimitive("1.0.0"))
            put("java", JsonPrimitive(System.getProperty("java.version")))
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
            val response = try {
                val result = Host.handle(method, params)
                respond(id, true, result)
            } catch (e: Throwable) {
                org.slf4j.LoggerFactory.getLogger("host").error("method {} failed", method, e)
                respond(id, false, error = "${e.javaClass.simpleName}: ${e.message}")
            }
            synchronized(stdout) { stdout.println(response) }
        }
    }

    pool.shutdown()
    pool.awaitTermination(3, TimeUnit.SECONDS)
    Runtime.getRuntime().halt(0)
}

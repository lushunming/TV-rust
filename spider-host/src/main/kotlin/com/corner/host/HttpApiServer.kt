package com.corner.host

import com.corner.catvodcore.util.Jsons
import fi.iki.elonen.NanoHTTPD
import kotlinx.serialization.json.*
import org.slf4j.LoggerFactory
import java.io.ByteArrayInputStream
import java.util.concurrent.ConcurrentHashMap

/** 宿主版本号：hello 握手、Swagger 文档、HTTP 健康检查共用 */
internal const val HOST_VERSION = "1.2.0"

/** 静态资源缓存（openapi.json / swagger 页面 / swagger-ui 资产） */
private val apiAssetCache = ConcurrentHashMap<String, ByteArray>()

/** 请求体上限：配置源可能较大，但不应无限占用内存 */
private const val MAX_BODY_BYTES = 32L * 1024 * 1024

/**
 * spider-host 的本地 HTTP API（仅监听 127.0.0.1）。
 *
 * 设计目标：
 * - 业务调用统一走 HTTP：`POST /api/rpc` 与语义化 `/api/...` 端点，stdin/stdout 不再承载业务请求；
 * - 自带 Swagger UI（`/swagger`）与 OpenAPI 3 描述（`/openapi.json`），可直接在浏览器里调试；
 * - 与 `/proxy` 端口分离：代理仍由 ProxyServer 提供（默认 9978），API 默认 9980。
 *
 * 端口策略：优先 DEFAULT_PORT..MAX_PORT，全部占用时退到系统临时端口，实际端口写进 hello 握手行。
 */
object HttpApiServer {
    private val log = LoggerFactory.getLogger(HttpApiServer::class.java)

    const val DEFAULT_PORT = 9980
    private const val MAX_PORT = 9999
    private const val SWAGGER_UI_VERSION = "5.17.14"

    @Volatile
    private var server: NanoHTTPD? = null

    /** 实际监听端口，-1 表示未启动 */
    @Volatile
    var port: Int = -1
        private set

    @Volatile
    private var startedAt: Long = 0

    /** `/api/shutdown` 的实际退出动作，由 Main 注入（避免本类反向依赖进程入口） */
    @Volatile
    var shutdownHook: (() -> Unit)? = null

    val baseUrl: String
        get() = if (port > 0) "http://127.0.0.1:$port" else ""

    fun uptimeMs(): Long = if (startedAt > 0) System.currentTimeMillis() - startedAt else 0

    fun start(preferredPort: Int = DEFAULT_PORT): Int {
        if (port > 0) return port
        val candidates = if (preferredPort <= 0) {
            listOf(0)
        } else {
            (preferredPort..MAX_PORT).toList() + 0
        }
        for (p in candidates) {
            try {
                val s = Handler(p)
                // daemon 线程：进程退出时无需等待 HTTP 线程
                s.start(NanoHTTPD.SOCKET_READ_TIMEOUT, true)
                server = s
                port = s.listeningPort
                startedAt = System.currentTimeMillis()
                log.info("api server listening on {}", baseUrl)
                return port
            } catch (e: Exception) {
                log.debug("api port {} unavailable: {}", p, e.message)
            }
        }
        log.error("api server 启动失败：{}..{} 与临时端口均不可用", DEFAULT_PORT, MAX_PORT)
        return -1
    }

    fun stop() {
        server?.stop()
        server = null
        port = -1
        startedAt = 0
    }

    private class BadRequest(message: String) : RuntimeException(message)

    private class Handler(bindPort: Int) : NanoHTTPD("127.0.0.1", bindPort) {
        private val log = LoggerFactory.getLogger(Handler::class.java)

        override fun serve(session: IHTTPSession): Response {
            if (session.method == Method.OPTIONS) {
                return cors(newFixedLengthResponse(Response.Status.NO_CONTENT, "text/plain", ""))
            }
            val uri = session.uri ?: "/"
            return try {
                // 必须在路由前统一消费请求体：即使端点不需要 body，也不能把数据留在
                // keep-alive 连接里，否则下一条请求会把残留的“{}”当成请求行。
                // 解析交给本层按 UTF-8 处理：NanoHTTPD 对无 charset 的 JSON 默认按
                // US-ASCII 解码，中文会变成“������”。
                val bodyBytes = if (session.method == Method.POST || session.method == Method.PUT) {
                    readBodyBytes(session)
                } else {
                    ByteArray(0)
                }
                route(session, uri, bodyBytes)
            } catch (e: BadRequest) {
                log.info("api {} {} -> 400 {}", session.method, uri, e.message)
                json(Response.Status.BAD_REQUEST, false, null, e.message, null)
            } catch (e: Exception) {
                log.warn("api {} {} failed: {}", session.method, uri, e.toString())
                json(Response.Status.INTERNAL_ERROR, false, null, errorOf(e), null)
            }
        }

        // ---------- 路由 ----------

        private fun route(session: IHTTPSession, uri: String, bodyBytes: ByteArray): Response {
            val path = uri.substringBefore('?')
            return when {
                path == "/" -> redirect("/swagger")
                path == "/favicon.ico" -> newFixedLengthResponse(Response.Status.NO_CONTENT, "text/plain", "")
                path == "/openapi.json" || path == "/v3/api-docs" ->
                    resource("openapi.json", "application/json; charset=utf-8")

                path == "/swagger" || path == "/swagger/" || path == "/swagger/index.html" ->
                    resource("swagger/index.html", "text/html; charset=utf-8")

                path.startsWith("/swagger-ui/") -> swaggerAsset(path.removePrefix("/swagger-ui/"))

                path == "/api/health" -> {
                    requireMethod(session, Method.GET)
                    json(Response.Status.OK, true, health(), null, null)
                }

                path == "/api/server/info" -> {
                    requireMethod(session, Method.GET)
                    runAndRespond("serverInfo", JsonObject(emptyMap()))
                }

                path == "/api/sites" -> {
                    requireMethod(session, Method.GET)
                    json(Response.Status.OK, true, Host.sitesInfo(), null, null)
                }

                path == "/api/rpc" -> {
                    requireMethod(session, Method.POST)
                    val body = readJsonBody(bodyBytes)
                    val method = body.jsonStr("method")
                    if (method.isBlank()) throw BadRequest("method 不能为空")
                    val params = body["params"] as? JsonObject ?: JsonObject(emptyMap())
                    runAndRespond(method, params)
                }

                path == "/api/config/load" -> {
                    requireMethod(session, Method.POST)
                    val body = readJsonBody(bodyBytes)
                    val data = body.jsonStr("data")
                    if (data.isBlank()) throw BadRequest("data 不能为空")
                    runAndRespond(
                        "loadConfig",
                        buildJsonObject {
                            put("data", data)
                            put("url", body.jsonStr("url"))
                            put("cfgId", body.jsonStr("cfgId"))
                        }
                    )
                }

                path == "/api/proxy/set" -> {
                    requireMethod(session, Method.POST)
                    val body = readJsonBody(bodyBytes)
                    runAndRespond("setProxy", buildJsonObject { put("proxy", body.jsonStr("proxy")) })
                }

                path == "/api/cache/clear" -> {
                    requireMethod(session, Method.POST)
                    runAndRespond("clearCache", JsonObject(emptyMap()))
                }

                path == "/api/shutdown" -> {
                    requireMethod(session, Method.POST)
                    val hook = HttpApiServer.shutdownHook
                        ?: return json(Response.Status.SERVICE_UNAVAILABLE, false, null, "shutdown hook 未注册", null)
                    // 先回包，再延迟退出，避免 TCP 响应被 halt 打断
                    Thread {
                        try {
                            Thread.sleep(250)
                        } catch (_: InterruptedException) {
                        }
                        runCatching { hook() }
                    }.apply { isDaemon = true }.start()
                    json(Response.Status.OK, true, JsonPrimitive("bye"), null, null)
                }

                path.startsWith("/api/sites/") -> siteRoute(session, path.removePrefix("/api/sites/"), bodyBytes)

                path == "/proxy" || path.startsWith("/proxy/") ->
                    json(
                        Response.Status.NOT_FOUND, false, null,
                        "代理端口为 ${ProxyServer.port}，请访问 ${ProxyServer.baseUrl}/proxy", null
                    )

                else -> json(Response.Status.NOT_FOUND, false, null, "not found: $uri", null)
            }
        }

        private fun siteRoute(session: IHTTPSession, rest: String, bodyBytes: ByteArray): Response {
            requireMethod(session, Method.POST)
            val slash = rest.indexOf('/')
            if (slash <= 0 || slash == rest.length - 1) {
                throw BadRequest("路径应为 /api/sites/{siteKey}/{action}")
            }
            val siteKey = rest.substring(0, slash)
            val action = rest.substring(slash + 1)
            val body = readJsonBody(bodyBytes)

            return when (action) {
                "home" -> runAndRespond(
                    "homeContent",
                    buildJsonObject {
                        put("siteKey", siteKey)
                        put("filter", body.jsonBool("filter"))
                    }
                )

                "home-video" -> runAndRespond("homeVideoContent", buildJsonObject { put("siteKey", siteKey) })

                "category" -> {
                    val tid = body.jsonStr("tid")
                    if (tid.isBlank()) throw BadRequest("tid 不能为空")
                    runAndRespond(
                        "categoryContent",
                        buildJsonObject {
                            put("siteKey", siteKey)
                            put("tid", tid)
                            put("pg", body.jsonStr("pg").ifBlank { "1" })
                            put("filter", body.jsonBool("filter"))
                            put("extend", body["extend"] as? JsonObject ?: JsonObject(emptyMap()))
                        }
                    )
                }

                "search" -> {
                    val key = body.jsonStr("key")
                    if (key.isBlank()) throw BadRequest("key 不能为空")
                    runAndRespond(
                        "searchContent",
                        buildJsonObject {
                            put("siteKey", siteKey)
                            put("key", key)
                            put("quick", body.jsonBool("quick"))
                            put("pg", body.jsonStr("pg").ifBlank { "1" })
                        }
                    )
                }

                "detail" -> {
                    val ids = stringArray(body["ids"])
                    if (ids.isEmpty()) throw BadRequest("ids 不能为空")
                    runAndRespond(
                        "detailContent",
                        buildJsonObject {
                            put("siteKey", siteKey)
                            put("ids", ids)
                        }
                    )
                }

                "player" -> {
                    val flag = body.jsonStr("flag")
                    val id = body.jsonStr("id")
                    if (flag.isBlank()) throw BadRequest("flag 不能为空")
                    if (id.isBlank()) throw BadRequest("id 不能为空")
                    runAndRespond(
                        "playerContent",
                        buildJsonObject {
                            put("siteKey", siteKey)
                            put("flag", flag)
                            put("id", id)
                            put("vipFlags", stringArray(body["vipFlags"]))
                        }
                    )
                }

                "recent" -> runAndRespond("setRecent", buildJsonObject { put("siteKey", siteKey) })
                "destroy" -> runAndRespond("destroy", buildJsonObject { put("siteKey", siteKey) })

                "proxy-local" -> runAndRespond(
                    "proxyLocal",
                    buildJsonObject {
                        put("siteKey", siteKey)
                        put("params", body["params"] as? JsonObject ?: JsonObject(emptyMap()))
                    }
                )

                else -> throw BadRequest("未知 action: $action")
            }
        }

        // ---------- 业务调用 ----------

        private fun runAndRespond(method: String, params: JsonObject): Response {
            val t0 = System.currentTimeMillis()
            return try {
                val result = Host.handle(method, params)
                val elapsed = System.currentTimeMillis() - t0
                log.info("api {} ok {}ms params={}", method, elapsed, describe(method, params))
                json(Response.Status.OK, true, result, null, elapsed)
            } catch (e: IllegalArgumentException) {
                val elapsed = System.currentTimeMillis() - t0
                log.info("api {} 400 {}ms: {}", method, elapsed, e.message)
                json(Response.Status.BAD_REQUEST, false, null, errorOf(e), elapsed)
            } catch (e: Exception) {
                val elapsed = System.currentTimeMillis() - t0
                log.warn("api {} 500 {}ms: {}", method, elapsed, e.toString())
                json(Response.Status.INTERNAL_ERROR, false, null, errorOf(e), elapsed)
            }
        }

        private fun health(): JsonElement = buildJsonObject {
            put("status", "up")
            put("version", HOST_VERSION)
            put("apiPort", HttpApiServer.port)
            put("apiBase", HttpApiServer.baseUrl)
            put("proxyPort", ProxyServer.port)
            put("proxyBase", ProxyServer.baseUrl)
            put("swagger", "${HttpApiServer.baseUrl}/swagger")
            put("uptimeMs", HttpApiServer.uptimeMs())
        }

        // ---------- 请求/响应工具 ----------

        private fun requireMethod(session: IHTTPSession, expected: Method) {
            if (session.method != expected) {
                throw BadRequest("需要 ${expected.name} 请求，实际 ${session.method.name}")
            }
        }

        /**
         * 直接按 UTF-8 读取请求体。
         *
         * 不能用 NanoHTTPD 的 `parseBody` 结果：它按 Content-Type 的 charset 解码，
         * 而 `application/json` 通常不带 charset，NanoHTTPD 会退到 US-ASCII，
         * 中文参数（搜索词、站点名、extend 值）全部变成“������”。
         */
        private fun readBodyBytes(session: IHTTPSession): ByteArray {
            val len = session.headers["content-length"]?.trim()?.toLongOrNull() ?: -1L
            if (len < 0) {
                // 无 Content-Length（极少见）：退回 NanoHTTPD，并强制 UTF-8 解释
                val files = HashMap<String, String>()
                val ct = session.headers["content-type"].orEmpty()
                if (!ct.contains("charset", true)) {
                    runCatching { session.headers["content-type"] = (ct.ifBlank { "application/json" }) + "; charset=UTF-8" }
                }
                session.parseBody(files)
                return files["postData"].orEmpty().toByteArray(Charsets.UTF_8)
            }
            if (len == 0L) return ByteArray(0)
            if (len > MAX_BODY_BYTES) {
                throw BadRequest("请求体过大（上限 ${MAX_BODY_BYTES / 1024 / 1024}MB）")
            }
            val bytes = ByteArray(len.toInt())
            var read = 0
            val input = session.inputStream
            while (read < bytes.size) {
                val n = input.read(bytes, read, bytes.size - read)
                if (n <= 0) throw BadRequest("请求体读取不完整（期望 ${bytes.size} 字节，实际 $read）")
                read += n
            }
            return bytes
        }

        private fun readJsonBody(bytes: ByteArray): JsonObject {
            val raw = bytes.toString(Charsets.UTF_8).trim()
            if (raw.isEmpty()) return JsonObject(emptyMap())
            val element = try {
                Jsons.parseToJsonElement(raw)
            } catch (e: Exception) {
                throw BadRequest("请求体不是合法 JSON: ${e.message}")
            }
            return element as? JsonObject ?: throw BadRequest("请求体必须是 JSON 对象")
        }

        private fun stringArray(element: JsonElement?): JsonArray = when (element) {
            null, JsonNull -> JsonArray(emptyList())
            is JsonArray -> buildJsonArray {
                element.forEach { item -> (item as? JsonPrimitive)?.content?.let { add(it) } }
            }

            is JsonPrimitive -> buildJsonArray {
                element.content.split(',').map { it.trim() }.filter { it.isNotEmpty() }.forEach { add(it) }
            }

            else -> JsonArray(emptyList())
        }

        private fun JsonObject.jsonStr(key: String): String =
            (this[key] as? JsonPrimitive)?.content.orEmpty()

        private fun JsonObject.jsonBool(key: String, def: Boolean = false): Boolean =
            (this[key] as? JsonPrimitive)?.content?.toBooleanStrictOrNull() ?: def

        private fun describe(method: String, params: JsonObject): String {
            if (method == "loadConfig") {
                return "url=${params.jsonStr("url")} dataLen=${params.jsonStr("data").length}"
            }
            return params.toString().take(800)
        }

        private fun errorOf(e: Throwable): String = "${e.javaClass.simpleName}: ${e.message}"

        // ---------- HTTP 响应 ----------

        private fun json(
            status: Response.IStatus,
            ok: Boolean,
            result: JsonElement?,
            error: String?,
            elapsedMs: Long?,
        ): Response {
            val payload = buildJsonObject {
                put("ok", ok)
                put("result", result ?: JsonNull)
                put("error", error?.let { JsonPrimitive(it) } ?: JsonNull)
                if (elapsedMs != null) put("elapsedMs", elapsedMs)
            }.toString().toByteArray(Charsets.UTF_8)
            return cors(
                newFixedLengthResponse(
                    status,
                    "application/json; charset=utf-8",
                    ByteArrayInputStream(payload),
                    payload.size.toLong(),
                )
            )
        }

        private fun redirect(location: String): Response {
            val resp = newFixedLengthResponse(Response.Status.REDIRECT, "text/plain", "")
            resp.addHeader("Location", location)
            return cors(resp)
        }

        private fun swaggerAsset(name: String): Response {
            if (name.isEmpty() || name.contains('/') || name.contains('\\') || name.contains("..")) {
                return json(Response.Status.NOT_FOUND, false, null, "invalid asset: $name", null)
            }
            val resource = "META-INF/resources/webjars/swagger-ui/$SWAGGER_UI_VERSION/$name"
            return resource(resource, mimeOf(name), cacheKey = "swagger-ui/$name")
        }

        private fun resource(resourcePath: String, mime: String, cacheKey: String = resourcePath): Response {
            val bytes = apiAssetCache[cacheKey] ?: run {
                val loaded = loadResource(resourcePath)
                    ?: return json(Response.Status.NOT_FOUND, false, null, "missing resource: $resourcePath", null)
                apiAssetCache.putIfAbsent(cacheKey, loaded)
                apiAssetCache[cacheKey] ?: loaded
            }
            val resp = newFixedLengthResponse(
                Response.Status.OK, mime, ByteArrayInputStream(bytes), bytes.size.toLong()
            )
            resp.addHeader("Cache-Control", "no-cache")
            return cors(resp)
        }

        private fun loadResource(path: String): ByteArray? =
            HttpApiServer::class.java.classLoader.getResourceAsStream(path)?.use { it.readBytes() }

        private fun mimeOf(name: String): String = when (name.substringAfterLast('.', "").lowercase()) {
            "html" -> "text/html; charset=utf-8"
            "css" -> "text/css; charset=utf-8"
            "js" -> "application/javascript; charset=utf-8"
            "json" -> "application/json; charset=utf-8"
            "png" -> "image/png"
            "svg" -> "image/svg+xml"
            "ico" -> "image/x-icon"
            else -> "application/octet-stream"
        }

        private fun cors(resp: Response): Response {
            resp.addHeader("Access-Control-Allow-Origin", "*")
            resp.addHeader("Access-Control-Allow-Headers", "*")
            resp.addHeader("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
            return resp
        }
    }
}

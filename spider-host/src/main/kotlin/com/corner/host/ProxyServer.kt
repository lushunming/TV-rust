package com.corner.host

import fi.iki.elonen.NanoHTTPD
import org.slf4j.LoggerFactory
import java.io.ByteArrayInputStream
import java.io.InputStream

/**
 * 宿主内置的本地 HTTP 代理服务（对应 composeApp 的 KtorD + /proxy 路由）。
 *
 * 为什么必须有它：
 * 大量 catvod jar 爬虫把图片、字幕、m3u8 切片地址写成 http://127.0.0.1:9978/proxy?do=xxx，
 * 由宿主侧回调爬虫的 proxy 方法并流式转发。缺少这个服务时，列表能出但海报与播放地址全部 404。
 *
 * 只实现 /proxy 这一个端点；端口默认 9978（爬虫里硬编码的端口），被占用时顺延。
 */
object ProxyServer {
    private val log = LoggerFactory.getLogger(ProxyServer::class.java)

    const val DEFAULT_PORT = 9978
    private const val MAX_PORT = 9999

    @Volatile
    private var server: NanoHTTPD? = null

    /** 实际监听端口，-1 表示未启动 */
    @Volatile
    var port: Int = -1
        private set

    val baseUrl: String
        get() = if (port > 0) "http://127.0.0.1:$port" else ""

    fun start(): Int {
        if (port > 0) return port
        var p = DEFAULT_PORT
        while (p <= MAX_PORT) {
            try {
                val s = Handler(p)
                // daemon 线程，跟随宿主进程退出
                s.start(NanoHTTPD.SOCKET_READ_TIMEOUT, true)
                server = s
                port = p
                log.info("proxy server listening on {}", baseUrl)
                return p
            } catch (e: Exception) {
                log.debug("proxy port {} unavailable: {}", p, e.message)
                p++
            }
        }
        log.error("proxy server 启动失败：{}-{} 端口均不可用，依赖本地代理的站点将不可用", DEFAULT_PORT, MAX_PORT)
        return -1
    }

    fun stop() {
        server?.stop()
        server = null
        port = -1
    }

    private class Handler(port: Int) : NanoHTTPD(port) {
        override fun serve(session: IHTTPSession): Response {
            val uri = session.uri ?: "/"
            if (uri != "/proxy" && !uri.startsWith("/proxy/")) {
                return newFixedLengthResponse(Response.Status.NOT_FOUND, "text/plain", "not found")
            }
            if (session.method == Method.OPTIONS) {
                return newFixedLengthResponse(Response.Status.NO_CONTENT, "text/plain", "")
                    .apply { addCors() }
            }
            return try {
                proxy(session)
            } catch (e: Exception) {
                log.warn("proxy 转发失败 uri={} err={}", uri, e.toString())
                errorResponse()
            }
        }

        /** 参数合并顺序与 Ktor 版一致：query 在前，header 覆盖同名项 */
        private fun collectParams(session: IHTTPSession): MutableMap<String, String> {
            val params = LinkedHashMap<String, String>()
            if (session.method == Method.POST || session.method == Method.PUT) {
                try {
                    session.parseBody(HashMap<String, String>())
                } catch (_: Exception) {
                    // body 非表单也继续，交给爬虫按 query/header 处理
                }
            }
            session.parameters.forEach { (k, v) -> if (v.isNotEmpty()) params[k] = v[0] }
            session.headers.forEach { (k, v) -> params[k] = v }
            return params
        }

        private fun proxy(session: IHTTPSession): Response {
            val params = collectParams(session)
            val result = Host.proxyHandle(params)
            if (result == null || result.isEmpty()) {
                log.debug("proxy 无可用处理者 do={}", params["do"])
                return errorResponse()
            }

            val first = result[0]
            // 形态一：爬虫直接返回 okhttp Response（保持状态码/头部/流）
            if (first is okhttp3.Response) {
                return fromOkHttp(first)
            }

            val code = (first as? Number)?.toInt() ?: 500
            // 形态二：[302, contentType, url]
            if (code == 302 || code == 301 || code == 307 || code == 308) {
                val location = result.getOrNull(2)?.toString().orEmpty()
                val resp = newFixedLengthResponse(statusOf(code), "text/plain", "")
                resp.addHeader("Location", location)
                resp.addCors()
                return resp
            }

            // 形态三：[code, contentType, InputStream, headers?]
            val mime = result.getOrNull(1)?.toString()?.takeIf { it.isNotBlank() } ?: "application/octet-stream"
            val body = result.getOrNull(2)
            val stream: InputStream = when (body) {
                null -> ByteArrayInputStream(ByteArray(0))
                is InputStream -> body
                is ByteArray -> ByteArrayInputStream(body)
                is String -> ByteArrayInputStream(body.toByteArray())
                else -> ByteArrayInputStream(body.toString().toByteArray())
            }
            val extra = result.getOrNull(3) as? Map<*, *>
            val length = headerValue(extra, "content-length")?.toLongOrNull() ?: -1L

            val resp = if (length >= 0) {
                newFixedLengthResponse(statusOf(code), mime, stream, length)
            } else {
                newChunkedResponse(statusOf(code), mime, stream)
            }
            extra?.forEach { (k, v) -> appendHeader(resp, k?.toString().orEmpty(), v?.toString().orEmpty()) }
            resp.addCors()
            return resp
        }

        private fun fromOkHttp(src: okhttp3.Response): Response {
            val body = src.body
            val mime = body?.contentType()?.toString() ?: "application/octet-stream"
            val length = body?.contentLength() ?: -1L
            val stream: InputStream = body?.byteStream() ?: ByteArrayInputStream(ByteArray(0))
            val resp = if (length >= 0) {
                newFixedLengthResponse(statusOf(src.code), mime, stream, length)
            } else {
                newChunkedResponse(statusOf(src.code), mime, stream)
            }
            src.headers.forEach { (name, value) -> appendHeader(resp, name, value) }
            resp.addCors()
            return resp
        }

        private fun headerValue(map: Map<*, *>?, name: String): String? {
            if (map == null) return null
            for ((k, v) in map) {
                if (k?.toString()?.equals(name, ignoreCase = true) == true) return v?.toString()
            }
            return null
        }

        private fun appendHeader(resp: Response, name: String, value: String) {
            if (name.isBlank() || value.isBlank()) return
            // 长度/连接类头部由 NanoHTTPD 自行管理，透传会破坏分块或长度语义
            if (name.equals("content-length", true) ||
                name.equals("transfer-encoding", true) ||
                name.equals("connection", true)
            ) return
            resp.addHeader(name, value)
        }

        private fun Response.addCors() {
            addHeader("Access-Control-Allow-Origin", "*")
            addHeader("Access-Control-Allow-Headers", "*")
            addHeader("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        }

        private fun errorResponse(): Response =
            newFixedLengthResponse(Response.Status.INTERNAL_ERROR, "text/plain", "proxy error")

        private fun statusOf(code: Int): Response.IStatus =
            Response.Status.lookup(code) ?: object : Response.IStatus {
                override fun getRequestStatus(): Int = code
                override fun getDescription(): String = "Status $code"
            }
    }
}

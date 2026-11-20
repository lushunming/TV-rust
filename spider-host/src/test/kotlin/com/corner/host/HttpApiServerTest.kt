package com.corner.host

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put
import org.junit.jupiter.api.AfterAll
import org.junit.jupiter.api.Assertions.assertEquals
import org.junit.jupiter.api.Assertions.assertNotNull
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.BeforeAll
import org.junit.jupiter.api.BeforeEach
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.TestInstance
import java.net.URI
import java.net.http.HttpClient
import java.net.http.HttpRequest
import java.net.http.HttpResponse
import java.time.Duration

/**
 * HTTP API 冒烟测试：不依赖真实爬虫 jar，只验证传输层、路由、Swagger 资源与错误映射。
 */
@TestInstance(TestInstance.Lifecycle.PER_CLASS)
class HttpApiServerTest {
    private var port: Int = 0
    private lateinit var client: HttpClient

    @BeforeAll
    fun setUp() {
        port = HttpApiServer.start(0)
        assertTrue(port > 0, "HTTP API 应启动成功")
        client = HttpClient.newBuilder().connectTimeout(Duration.ofSeconds(5)).build()
    }

    @AfterAll
    fun tearDown() {
        HttpApiServer.stop()
    }

    @BeforeEach
    fun resetHost() {
        // 每个用例从干净状态开始，避免 loadConfig 互相影响
        post("/api/cache/clear", "{}")
    }

    @Test
    fun `health returns running host info`() {
        val resp = get("/api/health")
        assertEquals(200, resp.statusCode())
        val body = Json.parseToJsonElement(resp.body()).jsonObject
        assertTrue(body["ok"]!!.jsonPrimitive.content.toBoolean())
        val result = body["result"]!!.jsonObject
        assertEquals("up", result["status"]!!.jsonPrimitive.content)
        assertEquals(port.toString(), result["apiPort"]!!.jsonPrimitive.content)
    }

    @Test
    fun `openapi document is served and contains core paths`() {
        val resp = get("/openapi.json")
        assertEquals(200, resp.statusCode())
        val doc = Json.parseToJsonElement(resp.body()).jsonObject
        assertEquals("3.0.3", doc["openapi"]!!.jsonPrimitive.content)
        val paths = doc["paths"]!!.jsonObject
        assertNotNull(paths["/api/rpc"])
        assertNotNull(paths["/api/config/load"])
        assertNotNull(paths["/api/sites/{siteKey}/search"])
    }

    @Test
    fun `swagger ui page and its bundled assets are served`() {
        val page = get("/swagger")
        assertEquals(200, page.statusCode())
        assertTrue(page.body().contains("SwaggerUIBundle"), "Swagger 页面应包含初始化脚本")

        val css = get("/swagger-ui/swagger-ui.css")
        assertEquals(200, css.statusCode())
        assertTrue(css.body().length > 1000, "swagger-ui.css 应来自 webjar 且非空")

        val js = get("/swagger-ui/swagger-ui-bundle.js")
        assertEquals(200, js.statusCode())
        assertTrue(js.body().length > 100_000, "swagger-ui-bundle.js 应来自 webjar")
    }

    @Test
    fun `rpc endpoint maps unknown method to 400`() {
        val resp = post("/api/rpc", """{"method":"noSuchMethod","params":{}}""")
        assertEquals(400, resp.statusCode())
        val body = Json.parseToJsonElement(resp.body()).jsonObject
        assertTrue(!body["ok"]!!.jsonPrimitive.content.toBoolean())
        assertTrue(body["error"]!!.jsonPrimitive.content.contains("unknown method"))
    }

    @Test
    fun `loadConfig then sites endpoint returns empty list`() {
        val load = post("/api/config/load", """{"data":"{\"sites\":[]}","cfgId":"test"}""")
        assertEquals(200, load.statusCode())
        val loaded = Json.parseToJsonElement(load.body()).jsonObject
        assertTrue(loaded["ok"]!!.jsonPrimitive.content.toBoolean())

        val sites = get("/api/sites")
        assertEquals(200, sites.statusCode())
        val result = Json.parseToJsonElement(sites.body()).jsonObject["result"]
        assertNotNull(result)
        assertEquals("[]", result.toString())
    }

    @Test
    fun `large config body over nanohttpd memory limit is parsed`() {
        val config = buildJsonObject {
            put("sites", buildJsonArray {
                (1..200).forEach { i ->
                    add(buildJsonObject {
                        put("key", "s$i")
                        put("name", "站点$i")
                        put("type", 3)
                        put("api", "csp_Demo")
                    })
                }
            })
        }.toString()
        val payload = buildJsonObject {
            put("data", config)
            put("cfgId", "配置-large")
        }.toString()
        assertTrue(payload.toByteArray().size > 1024, "请求体应超过 NanoHTTPD 内存阈值，覆盖临时文件路径")

        val resp = post("/api/config/load", payload)
        assertEquals(200, resp.statusCode())
        val body = Json.parseToJsonElement(resp.body()).jsonObject
        assertTrue(body["ok"]!!.jsonPrimitive.content.toBoolean())
        assertTrue(body["result"]!!.jsonPrimitive.content.contains("s200"))

        val sites = Json.parseToJsonElement(get("/api/sites").body()).jsonObject["result"]!!.jsonArray
        assertEquals(200, sites.size)
        val last = sites.first { it.jsonObject["key"]!!.jsonPrimitive.content == "s200" }.jsonObject
        assertEquals("站点200", last["name"]!!.jsonPrimitive.content, "UTF-8 中文站点名不应乱码")

        val info = Json.parseToJsonElement(get("/api/server/info").body()).jsonObject["result"]!!.jsonObject
        assertEquals("配置-large", info["cfgId"]!!.jsonPrimitive.content, "UTF-8 中文 cfgId 不应乱码")
    }

    @Test
    fun `semantic site endpoint reports missing site as 400`() {
        val resp = post("/api/sites/qq/home", """{"filter":true}""")
        assertEquals(400, resp.statusCode())
        assertTrue(resp.body().contains("site not found"))
    }

    private fun base(): String = "http://127.0.0.1:$port"

    private fun get(path: String): HttpResponse<String> =
        client.send(
            HttpRequest.newBuilder(URI.create(base() + path)).GET().build(),
            HttpResponse.BodyHandlers.ofString()
        )

    private fun post(path: String, body: String): HttpResponse<String> =
        client.send(
            HttpRequest.newBuilder(URI.create(base() + path))
                .header("Content-Type", "application/json")
                .POST(HttpRequest.BodyPublishers.ofString(body))
                .build(),
            HttpResponse.BodyHandlers.ofString()
        )
}

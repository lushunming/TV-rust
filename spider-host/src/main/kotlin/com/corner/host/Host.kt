package com.corner.host

import com.corner.catvodcore.util.Http
import com.corner.catvodcore.util.Jsons
import com.corner.catvodcore.util.Urls
import com.corner.catvod.enum.bean.Site
import com.github.catvod.crawler.Spider
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.*
import org.apache.commons.lang3.StringUtils
import org.slf4j.LoggerFactory
import java.util.concurrent.ConcurrentHashMap

// 配置模型：仅保留宿主运行所需字段，其余字段由 ignoreUnknownKeys 忽略
@Serializable
data class HostApi(
    val spider: String = "",
    var sites: MutableList<Site> = mutableListOf()
)

// 宿主状态 + 方法分发。所有 Spider 方法调用都通过 handle() 进入。
object Host {
    private val log = LoggerFactory.getLogger(Host::class.java)

    private val sites = ConcurrentHashMap<String, Site>()

    @Volatile
    private var globalSpider: String = ""

    @Volatile
    private var baseUrl: String = ""

    @Volatile
    private var cfgId: String = ""

    /** 最近一次 setRecent 的站点 key：/proxy 回退到实例方法 proxyLocal 时使用 */
    @Volatile
    private var recentSiteKey: String = ""

    /**
     * 解析配置 JSON（与 ApiConfig.parseConfig 对应的宿主侧实现）。
     * @param data 配置原始 JSON 文本
     * @param url 配置源 URL（用于解析相对路径，可为空）
     * @return 归一化后的配置 JSON 字符串
     */
    fun loadConfig(data: String, url: String?, cfgId: String?): String {
        baseUrl = url ?: ""
        this.cfgId = cfgId ?: ""
        val api = Jsons.decodeFromString<HostApi>(data)
        globalSpider = api.spider

        sites.clear()
        JarLoader.baseUrl = baseUrl
        JarLoader.clear()
        for (site in api.sites) {
            site.api = parseApi(site.api)
            site.ext = parseExt(site.ext)
            // FongMi 惯例：站点未单独指定 jar 时回退到全局 spider
            if (site.jar.isNullOrBlank() && globalSpider.isNotBlank()) site.jar = globalSpider
            sites[site.key] = site
        }

        val normalized = Jsons.encodeToString(HostApi(globalSpider, api.sites))
        log.info("loadConfig ok, sites={} url={}", api.sites.size, baseUrl)
        return normalized
    }

    fun parseExt(ext: String): String {
        if (StringUtils.isBlank(ext)) return ext
        if (ext.startsWith("file")) {
            return java.nio.file.Files.readString(java.nio.file.Paths.get(Urls.convert(ext)))
        }
        if (ext.endsWith(".js") || ext.endsWith(".json") || ext.endsWith(".txt")) {
            return Urls.convert(baseUrl, ext)
        }
        return ext
    }

    fun parseApi(str: String): String {
        if (StringUtils.isBlank(str)) return ""
        if (str.startsWith("http") || str.startsWith("file")) return str
        if (str.endsWith(".js")) return parseApi(Urls.convert(baseUrl, str))
        return str
    }

    fun getSite(key: String): Site? = sites[key]

    fun siteCount(): Int = sites.size

    fun configId(): String = cfgId

    /**
     * /proxy 端点的实际转发入口。
     * 优先走 jar 的静态 proxy(Map) 方法（FongMi 标准），失败时回退到最近站点的 proxyLocal 实例方法，
     * 兼容只实现其中一种的爬虫。
     */
    fun proxyHandle(params: Map<String, String?>): Array<Any>? {
        val flat = HashMap<String, String>()
        params.forEach { (k, v) -> if (k != null && v != null) flat[k] = v }
        runCatching { JarLoader.proxyInvoke(flat) }.getOrNull()?.let { if (it.isNotEmpty()) return it }
        val site = sites[recentSiteKey] ?: return null
        val map = HashMap<String?, String?>(params)
        return runCatching { getSpider(site).proxyLocal(map) }.getOrNull()
    }

    private fun getSpider(site: Site): Spider {
        val csp: Boolean = site.api.startsWith("csp_")
        return if (csp) JarLoader.getSpider(site.key, site.api, site.ext, site.jar ?: "") else Spider()
    }

    /** Spider 返回的 JSON 字符串；若该站点 init 曾失败，附带 warning 字段供前端提示 */
    private fun spiderJson(site: Site, json: String): JsonElement {
        val warn = JarLoader.initWarning(site.key)
        return if (warn == null) {
            JsonPrimitive(json)
        } else {
            buildJsonObject {
                put("data", JsonPrimitive(json))
                put("warning", JsonPrimitive(warn))
            }
        }
    }

    private fun requireSite(siteKey: String): Site {
        return sites[siteKey] ?: throw IllegalArgumentException("site not found: $siteKey")
    }

    private fun JsonObject.str(key: String): String {
        val v = this[key] as? JsonPrimitive ?: return ""
        return if (v.isString) v.content else v.content
    }

    private fun JsonObject.bool(key: String, def: Boolean = false): Boolean {
        val v = this[key] as? JsonPrimitive ?: return def
        return v.content.toBooleanStrictOrNull() ?: def
    }

    /**
     * 方法分发。返回值将作为响应中的 result 字段（可为 null）。
     * Spider 系列方法返回爬虫原始输出字符串（本身就是 JSON）。
     */
    fun handle(method: String, params: JsonObject): JsonElement? {
        return when (method) {
            "loadConfig" -> {
                val normalized = loadConfig(params.str("data"), params.str("url").ifBlank { null }, params.str("cfgId").ifBlank { null })
                JsonPrimitive(normalized)
            }

            "homeContent" -> {
                val site = requireSite(params.str("siteKey"))
                spiderJson(site, getSpider(site).homeContent(params.bool("filter")))            }

            "homeVideoContent" -> {
                val site = requireSite(params.str("siteKey"))
                spiderJson(site, getSpider(site).homeVideoContent())
            }

            "categoryContent" -> {
                val site = requireSite(params.str("siteKey"))
                val extend = HashMap<String, String>()
                (params["extend"] as? JsonObject)?.forEach { (k, v) ->
                    extend[k] = (v as? JsonPrimitive)?.content ?: ""
                }
                spiderJson(
                    site,
                    getSpider(site).categoryContent(params.str("tid"), params.str("pg"), params.bool("filter"), extend)
                )
            }

            "detailContent" -> {
                val site = requireSite(params.str("siteKey"))
                val ids = (params["ids"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.content } ?: emptyList()
                spiderJson(site, getSpider(site).detailContent(ids))
            }

            "searchContent" -> {
                val site = requireSite(params.str("siteKey"))
                val pg = params.str("pg").ifBlank { "1" }
                spiderJson(site, getSpider(site).searchContent(params.str("key"), params.bool("quick"), pg))
            }

            "playerContent" -> {
                val site = requireSite(params.str("siteKey"))
                val vipFlags = (params["vipFlags"] as? JsonArray)?.mapNotNull { (it as? JsonPrimitive)?.content } ?: emptyList()
                JsonPrimitive(getSpider(site).playerContent(params.str("flag"), params.str("id"), vipFlags))
            }

            "setRecent" -> {
                val site = requireSite(params.str("siteKey"))
                recentSiteKey = site.key
                JarLoader.SetRecent(site.jar)
                null
            }

            "serverInfo" -> buildJsonObject {
                put("proxyPort", ProxyServer.port)
                put("proxyBase", ProxyServer.baseUrl)
                put("sites", sites.size)
                put("cfgId", cfgId)
            }

            "setProxy" -> {
                // 出站代理（对应 ApiConfig.initProxy / rules.proxy），空串表示清除
                Http.setProxy(params.str("proxy"))
                null
            }

            "proxyLocal" -> {
                val site = requireSite(params.str("siteKey"))
                val map = HashMap<String?, String?>()
                (params["params"] as? JsonObject)?.forEach { (k, v) ->
                    map[k] = (v as? JsonPrimitive)?.content
                }
                val result = getSpider(site).proxyLocal(map)
                result?.let { arr ->
                    buildJsonArray {
                        for (item in arr) {
                            when (item) {
                                is String -> add(JsonPrimitive(item))
                                is Number -> add(JsonPrimitive(item))
                                is Boolean -> add(JsonPrimitive(item))
                                is ByteArray -> add(JsonPrimitive(java.util.Base64.getEncoder().encodeToString(item)))
                                else -> add(JsonPrimitive(item?.toString()))
                            }
                        }
                    }
                }
            }

            "destroy" -> {
                val site = requireSite(params.str("siteKey"))
                val spider = JarLoader.getSpider(site.key, site.api, site.ext, site.jar ?: "")
                spider.destroy()
                null
            }

            "clearCache" -> {
                JarLoader.clear()
                sites.clear()
                recentSiteKey = ""
                null
            }

            else -> throw IllegalArgumentException("unknown method: $method")
        }
    }
}

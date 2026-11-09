package com.corner.update

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.net.HttpURLConnection
import java.net.URL

/**
 * 应用自更新相关配置。
 *
 * 发布物托管在 GitHub Releases；资产下载前先探测加速前缀，选用第一个可用的。
 * Releases API 仍直连 api.github.com。
 */
object UpdateConfig {
    private const val GITHUB_HOST = "github.com"
    private const val REPO = "lushunming/TV-Multiplatform"

    /**
     * 探测用公开小文件（稳定、体积小）。
     * 也可用你们自己 release 里的某个小资产 URL。
     */
    private const val PROBE_TARGET =
        "https://raw.githubusercontent.com/github/gitignore/main/README.md"

    /** 连接/读超时（毫秒） */
    private const val PROBE_TIMEOUT_MS = 5_000

    val CDN_PREFIXES: List<String> = listOf(
        "https://gh-proxy.com/",
        "https://ghfast.top/",
        "https://mirror.ghproxy.com/",
        "https://ghproxy.net/",
        "https://gh-proxy.org/",
        "https://gh.llkk.cc/",
        "https://hub.gitmirror.com/",
        "https://ghps.cc/",
        "https://gitdl.cn/",
        "https://gh.ddlc.top/",
        "https://gh.con.sh/",
        "https://github.moeyy.xyz/",
        "https://fastgit.cc/",
        "https://g.blfrp.cn/",
        "https://ui.ghproxy.cc/",
        "https://gh.jasonzeng.dev/",
        "https://gh.sageer.me/",
        "https://cf.ghproxy.cc/",
        "https://slink.ltd/",
        "https://v4.gh-proxy.org/",
    )

    const val CURRENT_VERSION = "1.2.6"

    val owner: String = REPO.substringBefore("/")
    val repo: String = REPO.substringAfter("/")

    @Volatile
    private var cachedPrefix: String? = null

    /**
     * 顺序探测 [CDN_PREFIXES]，返回第一个 HTTP 2xx 的前缀；都失败返回 null。
     * 成功结果会缓存，避免每次下载都全量探测。
     */
    suspend fun selectWorkingPrefix(forceRefresh: Boolean = false): String? =
        withContext(Dispatchers.IO) {
            if (!forceRefresh) {
                cachedPrefix?.let { return@withContext it }
            }
            for (prefix in CDN_PREFIXES) {
                if (probePrefix(prefix)) {
                    cachedPrefix = prefix
                    return@withContext prefix
                }
            }
            cachedPrefix = null
            null
        }

    /** 清除缓存，下次 [selectWorkingPrefix] 会重新探测。 */
    fun invalidatePrefixCache() {
        cachedPrefix = null
    }

    /**
     * 使用当前缓存前缀（或先探测）把 github.com 下载链接转为加速链接。
     * 无可用前缀时返回原始 [url]。
     */
    suspend fun toCdnUrl(url: String, forceRefresh: Boolean = false): String {
        if (url.isBlank()) return url
        if (CDN_PREFIXES.any { url.startsWith(it) }) return url
        if (!url.contains("://$GITHUB_HOST/") && !url.startsWith("$GITHUB_HOST/")) return url

        val prefix = selectWorkingPrefix(forceRefresh) ?: return url
        return prefix + url
    }

    private fun probePrefix(prefix: String): Boolean {
        val probeUrl = prefix + PROBE_TARGET
        return try {
            val conn = (URL(probeUrl).openConnection() as HttpURLConnection).apply {
                instanceFollowRedirects = true
                connectTimeout = PROBE_TIMEOUT_MS
                readTimeout = PROBE_TIMEOUT_MS
                requestMethod = "GET"
                setRequestProperty("User-Agent", "TV-Multiplatform-UpdateProbe")
            }
            try {
                val code = conn.responseCode
                code in 200..299
            } finally {
                conn.disconnect()
            }
        } catch (_: Exception) {
            false
        }
    }
}
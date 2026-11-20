package com.corner.host

import com.google.gson.JsonElement
import com.google.gson.JsonObject
import com.google.gson.JsonParser
import org.slf4j.LoggerFactory
import java.io.File
import java.nio.file.Files
import java.nio.file.StandardCopyOption

/**
 * UC 网盘 token 缓存自愈。
 *
 * 背景：爬虫 jar 的 `UCTokenHandler` 在刷新 token 失败时，会把错误响应原样写进
 * `~/TV/.uctoken`，例如：
 * ```json
 * {"user":{"cookie":"{\"status\":-1,\"errno\":11005,\"refresh_token\":\"\"}"}}
 * ```
 * 下次 `UCApi` 构造时会把这个字符串当成 token JSON，取 `refresh_token` 得到 null，
 * 抛 NPE -> `UCApi$Loader` 类初始化失败 -> 同一 JVM 内所有 UC 站点都变成
 * `NoClassDefFoundError: Could not initialize class com.github.catvod.api.UCApi$Loader`。
 *
 * 这里在首次加载爬虫前校验一次：只接受带非空 access_token/refresh_token 的正常响应；
 * 错误响应或无法解析的内容会被重命名为 `.uctoken.bad-<时间戳>` 隔离。
 * 这样 `UCApi` 会走“无 token”分支，仍可使用 `~/TV/.uc` 里的登录 cookie。
 */
object UcTokenCacheGuard {
    private val log = LoggerFactory.getLogger(UcTokenCacheGuard::class.java)

    @Volatile
    private var checked = false

    fun ensureUsable() {
        if (checked) return
        synchronized(this) {
            if (checked) return
            checked = true
            runCatching { checkFile(defaultFile()) }
                .onFailure { log.warn("UC token 缓存检查失败: {}", it.toString()) }
        }
    }

    fun defaultFile(): File = File(File(System.getProperty("user.home"), "TV"), ".uctoken")

    /**
     * @return true 表示缓存可用（或不存在）；false 表示内容无效（已尝试隔离）
     */
    internal fun checkFile(file: File): Boolean {
        if (!file.isFile) return true
        val text = runCatching { file.readText(Charsets.UTF_8) }.getOrNull()?.trim().orEmpty()
        if (text.isEmpty()) return true

        val root = parseObject(text) ?: return quarantine(file, "内容不是 JSON 对象")
        val cookie = root.get("user")?.asObjectOrNull()?.get("cookie")?.asStringOrNull().orEmpty()
        if (cookie.isBlank()) return true // 没有 token，UCApi 不会走 refreshToken

        val token = parseObject(cookie) ?: return quarantine(file, "cookie 不是 JSON 对象")
        val status = token.get("status")?.asIntOrNull()
        if (status != null && status < 0) {
            return quarantine(file, "token 响应 status=$status")
        }
        val access = token.get("access_token")?.asStringOrNull().orEmpty()
        val refresh = token.get("refresh_token")?.asStringOrNull().orEmpty()
        if (access.isBlank() && refresh.isBlank()) {
            return quarantine(file, "access_token/refresh_token 均为空")
        }
        return true
    }

    private fun parseObject(text: String): JsonObject? =
        runCatching { JsonParser.parseString(text) }
            .getOrNull()
            ?.takeIf { it.isJsonObject() }
            ?.asJsonObject

    private fun JsonElement.asObjectOrNull(): JsonObject? =
        if (isJsonObject()) asJsonObject else null

    private fun JsonElement.asStringOrNull(): String? =
        if (isJsonPrimitive() && asJsonPrimitive.isString) asString else null

    private fun JsonElement.asIntOrNull(): Int? =
        if (isJsonPrimitive() && asJsonPrimitive.isNumber) asInt else null

    private fun quarantine(file: File, reason: String): Boolean {
        val target = File(file.parentFile, file.name + ".bad-" + System.currentTimeMillis())
        return runCatching {
            Files.move(file.toPath(), target.toPath(), StandardCopyOption.REPLACE_EXISTING)
            log.warn("UC token 缓存无效（{}），已隔离为 {}；UC 站点将重新获取 token", reason, target.name)
            false
        }.getOrElse {
            log.warn("隔离 UC token 缓存失败 {}: {}", file.absolutePath, it.toString())
            false
        }
    }
}

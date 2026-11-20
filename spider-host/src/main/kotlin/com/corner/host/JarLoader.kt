package com.corner.host

import com.corner.catvodcore.Constant
import com.corner.catvodcore.util.Http
import com.corner.catvodcore.util.Paths
import com.corner.catvodcore.util.Urls
import com.corner.catvodcore.util.Utils
import com.github.catvod.crawler.Spider
import org.apache.commons.lang3.StringUtils
import org.slf4j.LoggerFactory
import java.io.File
import java.lang.invoke.MethodHandle
import java.lang.invoke.MethodHandles
import java.lang.invoke.MethodType
import java.lang.reflect.Constructor
import java.net.URLClassLoader
import java.util.concurrent.ConcurrentHashMap

// 与 composeApp 的 JarLoader 逻辑保持一致的宿主侧实现
object JarLoader {
    private val log = LoggerFactory.getLogger(this::class.java)

    private val loaders: ConcurrentHashMap<String, URLClassLoader> by lazy { ConcurrentHashMap() }
    private val proxyHandles: ConcurrentHashMap<String, MethodHandle> by lazy { ConcurrentHashMap() }
    private val initHandles: ConcurrentHashMap<String, MethodHandle> by lazy { ConcurrentHashMap() }
    private val spiderConstructors: ConcurrentHashMap<String, Constructor<out Spider>> by lazy { ConcurrentHashMap() }
    private val spiders: ConcurrentHashMap<String, Spider> by lazy { ConcurrentHashMap() }
    /// 站点 init 失败的告警（不致命，但要透出给前端）
    private val initWarnings: ConcurrentHashMap<String, String> by lazy { ConcurrentHashMap() }

    var recent: String? = null

    // 相对 jar / ext 路径的解析基准（配置源 URL）
    @Volatile
    var baseUrl: String = ""

    fun clear() {
        loaders.clear()
        proxyHandles.clear()
        initHandles.clear()
        spiderConstructors.clear()
        spiders.clear()
        initWarnings.clear()
        recent = null
    }

    /// 该站点上次 init 失败的原因（无则 null）
    fun initWarning(key: String): String? = initWarnings[key]

    fun loadJar(key: String, spider: String) {
        if (StringUtils.isBlank(spider)) return
        val texts = spider.split(Constant.md5Split)
        val expectMd5 = if (texts.size <= 1) "" else texts[1].trim()
        var jar = texts[0].trim()

        // 必须先拆分 ";md5;" 再解析相对路径，否则 ";md5;hash" 会被当成 URL 路径的一部分
        if (!jar.startsWith("http") && !jar.startsWith("file")) {
            val abs = Urls.convert(baseUrl, jar)
            if (abs.isBlank() || abs == jar) {
                log.warn("无法解析jar相对路径: {} baseUrl: {}", jar, baseUrl)
                return
            }
            jar = abs
        }

        if (jar.startsWith("file")) {
            load(key, Paths.local(jar))
            return
        }

        // http(s)：缓存内容 md5 与配置一致则直接离线加载
        val cached = Paths.jar(jar)
        if (expectMd5.isNotEmpty() && Utils.equals(jar, expectMd5) && isZip(cached)) {
            log.debug("jar 缓存命中 {} md5={}", jar, expectMd5)
            load(key, cached)
            return
        }

        try {
            load(key, download(jar, expectMd5))
        } catch (e: Exception) {
            // 下载失败（断网/JVM TLS 被拒）时回退已有缓存，避免整站无数据
            if (isZip(cached)) {
                log.warn("jar 下载失败，回退本地缓存 {}: {}", jar, e.toString())
                load(key, cached)
            } else {
                log.error("jar 下载失败且无可用缓存 {}（依赖该 jar 的站点将无数据）: {}", jar, e.toString())
                throw e
            }
        }
    }

    /** 校验文件是否为 zip/jar（前两字节 PK） */
    private fun isZip(file: File): Boolean {
        if (!file.isFile || file.length() < 4) return false
        return try {
            file.inputStream().use { s ->
                val b = ByteArray(2)
                s.read(b) == 2 && b[0] == 0x50.toByte() && b[1] == 0x4B.toByte()
            }
        } catch (e: Exception) {
            false
        }
    }

    private fun load(key: String, jar: File) {
        log.debug("load jar {}", jar)
        if (loaders.containsKey(key)) return

        val loader = URLClassLoader(arrayOf(jar.toURI().toURL()), this.javaClass.classLoader)
        loaders[key] = loader

        putProxy(key, loader)
        invokeInit(key, loader)
    }

    private fun putProxy(key: String, loader: URLClassLoader) {
        try {
            val clazz = loader.loadClass(Constant.catVodProxy)
            val lookup = MethodHandles.lookup()
            val handle = lookup.findStatic(
                clazz,
                "proxy",
                MethodType.methodType(Array<Any>::class.java, Map::class.java)
            )
            proxyHandles[key] = handle
        } catch (e: Exception) {
            log.debug("putProxy failed for key=$key", e)
        }
    }

    private fun invokeInit(key: String, loader: URLClassLoader) {
        try {
            val clazz = loader.loadClass(Constant.catVodInit)
            val lookup = MethodHandles.lookup()
            val handle = lookup.findStatic(
                clazz,
                "init",
                MethodType.methodType(Void.TYPE)
            )
            handle.invokeExact()
            initHandles[key] = handle
        } catch (e: Exception) {
            log.debug("invokeInit failed for key=$key", e)
        }
    }

    fun getSpider(key: String, api: String, ext: String, jar: String): Spider {
        try {
            // 必须在任何爬虫类初始化前执行：UC 的 UCApi$Loader 一旦被坏 token 缓存
            // 触发类初始化失败，同一 JVM 内无法恢复（后续全是 NoClassDefFoundError）。
            UcTokenCacheGuard.ensureUsable()
            val jaKey = Utils.md5(jar)
            val spKey = jaKey + key

            spiders[spKey]?.let { return it }

            if (loaders[jaKey] == null) {
                loadJar(jaKey, jar)
            }
            val loader = loaders[jaKey] ?: return Spider()

            val constructor = spiderConstructors.computeIfAbsent(spKey) {
                val classPath = "${Constant.catVodSpider}.${api.replace("csp_", "")}"
                val clazz = loader.loadClass(classPath)
                val ctor = clazz.getDeclaredConstructor() as Constructor<out Spider>
                ctor.isAccessible = true
                ctor
            }

            val spider = constructor.newInstance() as Spider
            // init 失败不致命：很多站点只有“登录/网盘”分支依赖 init，列表与详情仍可用；
            // 原版 CatVod 同样忽略 init 异常继续使用。异常记录到告警，随结果回传给前端。
            try {
                spider.init(ext)
                initWarnings.remove(key)
            } catch (e: Throwable) {
                val msg = "${e.javaClass.simpleName}: ${e.message}"
                // 绝大多数是网盘站在 init 里做登录/容量查询（如天翼云盘返回 null）。
                // 这不影响列表/搜索/详情，按“可忽略”记录，避免每次启动都像致命错误。
                log.warn("spider init failed key=$key api=$api（已忽略，站点继续可用）: {}", msg)
                log.debug("spider init 异常详情 key=$key", e)
                initWarnings[key] = msg
            }
            spiders[spKey] = spider
            return spider
        } catch (e: Exception) {
            log.error("getSpider failed key=$key api=$api", e)
            // 不再返回空白 Spider：否则前端只能看到“暂无内容”，无法区分站点自身故障
            throw IllegalStateException(
                "爬虫初始化失败($key/$api): ${e.javaClass.simpleName}: ${e.message}",
                e
            )
        }
    }

    private fun download(jar: String, expectMd5: String = ""): File {
        val jarPath = Paths.jar(jar)
        val tmp = File(jarPath.parentFile, jarPath.name + ".part")
        log.info("download jar {} -> {}", jar, jarPath)
        val bytes = Http.Get(jar).execute().use { resp ->
            if (!resp.isSuccessful) throw IllegalStateException("HTTP ${resp.code}")
            resp.body?.bytes() ?: throw IllegalStateException("响应体为空")
        }
        if (bytes.size < 4 || bytes[0] != 0x50.toByte() || bytes[1] != 0x4B.toByte()) {
            throw IllegalStateException("下载内容不是 jar/zip（size=${bytes.size}）")
        }
        if (expectMd5.isNotEmpty()) {
            val got = Utils.md5(bytes)
            if (!got.equals(expectMd5, ignoreCase = true)) {
                log.warn("jar md5 与配置不一致: 期望 {} 实际 {}", expectMd5, got)
            }
        }
        tmp.writeBytes(bytes)
        if (jarPath.exists() && !jarPath.delete()) log.warn("旧 jar 删除失败 {}", jarPath)
        if (!tmp.renameTo(jarPath)) {
            tmp.copyTo(jarPath, overwrite = true)
            tmp.delete()
        }
        return jarPath
    }

    fun proxyInvoke(params: Map<String, String>): Array<Any>? {
        return try {
            val md5 = Utils.md5(recent ?: "")
            val handle = proxyHandles[md5] ?: return null
            handle.invokeExact(params) as Array<Any>
        } catch (e: Exception) {
            log.error("proxyInvoke failed", e)
            null
        }
    }

    fun SetRecent(jar: String?) {
        recent = jar
    }
}

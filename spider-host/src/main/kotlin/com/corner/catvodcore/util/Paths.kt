package com.corner.catvodcore.util

import org.slf4j.LoggerFactory
import java.io.File

// 自包含版本：不再依赖 SnackBar / UserDataDirProvider，
// 目录约定与原应用保持一致（%AppData%/TV-Multiplatform/cache），便于共享 jar 与配置缓存
object Paths {
    private val ApplicationName = "TV-Multiplatform"
    private val log = LoggerFactory.getLogger("Paths")

    private val userDataDir: File by lazy {
        val os = System.getProperty("os.name").orEmpty().lowercase()
        when {
            os.contains("win") -> File(System.getenv("AppData"), "$ApplicationName/cache")
            os.contains("mac") || os.contains("darwin") -> File(System.getProperty("user.home"), "Library/Caches/$ApplicationName")
            else -> File(System.getProperty("user.home"), ".cache/$ApplicationName")
        }
    }

    private fun File.check(): File {
        if (!exists()) {
            mkdirs()
        }
        return this
    }

    fun root(): File {
        return userDataDir.resolve("data")
    }

    fun userDataRoot(): File {
        return userDataDir
    }

    fun doh(): File {
        return cache("doh").check()
    }

    private fun cache(path: String): File {
        return root().resolve("cache").resolve(path)
    }

    fun db(): String {
        val path = userDataRoot().resolve("db").check().resolve("tv.db")
        return path.path
    }

    fun local(jar: String): File {
        val file = File(jar.replace("file:/", "").replace("file:\\", ""))
        return if (file.exists()) file else {
            log.info("jar文件不存在 {}", jar)
            File(jar)
        }
    }

    fun jar(): File {
        return cache("jar").check()
    }

    fun jar(fileName: String): File {
        return File(jar(), Utils.md5(fileName) + ".jar")
    }

    fun write(path: File, bytes: ByteArray?): File {
        if (bytes == null || bytes.isEmpty()) {
            return path
        } else {
            path.writeBytes(bytes)
        }
        return path
    }

    fun picCache(): File {
        return cache("pic").check()
    }

    fun configCache(): File {
        return cache("config").check()
    }

    fun hotCache(): File {
        return cache("hot").check()
    }

    fun playerLog(): File {
        return root().check().resolve("playerLog.txt")
    }

    fun logPath(): File {
        return root().resolve("log")
    }
}

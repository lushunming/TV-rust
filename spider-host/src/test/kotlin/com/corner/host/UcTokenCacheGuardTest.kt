package com.corner.host

import com.google.gson.JsonObject
import org.junit.jupiter.api.Assertions.assertFalse
import org.junit.jupiter.api.Assertions.assertTrue
import org.junit.jupiter.api.Test
import org.junit.jupiter.api.io.TempDir
import java.io.File
import java.nio.file.Path

class UcTokenCacheGuardTest {
    @TempDir
    lateinit var tempDir: Path

    @Test
    fun `valid token cache is kept`() {
        val file = write(cache("""{"access_token":"a","refresh_token":"r"}"""))

        assertTrue(UcTokenCacheGuard.checkFile(file))
        assertTrue(file.exists())
    }

    @Test
    fun `error response is quarantined`() {
        val file = write(
            cache("""{"status":-1,"errno":11005,"error_info":"Connection reset","refresh_token":""}""")
        )

        assertFalse(UcTokenCacheGuard.checkFile(file))
        assertFalse(file.exists(), "坏缓存应被移走")
        assertTrue(
            tempDir.toFile().listFiles()!!.any { it.name.startsWith(".uctoken.bad-") },
            "应保留隔离后的文件"
        )
    }

    @Test
    fun `empty cookie is treated as no token`() {
        val file = write(cache(""))

        assertTrue(UcTokenCacheGuard.checkFile(file))
        assertTrue(file.exists())
    }

    @Test
    fun `malformed json is quarantined`() {
        val file = write("not-json")

        assertFalse(UcTokenCacheGuard.checkFile(file))
        assertFalse(file.exists())
    }

    @Test
    fun `missing file is usable`() {
        assertTrue(UcTokenCacheGuard.checkFile(File(tempDir.toFile(), ".uctoken")))
    }

    private fun cache(cookie: String): String {
        val user = JsonObject().apply { addProperty("cookie", cookie) }
        return JsonObject().apply { add("user", user) }.toString()
    }

    private fun write(content: String): File =
        File(tempDir.toFile(), ".uctoken").apply { writeText(content, Charsets.UTF_8) }
}

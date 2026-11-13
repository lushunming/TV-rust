package com.corner.catvodcore.util

import com.github.catvod.bean.Doh
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import okhttp3.*
import okhttp3.Credentials.basic
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.dnsoverhttps.DnsOverHttps
import java.net.InetSocketAddress
import java.net.Proxy
import java.security.KeyStore
import java.security.SecureRandom
import java.security.cert.X509Certificate
import java.time.Duration
import java.time.temporal.ChronoUnit
import java.util.concurrent.TimeUnit
import javax.net.ssl.*

// 自包含版本：剥离 KtorClient / ProxySelect 依赖。
// 代理改为直接解析 host:port，DoH 保留（okhttp-dnsoverhttps）。
class Http {
    companion object {
        @Volatile
        private var doh: DnsOverHttps? = null

        @Volatile
        private var client: OkHttpClient? = null

        @Volatile
        private var proxy: Proxy? = null

        private val clientLock = Any()
        private val defaultHeaders: Headers = Headers.Builder().build()

        fun getSSLSocketFactory(): SSLSocketFactory {
            try {
                val sslContext = SSLContext.getInstance("SSL")
                sslContext.init(null, getTrustManager(), SecureRandom())
                return sslContext.socketFactory
            } catch (e: Exception) {
                throw RuntimeException(e)
            }
        }

        private fun getTrustManager(): Array<TrustManager> {
            return arrayOf(object : X509TrustManager {
                override fun checkClientTrusted(chain: Array<X509Certificate>, authType: String) {
                }

                override fun checkServerTrusted(chain: Array<X509Certificate>, authType: String) {
                }

                override fun getAcceptedIssuers(): Array<X509Certificate> {
                    return arrayOf()
                }
            })
        }

        fun getHostnameVerifier(): HostnameVerifier {
            return HostnameVerifier { _: String?, _: SSLSession? -> true }
        }

        fun getX509TrustManager(): X509TrustManager? {
            var trustManager: X509TrustManager? = null
            try {
                val trustManagerFactory = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm())
                trustManagerFactory.init(null as KeyStore?)
                val trustManagers = trustManagerFactory.trustManagers
                check(!(trustManagers.size != 1 || trustManagers[0] !is X509TrustManager)) { "Unexpected default trust managers:" + trustManagers.contentToString() }
                trustManager = trustManagers[0] as X509TrustManager
            } catch (e: Exception) {
                e.printStackTrace()
            }

            return trustManager
        }

        private val builder: OkHttpClient.Builder
            get() {
                val dispatcher = Dispatcher()
                val b = OkHttpClient.Builder().connectTimeout(10, TimeUnit.SECONDS)
                    .readTimeout(10, TimeUnit.SECONDS).writeTimeout(10, TimeUnit.SECONDS)
                    .followRedirects(true)
                    .sslSocketFactory(getSSLSocketFactory(), requireNotNull(getX509TrustManager()))
                    .hostnameVerifier(getHostnameVerifier())
                    .dispatcher(dispatcher)
                    .dns(dns())
                proxy?.let { b.proxy(it) }
                return b
            }

        /** 统一 URL 解析：非法地址抛出可读异常，而不是到处 NPE */
        private fun requireHttpUrl(url: String): HttpUrl {
            val trimmed = url.trim()
            return trimmed.toHttpUrlOrNull()
                ?: trimmed.replace(" ", "%20").toHttpUrlOrNull()
                ?: throw IllegalArgumentException("非法的 URL: $url")
        }

        fun dns(): Dns {
            return if (doh == null) Dns.SYSTEM else doh!!
        }

        fun setDoh(doh: Doh) {
            val dnsClient =
                OkHttpClient.Builder().cache(Cache(Paths.doh(), 8000))
                    .callTimeout(Duration.of(5, ChronoUnit.SECONDS))
                    .build()
            val dnsOverHttps =
                DnsOverHttps.Builder().client(dnsClient).bootstrapDnsHosts(doh.hosts)
                    .url(requireHttpUrl(doh.url)).build()
            synchronized(clientLock) {
                Companion.doh = dnsOverHttps
                client?.dispatcher?.executorService?.shutdownNow()
                client = builder.build()
            }
        }

        fun setProxy(proxy: String) {
            Companion.proxy = parseProxy(proxy)
            synchronized(clientLock) {
                client?.dispatcher?.executorService?.shutdownNow()
                client = null
            }
        }

        private fun parseProxy(proxy: String): Proxy? {
            try {
                val cleaned = proxy.trim().removePrefix("http://").removePrefix("https://")
                if (cleaned.isBlank()) return null
                val parts = cleaned.split(":")
                val host = parts[0]
                val port = parts.getOrNull(1)?.toIntOrNull() ?: 8080
                return Proxy(Proxy.Type.HTTP, InetSocketAddress(host, port))
            } catch (e: Exception) {
                return null
            }
        }

        fun setProxyHosts(hosts: List<String>?) {
            // spider-host 不做按域名分流代理，保留空实现以兼容调用方
        }

        fun client(connectTimeout: Int): java.net.http.HttpClient.Builder {
            return java.net.http.HttpClient.newBuilder().connectTimeout(Duration.of(connectTimeout.toLong(), ChronoUnit.MINUTES))
        }

        fun client(): OkHttpClient {
            client?.let { return it }
            synchronized(clientLock) {
                client?.let { return it }
                return builder.build().also { client = it }
            }
        }

        @JvmOverloads
        fun Get(url: String, params: Map<String, String?>? = null, headers: Headers? = null): Call {
            val builder: HttpUrl.Builder = requireHttpUrl(url).newBuilder()
            params?.forEach { (name: String, value: String?) ->
                builder.addQueryParameter(name, value)
            }
            val httpUrl: HttpUrl = builder.build()
            val request: Request.Builder = Request.Builder()
                .url(httpUrl.toUrl())
                .headers(headers ?: defaultHeaders)
            checkBaseAuth(httpUrl, request)
            return client().newCall(request.build())
        }

        fun Post(url: String, params: Map<String, String>?, headers: Headers?): Call {
            val builder: HttpUrl.Builder = requireHttpUrl(url).newBuilder()
            val httpUrl: HttpUrl = builder.build()
            val request: Request.Builder = Request.Builder()
                .url(httpUrl.toUrl())
                .post(Json.encodeToString(params).toByteArray().toRequestBody())
                .headers(headers ?: defaultHeaders)
            checkBaseAuth(httpUrl, request)
            return client().newCall(request.build())
        }

        private fun checkBaseAuth(url: HttpUrl, request: Request.Builder) {
            if (url.toUrl().userInfo != null) {
                request.header("Authorization", basic(url.username, url.password))
            }
        }

        private fun buildUrl(url: String, params: Map<String, String>): HttpUrl {
            val builder: HttpUrl.Builder = requireHttpUrl(url).newBuilder()
            for ((key, value) in params.entries)
                builder.addQueryParameter(key, value)
            return builder.build()
        }

        fun newCall(url: String, headers: Headers, params: Map<String, String>): Call {
            return client().newCall(Request.Builder().url(buildUrl(url, params)).headers(headers).build())
        }

        fun newCall(url: String, headers: Headers, body: RequestBody): Call {
            return client().newCall(Request.Builder().url(requireHttpUrl(url)).headers(headers).post(body).build())
        }

        fun newCall(api: String, headers: Headers): Call {
            return client().newCall(
                Request.Builder().url(requireHttpUrl(api)).headers(headers)
                    .build()
            )
        }

        fun toBody(params: Map<String, String>): FormBody {
            val builder = FormBody.Builder()
            for (param in params) {
                builder.add(param.key, param.value)
            }
            return builder.build()
        }

    }

}

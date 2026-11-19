package app.justyo.android

import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import uniffi.yo.*

class HubTransport(private val unsupported: () -> Unit) : HttpTransport {
    private val stopped = AtomicBoolean(false)
    // Separate pools: anonymous deliveries must carry no connection history from identified requests.
    private val signed = plain()
    private val anonymous = plain()
    private fun plain() = OkHttpClient.Builder().callTimeout(70, TimeUnit.SECONDS)
        .connectTimeout(15, TimeUnit.SECONDS).followRedirects(false).followSslRedirects(false).build()
    override fun request(method: String, path: String, headers: List<Header>, body: ByteArray): HttpResult {
        if (stopped.get()) return HttpResult(426u, byteArrayOf(), null)
        return try {
            require(path.startsWith("/") && !path.startsWith("//"))
            val request = Request.Builder().url(BuildConfig.HUB + path).header("Yo-App", BuildConfig.YO_APP)
            headers.forEach { request.header(it.name, it.value) }
            request.method(method, if (method == "GET" || method == "HEAD") null else body.toRequestBody())
            val identified = headers.any { it.name.equals("yo-device", true) }
            val client = if (!identified && anonymousRequest(method, path)) anonymous else signed
            client.newCall(request.build()).execute().use { response ->
                if (response.code == 426) { stopped.set(true); unsupported() }
                val input = response.body
                require(input.contentLength() <= MAX_ANSWER) { "Answer too large" }
                val bytes = input.byteStream().use { stream ->
                    val output = java.io.ByteArrayOutputStream()
                    val buffer = ByteArray(8192)
                    while (true) {
                        val n = stream.read(buffer)
                        if (n < 0) break
                        require(output.size() + n <= MAX_ANSWER) { "Answer too large" }
                        output.write(buffer, 0, n)
                    }
                    output.toByteArray()
                }
                HttpResult(response.code.toUShort(), bytes, null)
            }
        } catch (_: Exception) { HttpResult(0u, byteArrayOf(), "Could not reach the hub") }
    }
    fun close() { listOf(signed,anonymous).forEach { it.dispatcher.cancelAll();it.connectionPool.evictAll() } }
    companion object {
        const val MAX_ANSWER = 11 * 1024 * 1024
    }
}

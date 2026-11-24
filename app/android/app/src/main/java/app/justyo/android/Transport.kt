package app.justyo.android

import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.concurrent.thread
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import uniffi.yo.*

class HubTransport(
    private val unreachable: String,
    private val reached: (Boolean) -> Unit = {},
    /** TLS refused the hub's certificate for its dates: the hub is there, the phone's clock is off. */
    private val clockOff: () -> Unit = {},
    private val unsupported: () -> Unit,
) : HttpTransport {
    private val stopped = AtomicBoolean(false)
    // Separate pools: anonymous deliveries must carry no connection history from identified requests.
    private val signed = plain()
    private val anonymous = plain()
    // The hub holds a sync up to 25 s without a byte: OkHttp's default 10-s read timeout cut every one of them, so the
    // listener only backed off (up to 30 s between tries) and Status said "Waiting". The call timeout bounds it instead.
    // HTTP/2 pings every 15 s: a connection the network dropped silently (a NAT that forgot it, a suspended phone) fails
    // its ping and is replaced, instead of holding a Yo until the read timeout.
    private fun plain() = OkHttpClient.Builder().callTimeout(70, TimeUnit.SECONDS).readTimeout(60, TimeUnit.SECONDS)
        .connectTimeout(15, TimeUnit.SECONDS).pingInterval(15, TimeUnit.SECONDS)
        .followRedirects(false).followSslRedirects(false).build()
    override fun request(method: String, path: String, headers: List<Header>, body: ByteArray): HttpResult {
        if (stopped.get()) return HttpResult(426u, byteArrayOf(), null)
        return try {
            require(path.startsWith("/") && !path.startsWith("//"))
            val request = Request.Builder().url(BuildConfig.HUB + path).header("Yo-App", YO_APP_WIRE)
            headers.forEach { request.header(it.name, it.value) }
            request.method(method, if (method == "GET" || method == "HEAD") null else body.toRequestBody())
            val identified = headers.any { it.name.equals("yo-device", true) }
            val client = if (!identified && anonymousRequest(method, path)) anonymous else signed
            val started = System.nanoTime()
            client.newCall(request.build()).execute().use { response ->
                if (Lat.on) Lat.mark("http $method ${path.substringBefore('?').take(24)} ${response.protocol} ${response.code} ${(System.nanoTime() - started) / 1_000_000}ms")
                if (response.code == 426) { stopped.set(true); unsupported() }
                reached(true); Problems.answered(response.code)
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
        } catch (e: Exception) {
            // TLS refused the hub's certificate for its dates: the phone's clock is off (the core's `clock:`), not the
            // connection: it must not say "Offline".
            val dates = refusedForDates(e)
            if (dates) clockOff() else reached(false)
            HttpResult(0u, byteArrayOf(), if (dates) "clock: $unreachable" else unreachable)
        }
    }
    /** Connections from before a suspension or on another network: dropped, so the next request opens a fresh one at
     *  once instead of waiting on a dead socket. Off the main thread (it calls from `onStart`): closing a TLS socket
     *  writes to it, which Android refuses there (NetworkOnMainThreadException, rethrown by OkHttp: a crash). [now]:
     *  done before returning, for a caller off the main thread whose next request must not take an old one. */
    fun forgetConnections(now: Boolean = false) {
        val evict = { listOf(signed, anonymous).forEach { it.connectionPool.evictAll() } }
        if (now) runCatching(evict) else offMain(evict)
    }
    fun close() = offMain { listOf(signed, anonymous).forEach { it.dispatcher.cancelAll(); it.connectionPool.evictAll() } }
    private fun offMain(work: () -> Unit) { thread(name = "yo-connections") { runCatching(work) } }
    companion object {
        const val MAX_ANSWER = 11 * 1024 * 1024
    }
}

/** A certificate refused for its dates anywhere in [e]'s causes (Conscrypt: an SSLHandshakeException over a
 *  CertificateException over a CertPathValidatorException over the expired or not yet valid certificate's exception). */
internal fun refusedForDates(e: Throwable) = generateSequence(e) { it.cause }.take(16).any {
    it is java.security.cert.CertificateExpiredException || it is java.security.cert.CertificateNotYetValidException ||
        (it is java.security.cert.CertPathValidatorException &&
            (it.reason == java.security.cert.CertPathValidatorException.BasicReason.EXPIRED ||
                it.reason == java.security.cert.CertPathValidatorException.BasicReason.NOT_YET_VALID))
}

/** Yo-App on the wire (spec 6.3): the build and, for a release, its version ("android/202611241344 1.0.0-RC36"). */
private val YO_APP_WIRE = BuildConfig.YO_APP + if (BuildConfig.YO_VERSION.isEmpty()) "" else " ${BuildConfig.YO_VERSION}"

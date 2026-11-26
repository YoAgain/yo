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
    private val signed = plain(0)
    private val anonymous = plain(1)
    // The hub holds a sync up to 25 s without a byte: OkHttp's default 10-s read timeout cut every one of them, so the
    // listener only backed off (up to 30 s between tries) and Status said "Waiting". The call timeout bounds it instead.
    // HTTP/2 pings every 15 s: a connection the network dropped silently (a NAT that forgot it, a suspended phone) fails
    // its ping and is replaced, instead of holding a Yo until the read timeout.
    // The play build hands each line's requests to its own Cronet engine once one is there (HTTP/3, Quic.kt); OkHttp's
    // pool (5 minutes idle) and pings stay the way for the floss build and until then.
    private fun plain(line: Int) = OkHttpClient.Builder().callTimeout(70, TimeUnit.SECONDS).readTimeout(60, TimeUnit.SECONDS)
        .connectTimeout(15, TimeUnit.SECONDS).pingInterval(15, TimeUnit.SECONDS)
        .addInterceptor { chain -> Quic.interceptor(line)?.intercept(chain) ?: chain.proceed(chain.request()) }
        .followRedirects(false).followSslRedirects(false).build()
    override fun request(method: String, path: String, headers: List<Header>, body: ByteArray): HttpResult {
        // A request wrapped in Oblivious HTTP (the core's `use_hub`): its connection and the request inside, for this
        // app only, never sent; the status inside comes by `opened`.
        val line = headers.firstOrNull { it.name == "yo-ohttp-line" }?.value
        val inner = headers.firstOrNull { it.name == "yo-ohttp-inner" }?.value
        val attestation = path == "/v1/attest" || path == "/v1/attest/challenge"
        // Status's Privacy Relay: discovery, help, admin and ticket transfers go direct either way, so only the routes
        // OHTTP serves count.
        if (inner != null) viaOhttp.value = true
        else if (path.startsWith("/v1/") && !path.startsWith("/v1/ohttp") && !path.startsWith("/v1/blobs/t/") &&
            !path.startsWith("/v1/help/") && !path.startsWith("/v1/admin")) viaOhttp.value = false
        if (stopped.get()) return HttpResult(426u, byteArrayOf(), null, emptyList())
        return try {
            require(path.startsWith("/") && !path.startsWith("//"))
            val request = Request.Builder().url(Hub.url + path).header("Yo-App", YO_APP_WIRE)
            headers.forEach { if (it.name != "yo-ohttp-line" && it.name != "yo-ohttp-inner") request.header(it.name, it.value) }
            request.method(method, if (method == "GET" || method == "HEAD") null else body.toRequestBody())
            val identified = headers.any { it.name.equals("yo-device", true) }
            val client = when (line) {
                null -> if (!identified && anonymousRequest(method, path)) anonymous else signed
                "anonymous" -> anonymous
                else -> signed
            }
            val started = System.nanoTime()
            client.newCall(request.build()).execute().use { response ->
                if (Lat.on) Lat.mark("http ${inner ?: "$method $path"}".substringBefore('?').take(29) + " ${response.protocol} ${response.code} ${(System.nanoTime() - started) / 1_000_000}ms")
                if (!attestation) {
                    if (response.code == 426) { stopped.set(true); unsupported() }
                    reached(true); if (inner == null) Problems.answered(response.code)
                }
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
                HttpResult(response.code.toUShort(), bytes, null, response.headers.map { (name, value) -> Header(name, value) })
            }
        } catch (e: Exception) {
            // TLS refused the hub's certificate for its dates: the phone's clock is off (the core's `clock:`), not the
            // connection: it must not say "Offline".
            val dates = refusedForDates(e)
            if (!attestation) { if (dates) clockOff() else reached(false) }
            HttpResult(0u, byteArrayOf(), if (dates) "clock: $unreachable" else unreachable, emptyList())
        }
    }
    override fun opened(method: String, path: String, status: UShort) {
        if (path == "/v1/attest" || path == "/v1/attest/challenge") return
        if (status.toInt() == 426) { stopped.set(true); unsupported() }
        Problems.answered(status.toInt())
    }
    /** Connections from before a suspension or on another network: dropped, so the next request opens a fresh one at
     *  once instead of waiting on a dead socket. Off the main thread (it calls from `onStart`): closing a TLS socket
     *  writes to it, which Android refuses there (NetworkOnMainThreadException, rethrown by OkHttp: a crash). [now]:
     *  done before returning, for a caller off the main thread whose next request must not take an old one. */
    fun forgetConnections(now: Boolean = false) {
        val evict = { listOf(signed, anonymous).forEach { it.connectionPool.evictAll() } }
        if (now) runCatching(evict) else offMain(evict)
    }
    /** An account-free GET of the app's own (the hub's `.well-known` files) on the anonymous line's warm connection:
     *  its status and text, never a connection of its own. Blocking: off the main thread. */
    fun publicGet(path: String, seconds: Long): Pair<Int, String> {
        require(path.startsWith("/") && !path.startsWith("//"))
        val client = anonymous.newBuilder().callTimeout(seconds, TimeUnit.SECONDS).build()
        val request = Request.Builder().url(Hub.url.trimEnd('/') + path).header("Yo-App", YO_APP_WIRE).build()
        return client.newCall(request).execute().use { it.code to it.body.string() }
    }
    fun close() = offMain { listOf(signed, anonymous).forEach { it.dispatcher.cancelAll(); it.connectionPool.evictAll() } }
    private fun offMain(work: () -> Unit) { thread(name = "yo-connections") { runCatching(work) } }
    companion object {
        const val MAX_ANSWER = 11 * 1024 * 1024
        /** The last YoIP request went inside Oblivious HTTP (spec 4.1); null before any. */
        val viaOhttp = kotlinx.coroutines.flow.MutableStateFlow<Boolean?>(null)
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

/** Yo-App on the wire (spec 4.1): the build and, for a release, its version ("android/202611241344 1.0.0-RC36"). */
private val YO_APP_WIRE = BuildConfig.YO_APP + if (BuildConfig.YO_VERSION.isEmpty()) "" else " ${BuildConfig.YO_VERSION}"

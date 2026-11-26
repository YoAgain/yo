package app.justyo.android

import android.content.Context
import com.google.android.gms.net.CronetProviderInstaller
import com.google.net.cronet.okhttptransport.CronetInterceptor
import okhttp3.Interceptor
import org.chromium.net.CronetProvider
import org.chromium.net.ExperimentalCronetEngine

/**
 * HTTP/3 (QUIC, the hub's `alt-svc: h3`) through Cronet behind OkHttp: one engine per line, so the identified and the
 * anonymous line share no connection, socket or TLS session (spec 1.1). Nothing kept on disk (no storage path, no
 * cache), no 0-RTT. Cronet falls back to HTTP/2 itself where UDP does not get through. Until a native Cronet is there
 * (Google Play services, or the system's HttpEngine), and where none is, OkHttp's own HTTP/2.
 */
internal object Quic {
    /** Line 0 identified, 1 anonymous. */
    @Volatile private var lines: List<Interceptor>? = null

    fun start(context: Context) {
        val app = context.applicationContext
        runCatching {
            CronetProviderInstaller.installProvider(app).addOnCompleteListener {
                // Without Play services' provider the system's HttpEngine may still be there.
                runCatching { lines = listOf(line(app) ?: return@runCatching, line(app) ?: return@runCatching) }
                    .onFailure { if (Lat.on) Lat.mark("quic: ${it.javaClass.simpleName}") }
            }
        }
    }

    fun interceptor(line: Int): Interceptor? = lines?.getOrNull(line)

    /** A native Cronet's engine for one line, or null (only the fallback provider, which has no QUIC). */
    private fun line(context: Context): Interceptor? {
        if (CronetProvider.getAllProviders(context).none { it.isEnabled && it.name != CronetProvider.PROVIDER_NAME_FALLBACK }) {
            return null
        }
        // The best provider there is: a native one ranks above the fallback.
        val builder = ExperimentalCronetEngine.Builder(context)
        builder.enableQuic(true).enableHttp2(true)
            .enableHttpCache(ExperimentalCronetEngine.Builder.HTTP_CACHE_DISABLED, 0)
            .addQuicHint(Hub.host, 443, 443)
            // A replayable first flight never leaves: no 0-RTT (TLS or QUIC).
            .setExperimentalOptions("""{"QUIC":{"disable_tls_zero_rtt":true}}""")
        return CronetInterceptor.newBuilder(builder.build()).build()
    }
}

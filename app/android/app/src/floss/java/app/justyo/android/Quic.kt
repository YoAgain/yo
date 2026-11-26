package app.justyo.android

import android.content.Context
import okhttp3.Interceptor

/** No Google libraries here: OkHttp's own HTTP/2 on both lines. */
internal object Quic {
    fun start(context: Context) {}
    fun interceptor(line: Int): Interceptor? = null
}

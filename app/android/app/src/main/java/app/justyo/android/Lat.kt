package app.justyo.android

import android.util.Log

/** Latency marks for a debug launch with `--ez yo_perf true`: wall-clock microseconds in logcat (tag YoLat), so a
 *  phone's tap lines up with another device's receive. Off, it costs one volatile read. */
internal object Lat {
    @Volatile var on = false
    fun mark(what: String) {
        if (on) Log.d("YoLat", "${System.currentTimeMillis() * 1000 + (System.nanoTime() / 1000) % 1000} $what")
    }
    /** Marks the next frame after this point: when the change just made reaches the screen. */
    fun frame(what: String) {
        if (!on) return
        android.os.Handler(android.os.Looper.getMainLooper()).post {
            mark("main $what")
            android.view.Choreographer.getInstance().postFrameCallback { mark("frame $what") }
        }
    }
}

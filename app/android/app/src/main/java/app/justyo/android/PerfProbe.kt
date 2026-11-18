package app.justyo.android

import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import android.view.FrameMetrics
import android.view.Window

/** Local aggregate timings only. Enable a debug launch with: --ez yo_perf true. */
internal class PerfProbe(private val window: Window) : AutoCloseable {
    private val thread = HandlerThread("YoFrames").apply { start() }
    private var frames = 0L
    private var late = 0L
    private var worst = 0L
    private val listener = Window.OnFrameMetricsAvailableListener { _, metrics, dropped ->
        val duration = metrics.getMetric(FrameMetrics.TOTAL_DURATION)
        val budget = (1_000_000_000.0 / window.decorView.display.refreshRate).toLong()
        frames++; if(duration > budget) late++
        worst = maxOf(worst,duration)
        if(frames % 120 == 0L) Log.d("YoPerf", "frames=$frames late=$late worstMs=${worst/1_000_000} dropped=$dropped")
    }
    init { window.addOnFrameMetricsAvailableListener(listener, Handler(thread.looper)) }
    override fun close() { window.removeOnFrameMetricsAvailableListener(listener); thread.quitSafely() }
}

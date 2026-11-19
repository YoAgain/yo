package app.justyo.android

import android.app.ActivityManager
import android.app.ApplicationExitInfo
import android.content.Context
import android.os.Build
import androidx.core.content.edit
import org.json.JSONArray
import org.json.JSONObject
import uniffi.yo.HttpTransport
import java.util.UUID

/** Whitelist code locations only. Never retain exception messages, thread names, paths or memory. */
internal object CrashFrames {
    fun javaFrames(error: Throwable): String = error.stackTrace.take(64).mapNotNull {
        if(it.className.startsWith("app.justyo.android.") || it.className.startsWith("uniffi.yo."))
            "${it.className}.${it.methodName.replace(Regex("[^A-Za-z0-9_$<>]"), "").take(100)}:${it.lineNumber}" else null
    }.joinToString("\n")
    private data class Field(val id: Int,val number: Long=0,val bytes: ByteArray=byteArrayOf())
    // AOSP debuggerd/proto/tombstone.proto. Parse only bounded wire fields, no generated reflection or dump strings.
    private fun fields(data: ByteArray): List<Field> {
        var p=0
        fun varint(): Long { var v=0L;for(n in 0..9) { require(p<data.size);val b=data[p++].toInt() and 255;v=v or ((b and 127).toLong() shl (7*n));if(b<128) return v };error("varint") }
        val result=ArrayList<Field>()
        while(p<data.size && result.size<4096) {
            val tag=varint();val id=(tag ushr 3).toInt();require(id>0)
            when((tag and 7).toInt()) {
                0 -> result+=Field(id,varint())
                1 -> { require(p+8<=data.size);p+=8 }
                2 -> { val size=varint();require(size>=0 && size<=data.size-p);val end=p+size.toInt();result+=Field(id,bytes=data.copyOfRange(p,end));p=end }
                5 -> { require(p+4<=data.size);p+=4 }
                else -> error("wire")
            }
        }
        return result
    }
    fun nativeFrames(data: ByteArray): String = runCatching {
        require(data.size<=1024*1024)
        val tomb=fields(data);val tid=tomb.firstOrNull { it.id==6 }?.number
        val thread=tomb.filter { it.id==16 }.map { fields(it.bytes) }.firstOrNull { entry -> entry.any { it.id==1 && it.number==tid } }
        val frames=thread?.firstOrNull { it.id==2 }?.let { fields(it.bytes) }.orEmpty().filter { it.id==4 }.take(64)
        frames.mapNotNull { frame ->
            val f=fields(frame.bytes);val name=f.firstOrNull { it.id==6 }?.bytes?.toString(Charsets.UTF_8)?.substringAfterLast('/')
            // Only known packaged/system binaries; no arbitrary file names or symbols.
            val binary=name?.takeIf { it in setOf("libyo.so","libc.so","libart.so","libandroid_runtime.so","libc++_shared.so","base.apk") }
            binary?.let { "$it +0x${(f.firstOrNull { field -> field.id==1 }?.number ?: 0).toULong().toString(16)}" }
        }.joinToString("\n")
    }.getOrDefault("")
}
class CrashReports(context: Context, private val transport: HttpTransport) {
    private val preferences = context.getSharedPreferences("crashes", Context.MODE_PRIVATE)
    private val manager = context.getSystemService(ActivityManager::class.java)
    /** Sent automatically; Settings › Status › Send Crashes turns it off (reports then stay on this phone). */
    var enabled: Boolean
        get() = preferences.getBoolean("enabled", true)
        set(value) { preferences.edit { putBoolean("enabled", value) } }
    @Synchronized fun history(): List<String> = rows().map { it.getString("text") }.reversed()
    private fun rows(): List<JSONObject> = runCatching { val array=JSONArray(preferences.getString("history","[]"));(0 until array.length()).map { array.getJSONObject(it) } }.getOrDefault(emptyList())
    private fun payload(exception: String,termination: String,frames: String) = JSONObject()
        .put("uuid",UUID.randomUUID().toString()).put("app",BuildConfig.YO_APP).put("os","Android ${Build.VERSION.SDK_INT}")
        .put("model","Android").put("exception",exception).put("termination",termination).put("frames",frames).put("details",frames).toString()
    @Synchronized private fun retain(text: String) {
        preferences.edit(commit=true) { putString("history",JSONArray((rows()+JSONObject().put("text",text).put("sent",false)).takeLast(16)).toString()) }
    }
    @Synchronized internal fun countNotificationFallback(reason: String) {
        val day = System.currentTimeMillis() / 86_400_000L
        val key = "fallback:$day:$reason"
        val count = preferences.getLong(key, 0)
        preferences.edit(commit = true) { putLong(key, count + 1) }
    }

    @Synchronized internal fun countStateFailure(kind: String) {
        val day = System.currentTimeMillis() / 86_400_000L
        val key = "state:$day:$kind"
        preferences.edit(commit = true) { putLong(key, preferences.getLong(key, 0) + 1) }
    }

    private fun flushCountedErrors() {
        for ((key, value) in preferences.all) {
            if (!enabled) return
            if ((!key.startsWith("fallback:") && !key.startsWith("state:")) || value !is Long) continue
            val (category, day, reason) = key.split(':', limit = 3)
            val body = JSONObject()
                .put("app", BuildConfig.YO_APP).put("os", "Android ${Build.VERSION.SDK_INT}")
                .put("model", "Android").put("domain", if (category == "state") "StateMigration" else "NotificationFallback")
                .put("place", if (category == "state") "StateDb" else "ForegroundInbox").put("code", reason)
                .put("details", "day=$day count=$value")
                .put("count", value.coerceAtMost(100_000).toString())
            fun upload() = transport.request("POST", "/v1/errors", emptyList(), body.toString().toByteArray()).status.toInt()
            var status = upload()
            if (status == 400) {
                // Hubs predating counted diagnostics accept the count in details.
                body.remove("count")
                status = upload()
            }
            if (status !in 200..299) return
            preferences.edit(commit = true) { remove(key) }
        }
    }

    /** Covers uncaught managed crashes on API 28–29 too; always chain Android's original crash handler. */
    fun install() {
        synchronized(CrashReports::class.java) { if(installed) return;installed=true }
        val previous=Thread.getDefaultUncaughtExceptionHandler()
        Thread.setDefaultUncaughtExceptionHandler { thread,error ->
            try { retain(payload(error.javaClass.name.takeIf { it.startsWith("java.") || it.startsWith("kotlin.") } ?: "Managed crash","Uncaught",CrashFrames.javaFrames(error))) }
            finally { previous?.uncaughtException(thread,error) }
        }
    }
    companion object { private var installed=false }
    private fun readBounded(input: java.io.InputStream): ByteArray {
        val output=java.io.ByteArrayOutputStream();val buffer=ByteArray(8192)
        while(output.size()<=1024*1024) { val n=input.read(buffer,0,minOf(buffer.size,1024*1024+1-output.size()));if(n<0) break;output.write(buffer,0,n) }
        return output.toByteArray()
    }
    @Synchronized fun flush() {
        if(Build.VERSION.SDK_INT>=30) {
            val through=preferences.getLong("through",0)
            val exits=manager.getHistoricalProcessExitReasons(null,0,16).filter {
                it.timestamp>through && it.reason in listOf(ApplicationExitInfo.REASON_CRASH_NATIVE,ApplicationExitInfo.REASON_ANR)
            }.sortedBy { it.timestamp }
            for(exit in exits) {
                val frames=if(Build.VERSION.SDK_INT>=31 && exit.reason==ApplicationExitInfo.REASON_CRASH_NATIVE) runCatching {
                    exit.traceInputStream?.use { CrashFrames.nativeFrames(readBounded(it)) }.orEmpty()
                }.getOrDefault("") else ""
                retain(payload("${exit.reason}/${exit.status}","OS exit",frames))
                preferences.edit { putLong("through",exit.timestamp) }
            }
        }
        if(!enabled) return
        flushCountedErrors()
        val rows=rows()
        for(row in rows) {
            if(!enabled) break
            if(row.optBoolean("sent")) continue
            val response=transport.request("POST","/v1/diagnostics/crash",emptyList(),row.getString("text").toByteArray())
            if(response.status.toInt() !in 200..299) break
            row.put("sent",true)
            preferences.edit { putString("history",JSONArray(rows).toString()) }
        }
    }
}

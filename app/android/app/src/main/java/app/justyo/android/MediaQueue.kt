package app.justyo.android

import androidx.core.graphics.withTranslation
import android.content.Context
import android.os.SystemClock
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Typeface
import kotlinx.coroutines.*
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONArray
import org.json.JSONObject
import uniffi.yo.*
import java.io.ByteArrayOutputStream
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean

/** `missed`: a try failed and it is tried again; `failed`: the hub refused it for good, kept until a tap retries it. */
private data class Outgoing(val id: String, val owner: String, val target: String, val media: YoMedia,
    val video: Boolean, val duration: Int, val held: Boolean = true, val uploaded: Boolean = false,
    val missed: Boolean = false, val failed: Boolean = false)
data class UndoStack(
    val count: Int, val until: Long, val duration: Long, val video: Boolean = false,
    val kind: ListKind = ListKind.YO, val pausedRemaining: Long? = null,
) {
    fun remaining(now: Long) = pausedRemaining ?: (until - now).coerceAtLeast(0)
    fun fraction(now: Long) = (remaining(now).toFloat() / duration.coerceAtLeast(1)).coerceIn(0f, 1f)
}

internal data class OutgoingPreview(val id: String, val video: Boolean)

/** All manifests and media are encrypted in the Keystore-backed vault, excluded from Android backup. */
class MediaQueue(
    private val context: Context,
    private val vault: Vault, private val scope: CoroutineScope, private val client: () -> YoClient?, private val session: () -> Long,
    private val absorb: suspend (YoClient, List<YoEvent>) -> Unit, private val changed: suspend () -> Unit,
) {
    private val undoMs=listTimings().undoMs.toLong()
    private val gate = Mutex()
    private val flushGate = Mutex()
    private val flushRequested = AtomicBoolean()
    private var outgoing = listOf<Outgoing>()
    private var incoming = listOf<Received>()
    private var loaded = false
    private var generation = 0
    private val releases = mutableMapOf<String, Job>()
    private val paused = mutableSetOf<String>()
    private val yoReleases = mutableMapOf<String, Job>()
    private val mediaStacks = mutableMapOf<String, UndoStack>()
    private val yoStacks = mutableMapOf<String, UndoStack>()
    private val stacks = MutableStateFlow<Map<String, UndoStack>>(emptyMap())
    val undo = stacks.asStateFlow()
    private fun publishStacks() { stacks.value = yoStacks + mediaStacks }
    private val previews=MutableStateFlow<Map<String,List<OutgoingPreview>>>(emptyMap())
    internal val thumbnails=previews.asStateFlow()
    private fun publishPreviews() { previews.value=outgoing.groupBy { it.target }.mapValues { (_,items) -> items.map { OutgoingPreview(it.id, it.video) } } }
    suspend fun thumbnail(id: String,incoming: Boolean=false): ByteArray? = gate.withLock { vault.read(if(incoming) "in-thumb-$id" else "thumb-$id") }
    var sent: ((String, Boolean, Boolean)->Unit)? = null
    var sendYo: (suspend (Friend) -> Unit)? = null
    /** A target with media that did not go (on) or nothing waiting for it any more (off): its bar says so. */
    var unsent: ((String, Boolean) -> Unit)? = null
    private var unsentTargets = emptySet<String>()
    /** A tap on the bar or the banner: the wait between tries ends now. */
    private val nudge = kotlinx.coroutines.channels.Channel<Unit>(kotlinx.coroutines.channels.Channel.CONFLATED)
    private fun publishUnsent() {
        val now = outgoing.filter { it.missed || it.failed }.map { it.target }.toSet()
        val before = unsentTargets; unsentTargets = now
        (now - before).forEach { unsent?.invoke(it, true) }
        (before - now).forEach { unsent?.invoke(it, false) }
    }
    suspend fun load() = gate.withLock {
        if (loaded) return@withLock
        // Parse failures propagate. A damaged manifest is never read as an empty queue.
        outgoing = vault.read("outbox")?.let { data -> JSONArray(String(data)).objects().map { j ->
            Outgoing(j.getString("id"), j.getString("owner"), j.getString("target"), mediaFrom(j.getJSONObject("media")),
                j.getBoolean("video"), j.getInt("duration"), j.getBoolean("held"), j.getBoolean("uploaded"),
                j.optBoolean("missed"), j.optBoolean("failed"))
        } }.orEmpty()
        incoming = vault.read("received")?.let { data -> JSONArray(String(data)).objects().map { j ->
            Received(j.getString("id"), j.getString("target"), j.getString("sender"), mediaFrom(j.getJSONObject("media")),
                j.getBoolean("video"), j.getInt("duration"), j.getLong("at"), j.getBoolean("ready"), j.optBoolean("consumed"))
        } }.orEmpty()
        for (held in outgoing.filter { it.held }) { vault.delete("out-${held.id}");vault.delete("thumb-${held.id}") }
        outgoing = outgoing.filter { !it.held }
        for (seen in incoming.filter { it.consumed }) { vault.delete(receivedFile(seen.id));vault.delete("in-thumb-${seen.id}") }
        incoming = incoming.filter { !it.consumed }
        loaded = true
        saveOut(); saveIn();publishPreviews();publishUnsent()
    }
    private fun saveOut() {
        check(loaded)
        vault.write("outbox", JSONArray(outgoing.map { item -> JSONObject().put("id",item.id).put("owner",item.owner)
            .put("target",item.target).put("media",mediaJson(item.media)).put("video",item.video).put("duration",item.duration)
            .put("held",item.held).put("uploaded",item.uploaded).put("missed",item.missed).put("failed",item.failed) }).toString().toByteArray())
    }
    private fun saveIn() {
        check(loaded)
        vault.write("received", JSONArray(incoming.map { item -> JSONObject().put("id",item.id).put("target",item.target)
            .put("sender",item.sender).put("media",mediaJson(item.media)).put("video",item.video).put("duration",item.duration)
            .put("at",item.at).put("ready",item.ready).put("consumed",item.consumed) }).toString().toByteArray())
    }
    suspend fun stage(target: String, bytes: ByteArray, width: Int, height: Int, video: Boolean = false, duration: Int = 0, held: Boolean = true): String {
        load()
        val c = client() ?: error(context.getString(R.string.sign_in_first))
        val version = generation
        val sealed = sealMedia(bytes, width.toUShort(), height.toUShort())
        val thumbnail=mediaThumbnail(bytes,video)
        val item = Outgoing(UUID.randomUUID().toString(), c.userId(), target, sealed.media, video, duration, held)
        gate.withLock {
            check(c === client() && version == generation)
            vault.write("out-${item.id}", sealed.ciphertext)
            val before = outgoing
            outgoing = outgoing + item
            if(thumbnail!=null) vault.write("thumb-${item.id}",thumbnail)
            try { saveOut();publishPreviews() } catch (e: Exception) { outgoing = before; vault.delete("out-${item.id}");vault.delete("thumb-${item.id}");throw e }
            if (held) {
                val items = outgoing.filter { it.target == target && it.held }
                val stack = UndoStack(items.size, SystemClock.elapsedRealtime() + undoMs, undoMs,
                    video, if (video) ListKind.VIDEO else ListKind.PHOTO, undoMs.takeIf { target in paused })
                mediaStacks[target] = stack
                publishStacks()
                scheduleRelease(target, stack)
            }
        }
        wake()
        return item.id
    }
    private fun scheduleRelease(target: String, stack: UndoStack) {
        releases.remove(target)?.cancel()
        if (stack.pausedRemaining != null) return
        releases[target] = scope.launch(Dispatchers.IO) {
            delay(stack.remaining(SystemClock.elapsedRealtime()))
            gate.withLock {
                if (mediaStacks[target] !== stack) return@launch
                val before = outgoing
                outgoing = outgoing.map { if (it.target == target) it.copy(held = false) else it }
                try { saveOut() } catch (e: Exception) { outgoing = before; throw e }
                releases.remove(target)
                mediaStacks.remove(target)
                publishStacks()
            }
            wake()
        }
    }

    suspend fun pause(target: String, expectedSession: Long) = gate.withLock {
        if (session() != expectedSession) return@withLock
        paused.add(target)
        val stack = mediaStacks[target] ?: return@withLock
        val remaining = stack.remaining(SystemClock.elapsedRealtime())
        val waiting = stack.copy(pausedRemaining = remaining)
        mediaStacks[target] = waiting
        publishStacks()
        scheduleRelease(target, waiting)
    }

    suspend fun resume(target: String, expectedSession: Long) = gate.withLock {
        if (session() != expectedSession || !paused.remove(target)) return@withLock
        val stack = mediaStacks[target] ?: return@withLock
        val remaining = stack.pausedRemaining ?: return@withLock
        val waiting = stack.copy(until = SystemClock.elapsedRealtime() + remaining, pausedRemaining = null)
        mediaStacks[target] = waiting
        publishStacks()
        scheduleRelease(target, waiting)
    }

    suspend fun isWaiting(id: String) = gate.withLock { outgoing.any { it.id == id } }
    suspend fun holdYo(friend: Friend) = gate.withLock {
        val target = friend.target
        val count = (yoStacks[target]?.count ?: 0) + 1
        val duration = friend.undoSeconds * 1000L
        val stack = UndoStack(count, SystemClock.elapsedRealtime() + duration, duration)
        yoStacks[target] = stack
        publishStacks()
        yoReleases.remove(target)?.cancel()
        yoReleases[target] = scope.launch(Dispatchers.IO) {
            delay(duration)
            gate.withLock {
                if (yoStacks[target] !== stack) return@launch
                yoStacks.remove(target)
                yoReleases.remove(target)
                publishStacks()
            }
            repeat(count) { sendYo?.invoke(friend.copy(undo = false)) }
        }
    }

    suspend fun cancel(target: String) = gate.withLock {
        val removed = outgoing.filter { it.target == target && it.held }
        if (removed.isNotEmpty()) {
            val before = outgoing
            outgoing = outgoing.filterNot { it.target == target && it.held }
            try { saveOut() } catch (e: Exception) { outgoing = before; throw e }
            removed.forEach { vault.delete("out-${it.id}"); vault.delete("thumb-${it.id}") }
            publishPreviews()
        }
        releases.remove(target)?.cancel()
        yoReleases.remove(target)?.cancel()
        mediaStacks.remove(target)
        yoStacks.remove(target)
        publishStacks()
    }
    /** What did not go to `target` (all, without one) is tried again now, also what the hub refused before. */
    suspend fun retry(target: String? = null) {
        load()
        gate.withLock {
            if (outgoing.none { it.failed && (target == null || it.target == target) }) return@withLock
            outgoing = outgoing.map { if (it.failed && (target == null || it.target == target)) it.copy(failed = false) else it }
            saveOut()
        }
        nudge.trySend(Unit); wake()
    }
    /** A try that failed: kept on disk in any case. One that may go later (no connection, the hub busy) is tried again
     *  with backoff; one the hub refused waits for a tap. The bar says "Not Sent · Tap to Retry", the banner why. */
    private suspend fun missed(item: Outgoing, error: Exception, version: Int) {
        val problem = Problems.problemOf(error).let { if (it == ProductProblem.FAILED) ProductProblem.NOT_SENT else it }
        val passing = productProblemPassing(problem)
        gate.withLock {
            if (version != generation) return@withLock
            outgoing = outgoing.map { if (it.id == item.id) it.copy(missed = true, failed = !passing) else it }
            saveOut(); publishUnsent()
        }
        Problems.report(problem) { scope.launch { retry() } }
    }
    fun wake() {
        if (!flushRequested.compareAndSet(false, true)) return
        scope.launch(Dispatchers.IO) {
            flushGate.lock()
            try {
                flushRequested.set(false)
                load()
                val version = generation
                var pause = 1000L
                while (isActive && version == generation) {
                    val c = client() ?: break
                    // One refused for good waits for a tap; it holds back what came after it to the same target.
                    val items = gate.withLock { outgoing }
                    val pending = gate.withLock { incoming.filter { !it.ready && !it.consumed } }
                    if (items.none { !it.failed && it.owner == c.userId() } && pending.isEmpty()) break
                    var progress = false
                    // Upload independently, but emit messages below in manifest/target order.
                    val slots = Semaphore(3)
                    coroutineScope {
                        items.filter { !it.uploaded && !it.failed && it.owner == c.userId() }.map { item ->
                            async(Dispatchers.IO) { slots.withPermit {
                                if(c !== client() || version != generation) return@withPermit
                                try {
                                    val data=gate.withLock { if(version == generation) vault.read("out-${item.id}") else null } ?: return@withPermit
                                    absorb(c, c.uploadMediaTo(sendTarget(item.target), item.media, data))
                                    gate.withLock {
                                        if(c === client() && version == generation) {
                                            outgoing=outgoing.map { if(it.id==item.id) it.copy(uploaded=true) else it }; saveOut()
                                        }
                                    }
                                } catch(e: CancellationException) { throw e } catch(e: Exception) { missed(item, e, version) }
                            } }
                        }.awaitAll()
                    }
                    val blockedTargets = mutableSetOf<String>()
                    for (item in items) {
                        if (item.target in blockedTargets || c !== client() || generation != version) continue
                        try {
                            if (item.owner != c.userId()) continue
                            val latest = gate.withLock { outgoing.firstOrNull { it.id == item.id } }
                            if (latest == null || latest.held || !latest.uploaded || latest.failed) { blockedTargets += item.target; continue }
                            val events = c.send(sendTarget(item.target),
                                if (item.video) MessageKind.Video(item.media, item.duration.toUInt()) else MessageKind.Photo(item.media))
                            absorb(c, events)
                            val last = gate.withLock {
                                if (generation != version) return@withLock null
                                outgoing = outgoing.filterNot { it.id == item.id }
                                saveOut()
                                vault.delete("out-${item.id}")
                                vault.delete("thumb-${item.id}")
                                publishPreviews(); publishUnsent()
                                outgoing.none { it.target == item.target }
                            }
                            if (last != null && c === client()) sent?.invoke(item.target, last, item.video)
                            progress = true
                        } catch (e: CancellationException) { throw e }
                        catch (e: Exception) { blockedTargets += item.target; missed(item, e, version) }
                    }
                    for (item in pending) {
                        if (c !== client() || version != generation) break
                        try {
                            val bytes = c.fetchMedia(item.media)
                            if (c !== client() || version != generation) break
                            if (bytes != null) {
                                val thumbnail=runCatching { mediaThumbnail(bytes,item.video) }.getOrNull()
                                gate.withLock {
                                    if(c !== client() || version != generation) return@withLock
                                    if(thumbnail!=null) vault.write("in-thumb-${item.id}",thumbnail)
                                    vault.write(receivedFile(item.id),bytes)
                                    incoming = incoming.map { if (it.id == item.id) it.copy(ready=true) else it }; saveIn()
                                }
                            } else gate.withLock { incoming = incoming.filter { it.id != item.id }; saveIn() }
                            downloadChanges.update { it + 1 }; progress = true; changed()
                        } catch (e: CancellationException) { throw e } catch (e: Exception) { android.util.Log.w("YoMedia", "fetch failed", e) }
                    }
                    if (progress) pause=1000 else pause=(pause*2).coerceAtMost(30000)
                    if (withTimeoutOrNull(pause) { nudge.receive() } != null) pause=1000
                }
            } finally { flushGate.unlock() }
        }
    }
    suspend fun expect(id: String, target: String, sender: String, kind: MessageKind, at: Long) {
        val m = when(kind) { is MessageKind.Photo -> kind.media; is MessageKind.Video -> kind.media; else -> return }
        load()
        gate.withLock {
            if (incoming.any { it.id == id }) return@withLock
            val item=Received(id,target,sender,m,kind is MessageKind.Video,(kind as? MessageKind.Video)?.durationMs?.toInt() ?: 0,at)
            val before=incoming; incoming=incoming+item
            try { saveIn() } catch(e: Exception) { incoming=before; throw e }
        }
        changed()
    }
    suspend fun received() = gate.withLock { incoming.filter { !it.consumed }.sortedBy { it.at } }
    private val downloadChanges = MutableStateFlow(0L)
    suspend fun open(item: Received): ByteArray {
        downloadChanges.first {
            gate.withLock { incoming.firstOrNull { it.id == item.id }?.ready != false }
        }
        return gate.withLock {
            val data = vault.read(receivedFile(item.id)) ?: error(context.getString(R.string.photo_unavailable))
            val before = incoming
            incoming = incoming.map { if (it.id == item.id) it.copy(consumed = true) else it }
            try { saveIn() } catch (e: Exception) { incoming = before; throw e }
            data
        }
    }
    suspend fun forget(items: List<Received>) {
        gate.withLock {
            items.forEach { vault.delete(receivedFile(it.id));vault.delete("in-thumb-${it.id}") }
            incoming=incoming.filter { item -> items.none { it.id==item.id } }; saveIn()
        }
        changed()
    }
    suspend fun clear() = gate.withLock {
        generation++
        releases.values.forEach { it.cancel() }; releases.clear(); paused.clear()
        yoReleases.values.forEach { it.cancel() }; yoReleases.clear()
        mediaStacks.clear(); yoStacks.clear()
        outgoing=emptyList(); incoming=emptyList(); loaded=false; stacks.value=emptyMap();previews.value=emptyMap(); downloadChanges.update { it + 1 }
        unsentTargets=emptySet()
    }
    suspend fun stageCard(context: Context, target: String, text: String,users: List<String>,groups: List<String>): String {
        val bitmap=androidx.core.graphics.createBitmap(1080,1080)
        val canvas=Canvas(bitmap); canvas.drawColor(Color.rgb(155,89,182))
        val typeface=androidx.core.content.res.ResourcesCompat.getFont(context,R.font.montserrat_bold) ?: Typeface.DEFAULT_BOLD
        val heading=Paint(Paint.ANTI_ALIAS_FLAG).apply { color=Color.WHITE;textAlign=Paint.Align.CENTER;textSize=96f;this.typeface=typeface }
        canvas.drawText("Yo",540f,70f-heading.fontMetrics.ascent,heading)
        val sentence=android.text.SpannableString(text)
        fun mark(value: String,span: ()->Any) {
            if(value.isEmpty()) return
            var from=0
            while(true) { val start=text.indexOf(value,from);if(start<0) break;sentence.setSpan(span(),start,start+value.length,android.text.Spanned.SPAN_EXCLUSIVE_EXCLUSIVE);from=start+value.length }
        }
        users.distinct().forEach { mark(it) { android.text.style.ForegroundColorSpan(Color.rgb(241,196,15)) } }
        groups.distinct().forEach { mark(it) { android.text.style.UnderlineSpan() } }
        val paint=android.text.TextPaint(Paint.ANTI_ALIAS_FLAG).apply { color=Color.WHITE;textSize=116f;this.typeface=typeface }
        fun layout()=android.text.StaticLayout.Builder.obtain(sentence,0,sentence.length,paint,900)
            .setAlignment(android.text.Layout.Alignment.ALIGN_CENTER).setIncludePad(false).build()
        var layout=layout()
        while((layout.lineCount>5 || layout.height>790) && paint.textSize>35f) { paint.textSize=(paint.textSize*.9f).coerceAtLeast(35f);layout=layout() }
        canvas.withTranslation(90f,200f+(790f-layout.height)/2) { layout.draw(this) }
        val bytes=ByteArrayOutputStream().also { bitmap.compress(Bitmap.CompressFormat.JPEG,90,it) }.toByteArray(); bitmap.recycle()
        return stage(target,bytes,1080,1080,held=false)
    }
    companion object {
        /** What a bar's sign opens: only media downloaded, checked and written whole (`ready`), oldest first. One
         *  still loading stays on its bar, loading; the viewer never waits for a file or takes one in while open. */
        fun openable(media: List<Received>, target: String) =
            media.filter { it.target == target && it.ready && !it.consumed }.sortedBy { it.at }
        /** Who sent the newest photo or video waiting on a bar: a group's bar names them after a restart. */
        fun newestSender(media: List<Received>, target: String) =
            media.filter { it.target == target && !it.consumed }.maxByOrNull { it.at }?.sender
        private fun receivedFile(id: String) = "in-"+id.toByteArray().sha256Hex()
        fun mediaJson(m: YoMedia) = JSONObject().put("id",Repository.encode(m.id)).put("key",Repository.encode(m.key))
            .put("sha",Repository.encode(m.sha256)).put("size",m.size.toLong()).put("width",m.width.toInt()).put("height",m.height.toInt())
        fun mediaFrom(j: JSONObject) = YoMedia(Repository.decode(j.getString("id")),Repository.decode(j.getString("key")),Repository.decode(j.getString("sha")),j.getLong("size").toUInt(),j.getInt("width").toUShort(),j.getInt("height").toUShort())
    }
}

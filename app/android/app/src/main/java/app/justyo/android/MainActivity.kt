package app.justyo.android

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.core.content.IntentCompat
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import android.app.Application
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.Job
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import java.io.File

class YoViewModel(application: Application) : AndroidViewModel(application) {
    val repository = application.repository
    val shares = MutableStateFlow<List<Uri>>(emptyList())
    private val shareSending = kotlinx.coroutines.sync.Mutex()
    val importingShares=MutableStateFlow(false)
    val notificationsEnabled=MutableStateFlow(false)
    var deniedThisLaunch by mutableStateOf(false)
        private set
    fun refreshNotifications() {
        notificationsEnabled.value = getApplication<Application>()
            .getSystemService(android.app.NotificationManager::class.java).areNotificationsEnabled()
    }
    fun notificationResult(granted: Boolean) {
        if (!granted) deniedThisLaunch = true
        refreshNotifications()
    }
    @Volatile private var shareImportVersion=0
    private var importShares: Job?=null
    init {
        repository.crashes.install()
        viewModelScope.launch(Dispatchers.IO) {
            repository.state.first { !it.loading }
            if(importShares==null) runCatching { repository.pendingShares.items() }.getOrNull()?.let { shares.value=it }
        }
    }
    /** yo://camera: the list opens in camera mode (as iOS's Yo Camera). */
    val openCamera = MutableStateFlow(false)
    fun intent(intent: Intent) {
        intent.data?.takeIf { it.scheme?.lowercase() == "yo" && it.host?.lowercase() == "camera" }?.let { openCamera.value = true }
        intent.data?.let { uri -> inviteName(uri)?.let { name -> repository.command { invite(name) } } }
        val streams = when (intent.action) {
            Intent.ACTION_SEND -> listOfNotNull(IntentCompat.getParcelableExtra(intent, Intent.EXTRA_STREAM, Uri::class.java))
            Intent.ACTION_SEND_MULTIPLE -> IntentCompat.getParcelableArrayListExtra(intent, Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
            else -> return
        }
        val sources = buildList {
            addAll(streams)
            intent.clipData?.let { clip ->
                repeat(clip.itemCount) { index -> clip.getItemAt(index).uri?.let(::add) }
            }
            intent.data?.let(::add)
        }.filter { it.scheme == "content" }.distinct()
        if (sources.isEmpty()) return
        val resolver = getApplication<Application>().contentResolver
        val version=++shareImportVersion
        importingShares.value=true
        shares.value=emptyList()
        importShares?.cancel()
        val target=if(android.os.Build.VERSION.SDK_INT>=29) intent.getStringExtra(Intent.EXTRA_SHORTCUT_ID) else null
        importShares=viewModelScope.launch(Dispatchers.IO) {
            try {
                repository.state.first { !it.loading }
                val video = sources.firstOrNull { (resolver.getType(it) ?: intent.type)?.startsWith("video/") == true }
                val items = if (video == null) sources.take(uniffi.yo.productShareLimits().photos.toInt()) else listOf(video)
                repository.pendingShares.clear()
                val staged=mutableListOf<Uri>()
                for(source in items) {
                    val imported=repository.pendingShares.import(source, video != null)
                    staged+=imported
                    if(repository.pendingShares.open(imported).video) break
                }
                if(version!=shareImportVersion) return@launch
                shares.value=staged
                importingShares.value=false
                if(target!=null) repository.state.value.friends.firstOrNull { it.key==target && !it.blocked && !it.gone && !it.request }?.let { friend ->
                    stage(friend,staged)
                }
            } catch(e: CancellationException) { throw e }
            catch(e: Exception) { if(version==shareImportVersion) { error(shareImportError(getApplication(),e));shares.value=runCatching { repository.pendingShares.items() }.getOrDefault(emptyList()) } }
            finally { if(version==shareImportVersion) importingShares.value=false }
        }
    }
    fun cancelShares() {
        shareImportVersion++;importShares?.cancel();shares.value=emptyList();importingShares.value=false
        repository.command { pendingShares.clear() }
    }

    fun stage(friend: Friend, uris: List<Uri>, fromCamera: Boolean = false, session: Long = repository.sessionId): Job {
        val context=getApplication<Application>()
        return repository.command {
            val shared = uris.isNotEmpty() && uris.all { it.scheme == "yo-share" }
            if (shared && !shareSending.tryLock()) return@command
            try {
                if (shared && !pendingShares.items().containsAll(uris)) return@command
                click()
                var sharedVideo = false
                val queued = mutableListOf<String>()
                for (uri in uris) {
                    try {
                        if (sessionId != session) continue
                        val staged = if (shared) pendingShares.open(uri) else null
                        val isVideo = staged?.video ?: (context.contentResolver.getType(uri)?.startsWith("video/") == true ||
                            uri.path?.endsWith(".mp4") == true)
                        val prepared = staged ?: if (isVideo) MediaFiles.video(context, uri, fromCamera = fromCamera) else MediaFiles.photo(context, uri)
                        if (sessionId != session) continue
                        queued += media.stage(friend.target, prepared.bytes, prepared.width, prepared.height, prepared.video,
                            prepared.duration, held = !shared)
                        if (shared) {
                            sharedVideo = isVideo
                            pendingShares.remove(uri)
                        }
                    } finally {
                        if (fromCamera && uri.scheme == "file") uri.path?.let { File(it).delete() }
                    }
                }
                if (shared) {
                    shares.value = pendingShares.items()
                    val sent = kotlinx.coroutines.withTimeoutOrNull(30_000) {
                        for (id in queued) while (sessionId == session && media.isWaiting(id)) kotlinx.coroutines.delay(200)
                        sessionId == session
                    } == true
                    if (sent && queued.isNotEmpty()) {
                        val sign = if (sharedVideo) "🎥" else "📷"
                        val name = if (friend.group == null) friend.name else "#${friend.name.removePrefix("#")}"
                        kotlinx.coroutines.withContext(Dispatchers.Main) {
                            android.widget.Toast.makeText(context, context.getString(R.string.share_confirmation, sign, name),
                                android.widget.Toast.LENGTH_SHORT).show()
                        }
                    }
                }
            } finally {
                if (shared) {
                    shareSending.unlock()
                    if (sessionId == session) shares.value = pendingShares.items()
                }
            }
        }
    }

    /** [text] in the snackbar or the onboarding note: thrown inside a command, which says what failed. `kotlin.error`:
     *  a plain `error(text)` here was this method itself (the outer receiver's member wins over the library function),
     *  so every call (a passkey prompt cancelled, a share that could not be read) started a command that started the
     *  next, without end: three cores busy for as long as the process lived, also cached in the background. */
    fun error(text: String) { repository.command { kotlin.error(text) } }

}
class MainActivity : ComponentActivity() {
    private val model: YoViewModel by viewModels()
    private var perf: PerfProbe? = null
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge(
            statusBarStyle = androidx.activity.SystemBarStyle.dark(android.graphics.Color.TRANSPARENT),
            navigationBarStyle = androidx.activity.SystemBarStyle.dark(android.graphics.Color.TRANSPARENT),
        )
        if((BuildConfig.DEBUG || BuildConfig.APPLICATION_ID.endsWith(".soak")) && intent.getBooleanExtra("yo_perf",false)) { perf=PerfProbe(window); Lat.on=true }
        if(savedInstanceState==null) model.intent(intent)
        setContent { YoTheme { YoRoot(model) } }
    }
    override fun onDestroy() { perf?.close(); super.onDestroy() }
    override fun onNewIntent(intent: Intent) { super.onNewIntent(intent); setIntent(intent); model.intent(intent) }
    override fun onResume() { super.onResume(); model.repository.resumed(true); model.refreshNotifications() }
    override fun onPause() { model.repository.resumed(false); super.onPause() }
    override fun onStart() { super.onStart(); model.repository.active(true) }
    override fun onStop() { model.repository.active(false); super.onStop() }
}

/** A shared item the app may not read (no grant from the sharing app, or gone): the platform's text names the URI. */
internal fun unreadableShare(e: Exception) = e is SecurityException || e is java.io.FileNotFoundException

private fun shareImportError(context: Application, e: Exception) =
    if (unreadableShare(e)) context.getString(R.string.ui_couldn_t_open_this_photo_or_video) else friendlyError(context, e)

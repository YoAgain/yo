package app.justyo.android

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
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
    val repository = Repository(application, viewModelScope)
    val shares = MutableStateFlow<List<Uri>>(emptyList())
    val importingShares=MutableStateFlow(false)
    @Volatile private var shareImportVersion=0
    private var importShares: Job?=null
    init {
        repository.crashes.install(); repository.start()
        viewModelScope.launch(Dispatchers.IO) {
            repository.state.first { !it.loading }
            if(importShares==null) runCatching { repository.pendingShares.items() }.getOrNull()?.let { shares.value=it }
        }
    }
    fun intent(intent: Intent) {
        intent.data?.let { uri -> inviteName(uri)?.let { name -> repository.command { invite(name) } } }
        @Suppress("DEPRECATION")
        val incoming = when(intent.action) {
            Intent.ACTION_SEND -> listOfNotNull(intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM))
            Intent.ACTION_SEND_MULTIPLE -> intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM).orEmpty()
            else -> emptyList()
        }
        val items=incoming.filter { it.scheme=="content" }.take(10)
        if(items.isEmpty()) return
        val version=++shareImportVersion
        importingShares.value=true
        shares.value=emptyList()
        importShares?.cancel()
        val target=if(android.os.Build.VERSION.SDK_INT>=29) intent.getStringExtra(Intent.EXTRA_SHORTCUT_ID) else null
        importShares=viewModelScope.launch(Dispatchers.IO) {
            try {
                repository.state.first { !it.loading }
                repository.pendingShares.clear()
                val staged=mutableListOf<Uri>()
                for(source in items) {
                    val imported=repository.pendingShares.import(source)
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
            catch(e: Exception) { if(version==shareImportVersion) { error(friendlyError(getApplication(),e));shares.value=runCatching { repository.pendingShares.items() }.getOrDefault(emptyList()) } }
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
            click()
            for(uri in uris) {
                if(sessionId != session) {
                    if(fromCamera && uri.scheme=="file") uri.path?.let { File(it).delete() }
                    continue
                }
                val staged=if(uri.scheme=="yo-share") pendingShares.open(uri) else null
                val isVideo=staged?.video ?: (context.contentResolver.getType(uri)?.startsWith("video/") == true || uri.path?.endsWith(".mp4") == true)
                try {
                    val prepared=staged ?: if(isVideo) MediaFiles.video(context,uri) else MediaFiles.photo(context,uri)
                    if(sessionId != session) break
                    if(fromCamera && state.value.saveSent) MediaFiles.keep(context,prepared)
                    media.stage(friend.target,prepared.bytes,prepared.width,prepared.height,prepared.video,prepared.duration)
                    if(staged!=null) pendingShares.remove(uri)
                } finally { if(fromCamera && uri.scheme=="file") uri.path?.let { File(it).delete() } }
                if(isVideo) break
            }
            shares.value=pendingShares.items()
        }
    }
    fun error(text: String) { repository.command { error(text) } }
    override fun onCleared() { repository.active(false);repository.close() }
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
        if(BuildConfig.DEBUG && intent.getBooleanExtra("yo_perf",false)) perf=PerfProbe(window)
        if(savedInstanceState==null) model.intent(intent)
        setContent { YoTheme { YoRoot(model) } }
    }
    override fun onDestroy() { perf?.close(); super.onDestroy() }
    override fun onNewIntent(intent: Intent) { super.onNewIntent(intent); setIntent(intent); model.intent(intent) }
    override fun onStart() { super.onStart(); model.repository.active(true) }
    override fun onStop() { model.repository.active(false); super.onStop() }
}

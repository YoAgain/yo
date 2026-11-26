package app.justyo.android

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.os.SystemClock
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.*
import uniffi.yo.*

internal sealed interface LinkScreen {
    data object Starting : LinkScreen
    class Waiting(val text: String, val deadline: Long) : LinkScreen
    data object Registering : LinkScreen
    class Retry(val message: String) : LinkScreen
}

/** The view model keeps the one-time request and any received bundle across rotation, only in memory. */
internal class DeviceLink(private val scope: CoroutineScope, private val repo: Repository, private val context: Context) {
    var screen by mutableStateOf<LinkScreen?>(null)
        private set
    var error by mutableStateOf<String?>(null)
        private set
    private var job: Job? = null
    private var bundle: YoLinked? = null

    fun close() {
        job?.cancel()
        screen = null
        bundle = null
        error = null
    }

    fun expire() {
        close()
        error = context.getString(R.string.md_expired)
    }

    fun start() {
        if (screen != null) return
        error = null
        repo.clearError()
        screen = LinkScreen.Starting
        job = scope.launch {
            try {
                val request = withContext(Dispatchers.IO) { linkStart(repo.transport) }
                val until = SystemClock.elapsedRealtime() + 600_000L
                screen = LinkScreen.Waiting(request.text, until)
                while (SystemClock.elapsedRealtime() < until) {
                    val linked = try { withContext(Dispatchers.IO) { linkWait(repo.transport, request, 25uL) } }
                    catch (_: YoException.Network) { delay(1_000); continue }
                    if (linked != null) {
                        bundle = linked
                        register()
                        return@launch
                    }
                }
                expire()
            } catch (e: CancellationException) { throw e }
            catch (e: Exception) {
                currentCoroutineContext().ensureActive()
                val waiting = screen is LinkScreen.Waiting
                if (!waiting && e is YoException.Refused && e.status.toInt() == 404) repo.disableDeviceLinks()
                error = if (waiting && e is YoException.Refused && e.status.toInt() == 404)
                    context.getString(R.string.md_expired) else friendlyError(context, e)
                screen = null
            }
        }
    }

    private suspend fun register() {
        val linked = checkNotNull(bundle)
        screen = LinkScreen.Registering
        try {
            withContext(Dispatchers.IO) { repo.link(linked, null) }
            bundle = null
            screen = null
        } catch (e: CancellationException) {
            bundle = null
            screen = null
            throw e
        } catch (e: Exception) {
            if (repo.state.value.username == null) screen = LinkScreen.Retry(friendlyError(context, e))
            else {
                bundle = null
                screen = null
                repo.command { throw e }
            }
        }
    }

    fun retry() {
        if (screen is LinkScreen.Retry) job = scope.launch { register() }
    }
}

@Composable internal fun LinkDevicePage(flow: DeviceLink, page: LinkScreen) {
    val context = LocalContext.current
    val colors = ButtonDefaults.buttonColors(containerColor = Color.White, contentColor = Color.Black)
    val canClose = page != LinkScreen.Registering
    BackHandler { if (canClose) flow.close() }
    Column(Modifier.fillMaxSize().safeDrawingPadding().verticalScroll(rememberScrollState()).padding(24.dp),
        horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically)) {
        when (page) {
            LinkScreen.Starting -> CircularProgressIndicator(color = Color.White)
            is LinkScreen.Waiting -> {
                var showText by remember { mutableStateOf(false) }
                var seconds by remember(page) { mutableLongStateOf(600L) }
                LaunchedEffect(page) {
                    while (true) {
                        seconds = (page.deadline - SystemClock.elapsedRealtime() + 999L) / 1000L
                        if (seconds <= 0) { if (flow.screen === page) flow.expire(); break }
                        delay(500)
                    }
                }
                Text(stringResource(R.string.md_link_this), style = MaterialTheme.typography.headlineMedium)
                Text(stringResource(R.string.md_instructions), textAlign = TextAlign.Center)
                Qr(page.text, Modifier.fillMaxWidth().aspectRatio(1f))
                CircularProgressIndicator(color = Color.White)
                Text(stringResource(R.string.md_waiting, seconds / 60, seconds % 60), textAlign = TextAlign.Center)
                if (showText) Text(page.text, fontFamily = FontFamily.Monospace)
                val copyLabel = stringResource(R.string.md_copy_code)
                Button(onClick = {
                    if (!showText) showText = true else context.getSystemService(ClipboardManager::class.java)
                        .setPrimaryClip(ClipData.newPlainText(copyLabel, page.text))
                }, colors = colors) { Text(stringResource(if (showText) R.string.md_copy_code else R.string.md_show_code)) }
            }
            LinkScreen.Registering -> {
                CircularProgressIndicator(color = Color.White)
                Text(stringResource(R.string.md_linking), style = MaterialTheme.typography.headlineMedium)
                Text(stringResource(R.string.md_getting_account))
            }
            is LinkScreen.Retry -> {
                Text(page.message)
                Button(onClick = flow::retry, colors = colors) { Text(stringResource(R.string.retry)) }
            }
        }
        if (canClose) Button(onClick = flow::close, colors = colors) { Text(stringResource(R.string.ui_cancel)) }
    }
}

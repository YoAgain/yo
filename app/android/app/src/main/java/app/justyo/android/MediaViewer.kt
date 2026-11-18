package app.justyo.android

import android.graphics.ImageDecoder
import android.media.MediaPlayer
import java.nio.ByteBuffer
import android.net.Uri
import android.widget.VideoView
import androidx.activity.compose.PredictiveBackHandler
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.waitForUpOrCancellation
import androidx.compose.foundation.layout.*
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.sp
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.window.SecureFlagPolicy
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.automirrored.filled.VolumeOff
import androidx.compose.material.icons.automirrored.filled.VolumeUp
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.compose.ui.window.DialogWindowProvider
import androidx.core.content.edit
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import kotlinx.coroutines.*
import java.io.File
import java.util.UUID

@Composable fun MediaViewer(repo: Repository,queue: List<Received>,onClose: ()->Unit) {
    val uiResources=androidx.compose.ui.platform.LocalResources.current

    val context=LocalContext.current
    val state by repo.state.collectAsStateWithLifecycle()
    val session=remember { repo.sessionId }
    var index by remember { mutableIntStateOf(0) }
    var bitmap by remember { mutableStateOf<android.graphics.Bitmap?>(null) }
    var video by remember { mutableStateOf<File?>(null) }
    var player by remember { mutableStateOf<VideoView?>(null) }
    val settings = remember { context.getSharedPreferences("viewer", android.content.Context.MODE_PRIVATE) }
    var muted by remember { mutableStateOf(settings.getBoolean("muted", false)) }
    var audioPlayer by remember { mutableStateOf<MediaPlayer?>(null) }
    LaunchedEffect(muted, audioPlayer) {
        val volume = if (muted) 0f else 1f
        audioPlayer?.setVolume(volume, volume)
    }
    var held by remember { mutableStateOf(false) }
    var progress by remember { mutableFloatStateOf(0f) }
    var ready by remember { mutableStateOf(false) }
    var videoRatio by remember { mutableFloatStateOf(0f) }
    val currentHeld by rememberUpdatedState(held)
    val viewed=remember { mutableListOf<Received>() }
    val opened=remember { mutableMapOf<String,ByteArray>() }
    val files=remember { mutableListOf<File>() }
    fun finish(discard: Boolean = false) {
        player?.stopPlayback()
        repo.command { if(sessionId==session) media.forget(if(discard) queue else viewed.toList());files.forEach { it.delete() } }
        onClose()
    }
    DisposableEffect(Unit) { onDispose { player?.stopPlayback();repo.command { if(sessionId==session) media.forget(viewed.toList());files.forEach { it.delete() } } } }
    LaunchedEffect(index) {
        audioPlayer=null;progress=0f;ready=false;bitmap=null;video=null;videoRatio=0f;player?.stopPlayback()
        if(index>=queue.size) { finish();return@LaunchedEffect }
        val item=queue[index]
        try {
            val bytes=withContext(Dispatchers.IO) { opened[item.id] ?: repo.media.open(item).also { opened[item.id]=it } }
            if(item.video) {
                video=withContext(Dispatchers.IO) { File(context.cacheDir,"view-${UUID.randomUUID()}.mp4").also { it.writeBytes(bytes);files+=it } }
            } else {
                bitmap=withContext(Dispatchers.IO) {
                    ImageDecoder.decodeBitmap(ImageDecoder.createSource(ByteBuffer.wrap(bytes))) { decoder, info, _ ->
                        val scale = minOf(1.0, 2048.0 / maxOf(info.size.width, info.size.height))
                        decoder.setTargetSize((info.size.width * scale).toInt().coerceAtLeast(1),
                            (info.size.height * scale).toInt().coerceAtLeast(1))
                        decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
                    }
                }
                ready=true
            }
            if(viewed.none { it.id==item.id }) viewed+=item
        } catch(e: CancellationException) { throw e }
        catch(e: Exception) { repo.command { error(uiResources.getString(R.string.ui_couldn_t_open_this_photo_or_video)) };finish() }
    }
    LaunchedEffect(index,held,ready) {
        if(!ready) return@LaunchedEffect
        if(held) { player?.pause();return@LaunchedEffect }
        val current = index
        val item = queue.getOrNull(current) ?: return@LaunchedEffect
        player?.start()
        val duration=if(item.video) item.duration.coerceIn(1,SharedVideoLimitMs.toInt()) else 3000
        var last=android.os.SystemClock.elapsedRealtime()
        while(progress<1f) { delay(16);val now=android.os.SystemClock.elapsedRealtime();progress+=((now-last)/duration.toFloat());last=now }
        if (index == current) index++
    }
    Dialog(onDismissRequest = { finish() }, properties = DialogProperties(
        usePlatformDefaultWidth = false, decorFitsSystemWindows = false, securePolicy = SecureFlagPolicy.SecureOn,
    )) {
        val decor = LocalView.current
        DisposableEffect(decor) {
            val window = (decor.parent as? DialogWindowProvider)?.window
            window?.let {
                WindowCompat.getInsetsController(it, decor).apply {
                    systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
                    hide(WindowInsetsCompat.Type.systemBars())
                }
            }
            onDispose { }
        }
        PredictiveBackHandler { events -> events.collect { }; finish() }
        BoxWithConstraints(Modifier.fillMaxSize().clipToBounds().background(Color.Black).pointerInput(index) {
            awaitEachGesture {
                val down = awaitFirstDown(requireUnconsumed = false)
                held = true
                val up = try { waitForUpOrCancellation() } finally { held = false }
                if (up != null) {
                    val delta = up.position - down.position
                    if (delta.y > 90 && delta.y > kotlin.math.abs(delta.x)) finish(true)
                    else if (up.uptimeMillis - down.uptimeMillis < 300) {
                        index = if (down.position.x < size.width / 3) (index - 1).coerceAtLeast(0) else index + 1
                    }
                }
            }
        }, contentAlignment = Alignment.Center) {
            bitmap?.let {
                Image(it.asImageBitmap(), uiResources.getString(R.string.ui_photo), Modifier.fillMaxSize(),
                    contentScale = ContentScale.Crop)
            }
            video?.let { file -> key(file.path) {
                val width = if (videoRatio > 0f) maxOf(maxWidth, maxHeight * videoRatio) else maxWidth
                val height = if (videoRatio > 0f) width / videoRatio else maxHeight
                val current = index
                AndroidView(factory = { ctx -> VideoView(ctx).apply {
                    setSecure(true)
                    player = this
                    setVideoURI(Uri.fromFile(file))
                    setOnPreparedListener {
                        val volume = if (muted) 0f else 1f
                        it.setVolume(volume, volume)
                        audioPlayer = it
                        if (it.videoWidth > 0 && it.videoHeight > 0) videoRatio = it.videoWidth.toFloat() / it.videoHeight
                        ready = true
                        if (!currentHeld) start()
                    }
                    setOnErrorListener { _, _, _ ->
                        repo.command { error(uiResources.getString(R.string.ui_couldn_t_open_this_photo_or_video)) }
                        finish()
                        true
                    }
                    setOnCompletionListener { if (index == current) index++ }
                } }, modifier = Modifier.requiredSize(width, height))
            } }
            if (!ready) CircularProgressIndicator(color = Color.White)
            Column(Modifier.align(Alignment.TopCenter).fillMaxWidth()
                .background(Brush.verticalGradient(listOf(Color.Black.copy(alpha = .55f), Color.Transparent)))
                .windowInsetsPadding(WindowInsets.safeDrawing.only(WindowInsetsSides.Top + WindowInsetsSides.Horizontal))
                .padding(horizontal = 8.dp, vertical = 4.dp)) {
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(3.dp)) {
                    queue.forEachIndexed { i, _ ->
                        LinearProgressIndicator(
                            progress = { if (i < index) 1f else if (i == index) progress else 0f },
                            modifier = Modifier.weight(1f).height(3.dp), color = Color.White,
                            trackColor = Color.White.copy(alpha = .35f), gapSize = 0.dp, drawStopIndicator = {},
                        )
                    }
                }
                Row(Modifier.fillMaxWidth().padding(start = 4.dp, top = 10.dp), verticalAlignment = Alignment.Top) {
                    val title = queue.getOrNull(index)?.let { current ->
                        val target = state.friends.firstOrNull { it.target == current.target }
                        val sender = state.friends.firstOrNull { it.user == current.sender }?.name
                            ?: state.listRows[target?.key]?.senders?.firstOrNull { it.user == current.sender }?.name
                        if (target?.group != null) {
                            uiResources.getString(R.string.media_sender_in_group,
                                sender ?: uiResources.getString(R.string.media_unknown_sender), target.name)
                        } else target?.name ?: sender ?: "Yo"
                    }.orEmpty()
                    Text(title, Modifier.weight(1f), color = Color.White, fontFamily = YoFont, fontSize = 17.sp,
                        maxLines = 1, overflow = TextOverflow.Ellipsis)
                    if (queue.getOrNull(index)?.video == true) IconButton(onClick = {
                        muted = !muted
                        settings.edit { putBoolean("muted", muted) }
                    }) {
                        Icon(if (muted) Icons.AutoMirrored.Filled.VolumeOff else Icons.AutoMirrored.Filled.VolumeUp,
                            stringResource(if (muted) R.string.unmute_video else R.string.mute_video), tint = Color.White)
                    }
                    IconButton(onClick = { finish() }) {
                        Icon(Icons.Default.Close, uiResources.getString(R.string.ui_close), tint = Color.White)
                    }
                }
            }
        }
    }
}

package app.justyo.android

import android.app.PendingIntent
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.os.Build
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import android.util.Log
import androidx.annotation.RequiresApi
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.unit.DpSize
import androidx.compose.ui.unit.dp
import androidx.core.content.edit
import androidx.core.net.toUri
import androidx.glance.GlanceId
import androidx.glance.GlanceModifier
import androidx.glance.Image
import androidx.glance.ImageProvider
import androidx.glance.LocalSize
import androidx.glance.action.Action
import androidx.glance.action.ActionParameters
import androidx.glance.action.clickable
import androidx.glance.appwidget.GlanceAppWidget
import androidx.glance.appwidget.GlanceAppWidgetManager
import androidx.glance.appwidget.GlanceAppWidgetReceiver
import androidx.glance.appwidget.SizeMode
import androidx.glance.appwidget.action.ActionCallback
import androidx.glance.appwidget.action.actionRunCallback
import androidx.glance.appwidget.action.actionStartActivity
import androidx.glance.appwidget.provideContent
import androidx.glance.appwidget.updateAll
import androidx.glance.layout.Alignment
import androidx.glance.layout.Column
import androidx.glance.layout.size
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.update
import uniffi.yo.ListKind
import uniffi.yo.ProductText
import uniffi.yo.productDisplayTiming
import uniffi.yo.productNotificationLine

/** Scrolling changes list presentation, but the same contacts and receipts still identify the sender. */
private fun StateFlow<AppState>.senderChanges() = filter { !it.loading }.distinctUntilChanged { old, new ->
    old.username == new.username && old.friends === new.friends && old.lastReceived === new.lastReceived
}

/** An active tile needs a fresh snapshot even when signing in without a last sender. */
internal suspend fun updateSenderWidgets(context: Context, state: StateFlow<AppState>) {
    state.senderChanges().map {
        val sender = it.widgetSender()
        Triple(it.username != null, sender?.key, sender?.widgetName())
    }.distinctUntilChanged().collect {
        widgetHostCall { TileService.requestListeningState(context, ComponentName(context, YoBackTile::class.java)) }
        widgetHostCall { YoBackWidget().updateAll(context) }
        widgetHostCall { YoFromWidget().updateAll(context) }
    }
}

/** A launcher or System UI failure must not end the repository's updates. */
private inline fun widgetHostCall(block: () -> Unit) {
    try {
        block()
    } catch (e: CancellationException) {
        throw e
    } catch (e: Exception) {
        Log.w("YoWidget", "Widget host update failed", e)
    }
}

private suspend fun GlanceAppWidget.provideSender(context: Context, content: @Composable (String?) -> Unit) {
    val state = context.repository.state
    val initial = state.first { !it.loading }.widgetSender()?.widgetName()
    provideContent {
        val name by remember { state.senderChanges().map { it.widgetSender()?.widgetName() }.distinctUntilChanged() }.collectAsState(initial)
        content(name)
    }
}

@Composable
private fun WidgetFace(action: Action?, content: @Composable () -> Unit) {
    val base = widgetBackground()
    Column(if (action == null) base else base.clickable(action),
        horizontalAlignment = Alignment.CenterHorizontally, verticalAlignment = Alignment.CenterVertically) {
        content()
    }
}

class YoBackWidget : GlanceAppWidget() {
    override val sizeMode = SizeMode.Exact
    override val previewSizeMode = SizeMode.Single

    override suspend fun provideGlance(context: Context, id: GlanceId) {
        provideSender(context) { name ->
            val sent by remember(id) { WidgetBack.sent.map { id in it }.distinctUntilChanged() }.collectAsState(false)
            BackFace(context, name, sent, if (name == null) null else actionRunCallback<WidgetBack>())
        }
    }

    override suspend fun providePreview(context: Context, widgetCategory: Int) {
        provideContent { BackFace(context, context.resources.getStringArray(R.array.widget_sample_names).first(), false, null) }
    }
}

@Composable
private fun BackFace(context: Context, name: String?, sent: Boolean, action: Action?) {
    val size = LocalSize.current
    val captionHeight = if (name != null && !sent) minOf(19.dp, size.height / 3) else 0.dp
    val title = when {
        sent -> copy(ProductText.SENT_YO)
        name == null -> copy(ProductText.NO_YO_BACK)
        else -> name
    }
    WidgetFace(action) {
        val height = minOf(40.dp, size.height - captionHeight)
        WidgetWords(context, title, minOf(28f, height.value * 0.8f), size.width, height)
        if (captionHeight > 0.dp) WidgetWords(context, copy(ProductText.YO_BACK), 13f, size.width, captionHeight)
    }
}

/** Each widget keeps its own feedback until the most recent tap's timer expires. */
class WidgetBack : ActionCallback {
    override suspend fun onAction(context: Context, glanceId: GlanceId, parameters: ActionParameters) {
        if (context.repository.state.first { !it.loading }.widgetSender() == null) return
        YoWork.enqueue(context, "send", latest = true)
        val tap = Any()
        sent.update { it + (glanceId to tap) }
        try {
            YoBackWidget().update(context, glanceId)
            delay(productDisplayTiming().widgetSentSecs.toLong() * 1000)
        } finally {
            sent.update { if (it[glanceId] === tap) it - glanceId else it }
            YoBackWidget().update(context, glanceId)
        }
    }

    companion object {
        internal val sent = MutableStateFlow(emptyMap<GlanceId, Any>())
    }
}

class YoFromWidget : GlanceAppWidget() {
    override val sizeMode = SizeMode.Exact
    override val previewSizeMode = SizeMode.Single

    override suspend fun provideGlance(context: Context, id: GlanceId) {
        provideSender(context) { name -> FromFace(context, name, actionStartActivity(Intent(context, MainActivity::class.java))) }
    }

    override suspend fun providePreview(context: Context, widgetCategory: Int) {
        provideContent { FromFace(context, context.resources.getStringArray(R.array.widget_sample_names).first(), null) }
    }
}

@Composable
private fun FromFace(context: Context, name: String?, action: Action?) {
    val size = LocalSize.current
    val line = remember(name) { name?.let { productNotificationLine(it, ListKind.YO, false, Words) } ?: copy(ProductText.YO) }
    WidgetFace(action) { WidgetWords(context, line, 24f, size.width, minOf(size.height, 40.dp)) }
}

private fun cameraIntent(context: Context) = Intent(Intent.ACTION_VIEW, "yo://camera".toUri()).setPackage(context.packageName)

class YoCameraWidget : GlanceAppWidget() {
    override val sizeMode = SizeMode.Exact
    override val previewSizeMode = SizeMode.Responsive(setOf(DpSize(48.dp, 48.dp), DpSize(64.dp, 64.dp)))

    override suspend fun provideGlance(context: Context, id: GlanceId) {
        context.repository.state.first { !it.loading }
        provideContent { CameraFace(context, actionStartActivity(cameraIntent(context))) }
    }

    override suspend fun providePreview(context: Context, widgetCategory: Int) {
        provideContent { CameraFace(context, null) }
    }
}

@Composable
private fun CameraFace(context: Context, action: Action?) {
    val size = LocalSize.current
    val caption = context.getString(R.string.ui_camera)
    val showCaption = size.width >= 64.dp && size.height >= 64.dp
    WidgetFace(action) {
        Image(ImageProvider(R.drawable.ic_widget_camera), if (showCaption) null else caption, GlanceModifier.size(32.dp))
        if (showCaption) WidgetWords(context, caption, 13f, size.width, 20.dp)
    }
}

class YoBackWidgetReceiver : GlanceAppWidgetReceiver() {
    override val glanceAppWidget = YoBackWidget()
}

class YoFromWidgetReceiver : GlanceAppWidgetReceiver() {
    override val glanceAppWidget = YoFromWidget()
}

class YoCameraWidgetReceiver : GlanceAppWidgetReceiver() {
    override val glanceAppWidget = YoCameraWidget()
}

/** Remember each success so a rate-limited receiver gets the next launch's quota. */
@RequiresApi(35)
internal suspend fun publishWidgetPreviews(context: Context) {
    val preferences = context.getSharedPreferences("widget-previews", Context.MODE_PRIVATE)
    val manager = GlanceAppWidgetManager(context)
    for (receiver in listOf(YoWidgetReceiver::class, YoBackWidgetReceiver::class, YoFromWidgetReceiver::class, YoCameraWidgetReceiver::class)) {
        val key = receiver.java.name
        if (preferences.getString(key, null) == BuildConfig.YO_BUILD) continue
        widgetHostCall {
            if (manager.setWidgetPreviews(receiver) == GlanceAppWidgetManager.SET_WIDGET_PREVIEWS_RESULT_SUCCESS) {
                preferences.edit { putString(key, BuildConfig.YO_BUILD) }
            }
        }
    }
}

class YoCameraTile : TileService() {
    override fun onStartListening() {
        super.onStartListening()
        qsTile?.apply {
            state = Tile.STATE_INACTIVE
            updateTile()
        }
    }

    override fun onClick() {
        if (isLocked) unlockAndRun { openCamera() } else openCamera()
    }

    private fun openCamera() {
        val intent = cameraIntent(this).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        startActivityAndCollapse(PendingIntent.getActivity(this, 0, intent, PendingIntent.FLAG_IMMUTABLE))
    }
}

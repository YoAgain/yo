package app.justyo.android

import android.app.Activity
import android.appwidget.AppWidgetManager
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Paint
import android.os.Build
import android.os.Bundle
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.core.content.edit
import androidx.core.content.res.ResourcesCompat
import androidx.glance.GlanceId
import androidx.glance.GlanceModifier
import androidx.glance.Image
import androidx.glance.ImageProvider
import androidx.glance.LocalSize
import androidx.glance.action.ActionParameters
import androidx.glance.action.actionParametersOf
import androidx.glance.action.clickable
import androidx.glance.appwidget.GlanceAppWidget
import androidx.glance.appwidget.GlanceAppWidgetManager
import androidx.glance.appwidget.GlanceAppWidgetReceiver
import androidx.glance.appwidget.SizeMode
import androidx.glance.appwidget.action.ActionCallback
import androidx.glance.appwidget.action.actionRunCallback
import androidx.glance.appwidget.cornerRadius
import androidx.glance.appwidget.provideContent
import androidx.glance.background
import androidx.glance.layout.Alignment as WidgetAlignment
import androidx.glance.layout.Box as WidgetBox
import androidx.glance.layout.Column as WidgetColumn
import androidx.glance.layout.fillMaxSize as widgetFillMaxSize
import androidx.glance.layout.fillMaxWidth as widgetFillMaxWidth
import androidx.glance.layout.height as widgetHeight
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import org.json.JSONArray
import uniffi.yo.ProductText
import uniffi.yo.productWidgetEligible
import uniffi.yo.productWidgetIdSelection

private fun Context.widgetPreferences() = getSharedPreferences("yo-widget", Context.MODE_PRIVATE)
internal fun Friend.widgetName() = if (group == null) name else "#${name.removePrefix("#")}"
/** The core's rule for the eight combinations, asked once: the widget filtered every friend with a core call on each
 *  change of the app's state, on the main thread. */
private val widgetEligible by lazy {
    BooleanArray(8) { productWidgetEligible(it and 1 != 0, it and 2 != 0, it and 4 != 0) }
}
private fun eligible(friend: Friend) =
    widgetEligible[(if (friend.blocked) 1 else 0) or (if (friend.request) 2 else 0) or (if (friend.gone) 4 else 0)]
private fun AppState.widgetFriends() = friends.filter(::eligible)
/** Match the latest-send worker even when the newest sender has since been blocked or removed. */
internal fun AppState.widgetSender(): Friend? {
    if (username == null) return null
    val key = lastReceived.maxByOrNull { it.value }?.key ?: return null
    return friends.firstOrNull { it.key == key && eligible(it) }
}

private const val WidgetSlots = 16

private fun Context.widgetPicks(id: Int, account: String?): List<String> {
    val preferences = widgetPreferences()
    if (account == null || preferences.getString("account:$id", null) != account) return emptyList()
    val keys = JSONArray(preferences.getString("friends:$id", "[]"))
    return List(keys.length()) { keys.getString(it) }
}

/** Text as a bitmap in Montserrat Bold: RemoteViews cannot use the app's font. Scaled down to fit, never cut. */
internal fun Context.wordsBitmap(text: String, size: Float, width: Float, height: Float): Bitmap {
    val density = resources.displayMetrics.density
    val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = android.graphics.Color.WHITE
        typeface = ResourcesCompat.getFont(this@wordsBitmap, R.font.montserrat_bold)
        textSize = size * density
        textAlign = Paint.Align.CENTER
    }
    val room = (width - 32f) * density
    val natural = paint.measureText(text)
    if (natural > room) paint.textSize *= maxOf(0.5f, room / natural)
    val w = (width * density).toInt().coerceAtLeast(1)
    val h = (height * density).toInt().coerceAtLeast(1)
    return androidx.core.graphics.createBitmap(w, h).also { bitmap ->
        val metrics = paint.fontMetrics
        Canvas(bitmap).drawText(text, w / 2f, h / 2f - (metrics.ascent + metrics.descent) / 2, paint)
    }
}

internal fun widgetBackground(): GlanceModifier {
    val base = GlanceModifier.widgetFillMaxSize()
    val rounded = if (Build.VERSION.SDK_INT >= 31) base.cornerRadius(android.R.dimen.system_app_widget_background_radius) else base
    return rounded.background(Amethyst)
}

/** The list widget (iOS YoWidgets): one friend on Amethyst, or rows in their colours sharing the height. A tap sends a Yo. */
class YoWidget : GlanceAppWidget() {
    override val sizeMode = SizeMode.Exact

    override suspend fun provideGlance(context: Context, id: GlanceId) {
        val repo = context.repository
        repo.state.first { !it.loading }
        val widgetId = GlanceAppWidgetManager(context).getAppWidgetId(id)
        val shownAtStart = repo.state.value.widgetFriends()
        provideContent {
            // Only what the widget shows: the app's list changing its state as it scrolls (rows seen, a finger down)
            // redrew every row's bitmap and sent the launcher new RemoteViews, on the main thread, while scrolling.
            val friends by remember { repo.state.map { it.friends to it.username }.distinctUntilChanged().map { (all, _) -> all.filter(::eligible) } }
                .collectAsState(shownAtStart)
            val account = repo.widgetAccount()
            val sent by WidgetSent.bars.collectAsState()
            val size = LocalSize.current
            val slots = (size.height / 44.dp).toInt().coerceIn(1, WidgetSlots)
            val selected = productWidgetIdSelection(friends.map { it.key }, context.widgetPicks(widgetId, account),
                account != null, slots.toUInt(), slots.toUInt()).map { friends[it.toInt()] }
            WidgetColumn(widgetBackground()) {
                if (selected.isEmpty()) {
                    WidgetBox(GlanceModifier.widgetFillMaxSize(), contentAlignment = WidgetAlignment.Center) {
                        WidgetColumn(horizontalAlignment = WidgetAlignment.CenterHorizontally) {
                            WidgetWords(context, "Yo", 34f, size.width, 48.dp)
                            WidgetWords(context, copy(ProductText.WIDGET_EMPTY), 13f, size.width, 24.dp)
                        }
                    }
                }
                val rowHeight = size.height / selected.size.coerceAtLeast(1)
                val font = minOf(36f, rowHeight.value * 0.42f)
                // RemoteViews containers have a ten-child limit: rows go in sections of at most eight.
                selected.chunked(8).forEach { section ->
                    WidgetColumn(GlanceModifier.widgetFillMaxWidth().widgetHeight(rowHeight * section.size)) {
                        section.forEach { friend ->
                            val single = selected.size == 1
                            WidgetBox(GlanceModifier.widgetFillMaxWidth().defaultWeight()
                                .background(if (single) Amethyst else RowColors[Math.floorMod(friend.color, RowColors.size)])
                                .clickable(actionRunCallback<WidgetSend>(actionParametersOf(
                                    WidgetSend.friend to friend.key, WidgetSend.account to account.orEmpty()))),
                                contentAlignment = WidgetAlignment.Center) {
                                WidgetWords(context, if (friend.key in sent) copy(ProductText.SENT_YO) else friend.widgetName(),
                                    if (single) 28f else font, size.width, rowHeight)
                            }
                        }
                    }
                }
            }
        }
    }

    override val previewSizeMode = SizeMode.Single

    override suspend fun providePreview(context: Context, widgetCategory: Int) {
        provideContent {
            val size = LocalSize.current
            val names = context.resources.getStringArray(R.array.widget_sample_names)
            // The picker scales the preview to its own frame: the rows share it by weight, not by a computed height.
            WidgetColumn(widgetBackground()) {
                names.forEachIndexed { index, name ->
                    WidgetBox(GlanceModifier.widgetFillMaxWidth().defaultWeight().background(RowColors[index]),
                        contentAlignment = WidgetAlignment.Center) {
                        WidgetWords(context, name, 20f, size.width, 32.dp)
                    }
                }
            }
        }
    }

    override suspend fun onDelete(context: Context, glanceId: GlanceId) {
        val id = GlanceAppWidgetManager(context).getAppWidgetId(glanceId)
        context.widgetPreferences().edit { remove("friends:$id"); remove("account:$id") }
    }
}

@androidx.compose.runtime.Composable
internal fun WidgetWords(context: Context, text: String, size: Float, width: Dp, height: Dp) {
    val bitmap = remember(text, size, width, height) { context.wordsBitmap(text, size, width.value, height.value) }
    Image(ImageProvider(bitmap), text,
        GlanceModifier.widgetFillMaxWidth().widgetHeight(height))
}

class YoWidgetReceiver : GlanceAppWidgetReceiver() {
    override val glanceAppWidget = YoWidget()
}

/** Bars a widget tap just sent a Yo to: they say "Sent Yo!" for the core's moment, as iOS's widget. */
internal object WidgetSent {
    val bars = kotlinx.coroutines.flow.MutableStateFlow(emptySet<String>())
}

class WidgetSend : ActionCallback {
    override suspend fun onAction(context: Context, glanceId: GlanceId, parameters: ActionParameters) {
        val key = parameters[friend] ?: return
        YoWork.enqueue(context, "send", key, parameters[account])
        WidgetSent.bars.value += key
        YoWidget().update(context, glanceId)
        kotlinx.coroutines.delay(uniffi.yo.productDisplayTiming().widgetSentSecs.toLong() * 1000)
        WidgetSent.bars.value -= key
        YoWidget().update(context, glanceId)
    }

    companion object {
        val friend = ActionParameters.Key<String>("friend")
        val account = ActionParameters.Key<String>("account")
    }
}

/** Choosing the widget's friends, in order: standard top bar, Done, checkable list items. */
class YoWidgetConfiguration : ComponentActivity() {
    @OptIn(ExperimentalMaterial3Api::class)
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val id = intent.getIntExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, AppWidgetManager.INVALID_APPWIDGET_ID)
        val result = Intent().putExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, id)
        setResult(Activity.RESULT_CANCELED, result)
        val glanceId = if (id == AppWidgetManager.INVALID_APPWIDGET_ID) null
            else runCatching { GlanceAppWidgetManager(this).getGlanceIdBy(id) }.getOrNull()
        if (glanceId == null) { finish(); return }
        enableEdgeToEdge()
        val repo = repository
        setContent {
            YoSystemTheme {
                val snapshot by repo.state.collectAsStateWithLifecycle()
                val account = repo.widgetAccount()
                var selected by rememberSaveable(id, account) { mutableStateOf(widgetPicks(id, account)) }
                var saving by remember { mutableStateOf(false) }
                Scaffold(topBar = {
                    TopAppBar(
                        title = { Text(stringResource(R.string.widget_choose)) },
                        actions = {
                            TextButton(enabled = account != null && !saving, onClick = {
                                saving = true
                                widgetPreferences().edit { putString("friends:$id", JSONArray(selected).toString()); putString("account:$id", account) }
                                lifecycleScope.launch {
                                    YoWidget().update(this@YoWidgetConfiguration, glanceId)
                                    setResult(Activity.RESULT_OK, result)
                                    finish()
                                }
                            }) { Text(stringResource(R.string.ui_done)) }
                        },
                    )
                }) { padding ->
                    Column(Modifier.fillMaxSize().padding(padding).verticalScroll(rememberScrollState()).padding(16.dp)) {
                        Section(null, copy(ProductText.WIDGET_LIST_INTENT), *snapshot.widgetFriends().map { friend ->
                            val checked = friend.key in selected
                            Setting(friend.widgetName(), leading = { SettingsAvatar(friend.name, 36) }, checked = checked, checkbox = true,
                                enabled = !saving && (checked || selected.size < WidgetSlots),
                                onChecked = { on -> selected = if (on) selected + friend.key else selected - friend.key })
                        }.toTypedArray())
                    }
                }
            }
        }
    }
}

class YoBackTile : TileService() {
    override fun onStartListening() {
        super.onStartListening()
        val snapshot = applicationContext.repository.state.value
        qsTile?.apply {
            state = if (snapshot.username == null) Tile.STATE_UNAVAILABLE else Tile.STATE_INACTIVE
            if (Build.VERSION.SDK_INT >= 29) subtitle = snapshot.widgetSender()?.widgetName() ?: copy(ProductText.NO_YO_BACK)
            updateTile()
        }
    }

    override fun onClick() {
        val work: () -> Unit = { YoWork.enqueue(this, "send", latest = true) }
        if (isLocked) unlockAndRun { work() } else work()
    }
}

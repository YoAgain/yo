package app.justyo.android

import android.app.Activity
import android.appwidget.AppWidgetManager
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.service.quicksettings.TileService
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.edit
import androidx.glance.GlanceId
import androidx.glance.GlanceModifier
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
import androidx.glance.appwidget.provideContent
import androidx.glance.background
import androidx.glance.layout.Alignment as WidgetAlignment
import androidx.glance.layout.Box as WidgetBox
import androidx.glance.layout.Column as WidgetColumn
import androidx.glance.layout.fillMaxSize as widgetFillMaxSize
import androidx.glance.layout.fillMaxWidth as widgetFillMaxWidth
import androidx.glance.layout.height as widgetHeight
import androidx.glance.text.FontWeight
import androidx.glance.text.Text as WidgetText
import androidx.glance.text.TextStyle
import androidx.glance.unit.ColorProvider
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import org.json.JSONArray

private fun Context.widgetPreferences() = getSharedPreferences("yo-widget", Context.MODE_PRIVATE)
private fun Friend.widgetName() = if (group == null) name else "#${name.removePrefix("#")}"
private fun AppState.widgetFriends() = friends.filter { !it.blocked && !it.request && !it.gone }

private fun Context.widgetSelection(id: Int, account: String?): List<String> {
    val preferences = widgetPreferences()
    if (account == null || preferences.getString("account:$id", null) != account) return emptyList()
    val keys = JSONArray(preferences.getString("friends:$id", "[]"))
    return List(keys.length()) { keys.getString(it) }
}

class YoWidget : GlanceAppWidget() {
    override val sizeMode = SizeMode.Exact

    override suspend fun provideGlance(context: Context, id: GlanceId) {
        val repo = context.repository
        repo.state.first { !it.loading }
        val widgetId = GlanceAppWidgetManager(context).getAppWidgetId(id)
        provideContent {
            val snapshot by repo.state.collectAsState()
            val account = repo.widgetAccount()
            val friends = snapshot.widgetFriends().associateBy { it.key }
            val size = LocalSize.current
            val limit = when {
                size.height >= 360.dp -> 16
                size.height >= 230.dp -> 8
                else -> 4
            }
            val small = size.width < 250.dp && size.height < 180.dp
            val choices = context.widgetSelection(widgetId, account).mapNotNull(friends::get)
            val selected = choices.ifEmpty {
                if (account == null) emptyList() else friends.values.take(if (small) 1 else limit)
            }.take(limit)
            WidgetColumn(GlanceModifier.widgetFillMaxSize().background(Amethyst)) {
                if (selected.isEmpty()) {
                    WidgetBox(GlanceModifier.widgetFillMaxSize(), contentAlignment = WidgetAlignment.Center) {
                        WidgetText("Yo", style = TextStyle(color = ColorProvider(androidx.compose.ui.graphics.Color.White),
                            fontWeight = FontWeight.Bold, fontSize = 28.sp))
                    }
                }
                // RemoteViews containers have a ten-child limit. Group rows without changing their height.
                val sectionSize = if (selected.size > 8) (selected.size + 1) / 2 else 8
                selected.chunked(sectionSize).forEach { section ->
                    WidgetColumn(GlanceModifier.widgetFillMaxWidth().widgetHeight(size.height * section.size / selected.size)) {
                        section.forEach { friend ->
                            val single = small && selected.size == 1
                            WidgetBox(GlanceModifier.widgetFillMaxWidth().defaultWeight()
                                .background(if (single) Amethyst else RowColors[friend.color.mod(RowColors.size)])
                                .clickable(actionRunCallback<WidgetSend>(actionParametersOf(
                                    WidgetSend.friend to friend.key, WidgetSend.account to account.orEmpty(),
                                ))), contentAlignment = WidgetAlignment.Center) {
                                val style = TextStyle(color = ColorProvider(androidx.compose.ui.graphics.Color.White),
                                    fontWeight = FontWeight.Bold, fontSize = if (single) 22.sp else 14.sp)
                                WidgetText(if (single) "Yo · ${friend.widgetName()}" else friend.widgetName(), style = style, maxLines = 1)
                            }
                        }
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

class YoWidgetReceiver : GlanceAppWidgetReceiver() {
    override val glanceAppWidget = YoWidget()
}

class WidgetSend : ActionCallback {
    override suspend fun onAction(context: Context, glanceId: GlanceId, parameters: ActionParameters) {
        val key = parameters[friend] ?: return
        YoWork.enqueue(context, "send", key, parameters[account])
    }

    companion object {
        val friend = ActionParameters.Key<String>("friend")
        val account = ActionParameters.Key<String>("account")
    }
}

class YoWidgetConfiguration : ComponentActivity() {
    @OptIn(ExperimentalMaterial3Api::class)
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val id = intent.getIntExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, AppWidgetManager.INVALID_APPWIDGET_ID)
        val result = Intent().putExtra(AppWidgetManager.EXTRA_APPWIDGET_ID, id)
        setResult(Activity.RESULT_CANCELED, result)
        val glanceId = if (id == AppWidgetManager.INVALID_APPWIDGET_ID) null else try {
            GlanceAppWidgetManager(this).getGlanceIdBy(id)
        } catch (_: IllegalArgumentException) {
            null
        }
        if (glanceId == null) {
            finish()
            return
        }
        enableEdgeToEdge()
        val repo = repository
        setContent {
            YoSystemTheme {
                val snapshot by repo.state.collectAsStateWithLifecycle()
                val account = repo.widgetAccount()
                var selected by rememberSaveable(id, account) { mutableStateOf(widgetSelection(id, account)) }
                var saving by remember { mutableStateOf(false) }
                Scaffold(topBar = {
                    TopAppBar(
                        title = { Text(stringResource(R.string.widget_choose)) },
                        actions = {
                            TextButton(enabled = account != null && !saving, onClick = {
                                saving = true
                                widgetPreferences().edit {
                                    putString("friends:$id", JSONArray(selected).toString())
                                    putString("account:$id", account)
                                }
                                lifecycleScope.launch {
                                    YoWidget().update(this@YoWidgetConfiguration, glanceId)
                                    setResult(Activity.RESULT_OK, result)
                                    finish()
                                }
                            }) { Text(stringResource(R.string.ui_done)) }
                        },
                    )
                }) { padding ->
                    LazyColumn(Modifier.fillMaxSize().padding(padding)) {
                        items(snapshot.widgetFriends(), key = { it.key }) { friend ->
                            val checked = friend.key in selected
                            ListItem(
                                headlineContent = { Text(friend.widgetName()) },
                                leadingContent = { Checkbox(checked, onCheckedChange = null) },
                                modifier = Modifier.toggleable(
                                    value = checked,
                                    enabled = !saving && (checked || selected.size < 16),
                                    role = Role.Checkbox,
                                    onValueChange = { chosen ->
                                        selected = if (chosen) selected + friend.key else selected - friend.key
                                    },
                                ),
                            )
                        }
                    }
                }
            }
        }
    }
}

class YoBackTile : TileService() {
    override fun onClick() {
        val work: () -> Unit = { YoWork.enqueue(this, "send", latest = true) }
        if (isLocked) unlockAndRun { work() } else work()
    }
}

@file:OptIn(androidx.compose.material3.ExperimentalMaterial3ExpressiveApi::class, androidx.compose.material3.ExperimentalMaterial3Api::class)
package app.justyo.android

import android.app.Activity
import android.app.KeyguardManager
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.net.Uri
import androidx.activity.compose.PredictiveBackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedContent
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.relocation.BringIntoViewRequester
import androidx.compose.foundation.relocation.bringIntoViewRequester
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.TextAutoSize
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.Help
import androidx.compose.material.icons.automirrored.filled.Logout
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.net.toUri
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.MultiFormatReader
import com.google.zxing.RGBLuminanceSource
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeWriter
import kotlinx.coroutines.*
import uniffi.yo.*

/** One settings row. The rows of a section are Material 3 Expressive segmented list items. */
internal data class Setting(
    val title: String,
    val icon: ImageVector? = null,
    val value: String? = null,
    val supporting: String? = null,
    val checked: Boolean? = null,
    /** A checkbox instead of a switch: a choice among many. */
    val checkbox: Boolean = false,
    val badge: Boolean = false,
    val good: Boolean? = null,
    val danger: Boolean = false,
    val enabled: Boolean = true,
    val leading: (@Composable () -> Unit)? = null,
    val onChecked: ((Boolean) -> Unit)? = null,
    val onClick: (() -> Unit)? = null,
)

@Composable internal fun Section(title: String? = null, footer: String? = null, vararg rows: Setting?) {
    val shown = rows.filterNotNull()
    run {
        Column(Modifier.fillMaxWidth().padding(bottom = 20.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
            if (title != null) Text(title, Modifier.padding(start = 16.dp, bottom = 6.dp),
                style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
            shown.forEachIndexed { index, row -> SettingRow(row, ListItemDefaults.segmentedShapes(index, shown.size)) }
            if (footer != null) Text(footer, Modifier.padding(horizontal = 16.dp, vertical = 6.dp),
                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@Composable
private fun SettingRow(row: Setting, shapes: ListItemShapes) {
    val leading: (@Composable () -> Unit)? = row.leading ?: row.icon?.let { icon -> { Icon(icon, null) } }
    val hasTrailing = row.checked != null || row.value != null || row.badge || row.good != null
    val trailing: (@Composable () -> Unit)? = if (!hasTrailing) null else { {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            row.value?.let { Text(it, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1) }
            row.good?.let { ok ->
                Icon(if (ok) Icons.Default.CheckCircle else Icons.Default.ErrorOutline, null, Modifier.size(18.dp),
                    tint = if (ok) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.error)
            }
            if (row.badge) Badge { Text("1") }
            if (row.checked != null) { if (row.checkbox) Checkbox(row.checked, onCheckedChange = null, enabled = row.enabled) else Switch(row.checked, onCheckedChange = null, enabled = row.enabled) }
        }
    } }
    val supporting: (@Composable () -> Unit)? = row.supporting?.let { { Text(it) } }
    val colors = ListItemDefaults.segmentedColors(containerColor = MaterialTheme.colorScheme.surfaceBright,
        selectedContainerColor = MaterialTheme.colorScheme.surfaceBright)
    val content: @Composable () -> Unit = {
        Text(row.title, color = if (row.danger) MaterialTheme.colorScheme.error else Color.Unspecified)
    }
    when {
        row.checked != null -> SegmentedListItem(checked = row.checked, onCheckedChange = { row.onChecked?.invoke(it) },
            shapes = shapes, enabled = row.enabled, leadingContent = leading, trailingContent = trailing,
            supportingContent = supporting, colors = colors, content = content)
        row.onClick != null -> SegmentedListItem(onClick = row.onClick, shapes = shapes, enabled = row.enabled,
            leadingContent = leading, trailingContent = trailing, supportingContent = supporting, colors = colors, content = content)
        else -> SegmentedListItem(shapes = shapes, leadingContent = leading, trailingContent = trailing,
            supportingContent = supporting, colors = colors, content = content)
    }
}

@Composable private fun Note(text: String) {
    Text(text, Modifier.fillMaxWidth().padding(32.dp), textAlign = TextAlign.Center,
        style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

private val Pages = mapOf(
    "Account" to "Profile", "Appearance" to "Profile", "Help" to "Profile",
    "Contacts" to "Account", "Blocked" to "Account", "Account Recovery" to "Account", "Media" to "Account",
    "Statistics" to "Account", "Delete Account" to "Account",
    "Contact Us" to "Help", "Status" to "Help", "Licenses" to "Help", "Crash Reports" to "Status",
)

@Composable fun ProfileSheet(model: YoViewModel, s: AppState, onClose: () -> Unit, onAddFriends: () -> Unit) =
    YoSystemTheme { Settings(model, s, onClose, onAddFriends) }

@Composable private fun Settings(model: YoViewModel, s: AppState, onClose: () -> Unit, onAddFriends: () -> Unit) {
    val res = LocalResources.current
    val context = LocalContext.current
    val repo = model.repository
    // The way here: the encryption and contact pages are reached from two places.
    var trail by rememberSaveable { mutableStateOf(listOf("Profile")) }
    val page = trail.last()
    fun open(next: String) { trail = trail + next }
    val back = { if (trail.size > 1) trail = trail.dropLast(1) else onClose() }
    PredictiveBackHandler { events -> events.collect { }; back() }

    var secure by remember { mutableStateOf(false) }
    var code by remember { mutableStateOf(false) }
    var logout by remember { mutableStateOf(false) }
    var renew by remember { mutableStateOf(false) }
    var removePasskey by remember { mutableStateOf(false) }
    var recoveryHelp by remember { mutableStateOf(false) }
    var passkeyRevision by remember { mutableIntStateOf(0) }
    val contacts = remember(s.friends) { s.friends.filter { it.group == null && !it.blocked }.sortedBy { it.name.lowercase() } }
    val auth = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        if (result.resultCode == Activity.RESULT_OK) {
            if (page == "Delete Account") repo.command { deleteAccount(); onClose() } else { repo.command { prepareRecovery() }; secure = true }
        }
    }
    fun authenticate() {
        @Suppress("DEPRECATION")
        val intent = context.getSystemService(KeyguardManager::class.java).createConfirmDeviceCredentialIntent(
            res.getString(if (page == "Delete Account") R.string.ui_delete_account else R.string.ui_recovery_key),
            res.getString(R.string.ui_confirm_it_s_you))
        if (intent != null) auth.launch(intent) else model.error(res.getString(R.string.ui_set_a_screen_lock_in_android_settings_to_continue))
    }
    val passkeyConfigured by produceState(false, s.secured, secure, passkeyRevision, page) {
        if (!secure && s.secured && page in setOf("Account Recovery", "Delete Account")) {
            value = withContext(Dispatchers.IO) { runCatching { repo.hasRecoveryPasskey() }.getOrDefault(false) }
        }
    }
    val encrypted by produceState<Pair<Int, Int>?>(null, s.friends, s.lastSync) {
        value = withContext(Dispatchers.IO) {
            val good = contacts.count { friend ->
                val details = repo.contact(friend)
                friend.user != null && !friend.gone && !friend.keyChanged && details?.contact?.let { !it.gone && !it.unconfirmed && !it.outdated } == true &&
                    details.sessions.all { it.usable && !it.retrying }
            }
            good to contacts.size
        }
    }
    val connected = s.lastSync != null && System.currentTimeMillis() - s.lastSync < 60_000
    val title = when {
        page == "Profile" -> stringResource(R.string.ui_settings)
        page == "Account" -> s.username.orEmpty()
        page.startsWith("contact:") -> s.friends.firstOrNull { it.key == page.removePrefix("contact:") }?.name.orEmpty()
        else -> stringResource(when (page) {
            "Account Recovery" -> R.string.account_recovery
            "Contacts" -> R.string.ui_contacts
            "Appearance" -> R.string.ui_appearance
            "Blocked" -> R.string.ui_blocked
            "Contact Us" -> R.string.ui_yo_team
            "Crash Reports" -> R.string.ui_crash_reports
            "Delete Account" -> R.string.ui_delete_account
            "Encryption" -> R.string.ui_encryption
            "Help" -> R.string.help_and_about
            "Licenses" -> R.string.about_and_licenses
            "Media" -> R.string.ui_media
            "Statistics" -> R.string.ui_statistics
            else -> R.string.ui_status
        })
    }
    var feedback by rememberSaveable { mutableStateOf("") }
    var sendingFeedback by remember { mutableStateOf(false) }
    var support by remember { mutableStateOf<SupportThread?>(null) }

    SystemBars()
    Scaffold(
        modifier = Modifier.fillMaxSize(),
        containerColor = MaterialTheme.colorScheme.surfaceContainer,
        topBar = {
            TopAppBar(
                colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.surfaceContainer),
                title = { Text(title, maxLines = 1) },
                navigationIcon = { IconButton(onClick = back) { Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.ui_back)) } },
            )
        },
        bottomBar = {
            if (page == "Contact Us") SupportComposer(feedback, { feedback = utf8Limit(it, 2000) }, sendingFeedback) {
                val text = feedback.trim()
                sendingFeedback = true
                repo.command {
                    try {
                        feedback(text)
                        val updated = supportThread()
                        withContext(Dispatchers.Main) { feedback = ""; support = updated }
                    } finally { withContext(Dispatchers.Main) { sendingFeedback = false } }
                }
            }
        },
        contentWindowInsets = WindowInsets.safeDrawing,
    ) { padding ->
        AnimatedContent(page, Modifier.padding(padding).consumeWindowInsets(padding), label = "settings") { shown ->
            when {
                shown == "Contact Us" -> SupportPage(model, support) { support = it }
                shown == "Licenses" -> LicensesPage()
                else -> Column(Modifier.fillMaxSize().imePadding().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp)) { when {
                    shown == "Profile" -> {
                        ProfileCard(s, onOpen = { open("Account") }, onCode = { code = true })
                        Section(null, null,
                            Setting(stringResource(R.string.ui_app_icon), leading = { AppIconPreview(s.appIcon, 32, 8) }, onClick = { open("Appearance") }),
                            Setting(stringResource(R.string.ui_left_handed), Icons.Default.PanTool, checked = s.leftHanded,
                                onChecked = { on -> repo.command { settings(left = on) } }),
                        )
                        Section(null, null,
                            Setting(stringResource(R.string.help_and_about), Icons.AutoMirrored.Filled.Help, onClick = { open("Help") }),
                            Setting(stringResource(R.string.ui_add_friends), Icons.Default.PersonAdd, onClick = onAddFriends),
                        )
                    }
                    shown == "Account" -> {
                        Section(null, null,
                            Setting(stringResource(R.string.ui_contacts), Icons.Default.People, contacts.size.toString(), onClick = { open("Contacts") }),
                            Setting(stringResource(R.string.ui_blocked), Icons.Default.Block, s.friends.count { it.blocked }.toString(), onClick = { open("Blocked") }),
                        )
                        Section(null, null,
                            Setting(stringResource(R.string.account_recovery), Icons.Default.Lock, badge = !s.secured, onClick = { open("Account Recovery") }),
                            Setting(stringResource(R.string.ui_sign_out), Icons.AutoMirrored.Filled.Logout, onClick = { logout = true }),
                        )
                        Section(null, null,
                            Setting(stringResource(R.string.ui_media), Icons.Default.PhotoLibrary, onClick = { open("Media") }),
                            Setting(stringResource(R.string.ui_encryption), Icons.Default.EnhancedEncryption,
                                encrypted?.let { res.getString(R.string.part_of_total, it.first, it.second) }, onClick = { open("Encryption") }),
                            Setting(stringResource(R.string.ui_statistics), Icons.Default.BarChart, (s.sent + s.received).toString(), onClick = { open("Statistics") }),
                            Setting(stringResource(R.string.ui_requests_from_strangers), Icons.Default.PersonAddAlt, checked = s.requestsOpen,
                                onChecked = { on -> repo.command { settings(requests = on) } }),
                        )
                        Section(null, null,
                            Setting(stringResource(R.string.ui_delete_account), Icons.Default.DeleteForever, danger = true, onClick = { open("Delete Account") }),
                        )
                    }
                    shown == "Account Recovery" -> {
                        Section(null, stringResource(R.string.account_recovery_explanation),
                            Setting(stringResource(R.string.ui_recovery_key), Icons.Default.Key, checked = s.secured, onChecked = { on ->
                                if (on) { repo.command { prepareRecovery() }; secure = true } else renew = true
                            }),
                            Setting(stringResource(R.string.ui_show_recovery_key), Icons.Default.Visibility, enabled = s.secured, onClick = ::authenticate),
                            Setting(stringResource(R.string.recovery_passkey), Icons.Default.Fingerprint, checked = passkeyConfigured, onChecked = { on ->
                                if (on) { repo.command { prepareRecovery() }; secure = true } else removePasskey = true
                            }),
                        )
                        Section(null, stringResource(R.string.account_backup_footer),
                            Setting(stringResource(R.string.account_backup), Icons.Default.CloudUpload, onClick = { repo.command { saveContacts() } }),
                        )
                    }
                    shown == "Blocked" -> {
                        val blocked = s.friends.filter { it.blocked }
                        if (blocked.isEmpty()) Note(res.getString(R.string.ui_no_blocked_friends))
                        else Section(null, stringResource(R.string.ui_blocked_friends_can_t_send_you_yos_tap_unblock_to_unblock),
                            *blocked.map { friend ->
                                Setting(friend.name, leading = { SettingsAvatar(friend.name, 40) }, value = res.getString(R.string.ui_unblock),
                                    onClick = { repo.command { block(friend, false) } })
                            }.toTypedArray())
                    }
                    shown == "Media" -> Section(null, stringResource(R.string.ui_saves_photos_and_videos_you_capture_to_your_photo_library_on_andr),
                        Setting(stringResource(R.string.ui_keep_sent_photos), Icons.Default.SaveAlt, checked = s.saveSent,
                            onChecked = { on -> repo.command { settings(save = on) } }),
                    )
                    shown == "Appearance" -> {
                        IconChoice(s.appIcon) { name -> repo.command { icon(name) } }
                        Section(null, stringResource(R.string.ui_yo_and_photos_at_the_left_end_of_a_bar_undo_at_the_right_the_came),
                            Setting(stringResource(R.string.ui_left_handed), Icons.Default.PanTool, checked = s.leftHanded,
                                onChecked = { on -> repo.command { settings(left = on) } }),
                        )
                    }
                    shown == "Statistics" -> {
                        val top = s.byFriend.maxByOrNull { it.value }
                        if (top != null) Section(null, null,
                            Setting(stringResource(R.string.top_friend), Icons.Default.Favorite, s.friends.firstOrNull { it.key == top.key }?.name.orEmpty()),
                            Setting(stringResource(R.string.yos_together), Icons.Default.Handshake, top.value.toString()),
                        )
                        Section(null, stringResource(R.string.ui_counted_on_this_phone),
                            Setting(stringResource(R.string.ui_yos_sent), Icons.Default.NorthEast, s.sent.toString()),
                            Setting(stringResource(R.string.ui_yos_received), Icons.Default.SouthWest, s.received.toString()),
                            Setting(stringResource(R.string.ui_friends), Icons.Default.People, s.friends.count { !it.blocked }.toString()),
                        )
                    }
                    shown == "Help" -> {
                        // Yo's own look for content: the tutorial on Amethyst.
                        Surface(Modifier.padding(bottom = 20.dp), shape = MaterialTheme.shapes.extraLarge, color = Amethyst, contentColor = Color.White) {
                            YoTutorial(Modifier.fillMaxWidth().height(330.dp), compact = true)
                        }
                        Section(null, null,
                            Setting(stringResource(R.string.ui_contact_us), Icons.Default.Forum, onClick = { open("Contact Us") }),
                            Setting(stringResource(R.string.ui_status), Icons.Default.CheckCircle,
                                stringResource(if (connected) R.string.ui_connected else R.string.ui_waiting), onClick = { open("Status") }),
                            Setting(stringResource(R.string.about_and_licenses), Icons.Default.Info,
                                stringResource(R.string.about_version, BuildConfig.VERSION_NAME), onClick = { open("Licenses") }),
                        )
                    }
                    shown == "Status" -> StatusPage(model, connected) { open(it) }
                    shown == "Crash Reports" -> {
                        val reports = repo.crashes.history()
                        if (reports.isEmpty()) Note(res.getString(R.string.settings_no_crash_reports))
                        else reports.forEach { text -> Monospace(text) }
                    }
                    shown == "Delete Account" -> DeletePage(model, s, passkeyConfigured, ::authenticate)
                    shown == "Encryption" -> EncryptionPage(model, connected)
                    shown == "Contacts" -> {
                        if (contacts.isEmpty()) Note(res.getString(R.string.ui_no_chat_yet))
                        else Section(null, null, *contacts.map { friend ->
                            Setting(friend.name, leading = { SettingsAvatar(friend.name, 40) },
                                good = when {
                                    friend.gone || friend.keyChanged -> false
                                    else -> null
                                }, onClick = { open("contact:${friend.key}") })
                        }.toTypedArray())
                    }
                    shown.startsWith("contact:") -> s.friends.firstOrNull { it.key == shown.removePrefix("contact:") }?.let { friend ->
                        ContactPage(model, friend)
                    }
                } }
            }
        }
    }

    if (recoveryHelp) AlertDialog(
        onDismissRequest = { recoveryHelp = false },
        title = { Text(stringResource(R.string.how_account_recovery_works)) },
        text = { Text(stringResource(R.string.account_recovery_explanation)) },
        confirmButton = { TextButton(onClick = { recoveryHelp = false }) { Text(stringResource(R.string.ui_done)) } },
    )
    if (secure) RecoverySheet(model, s) { secure = false }
    if (code) ModalBottomSheet(onDismissRequest = { code = false }, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(Modifier.fillMaxWidth().padding(24.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            Text(stringResource(R.string.ui_your_code), style = MaterialTheme.typography.titleLarge)
            Qr("https://justyo.co/${Uri.encode(s.username)}", Modifier.padding(24.dp).size(240.dp))
            Text(s.username.orEmpty(), style = MaterialTheme.typography.headlineSmall.copy(fontFamily = YoFont))
        }
    }
    if (logout) AlertDialog(
        onDismissRequest = { logout = false },
        icon = { Icon(Icons.AutoMirrored.Filled.Logout, null) },
        title = { Text(stringResource(R.string.ui_sign_out_cd2476)) },
        text = { Text(stringResource(if (s.secured) R.string.ui_sign_back_in_with_your_recovery_key_friends_on_this_phone_are_rem
            else R.string.ui_your_account_isn_t_secured_without_a_recovery_key_you_won_t_be_ab)) },
        confirmButton = { TextButton(onClick = { logout = false; repo.command { signOut(byPerson = true); onClose() } }) {
            Text(stringResource(if (s.secured) R.string.ui_sign_out else R.string.ui_sign_out_anyway))
        } },
        dismissButton = { TextButton(onClick = { logout = false }) { Text(stringResource(R.string.ui_cancel)) } },
    )
    if (removePasskey) AlertDialog(
        onDismissRequest = { removePasskey = false },
        title = { Text(stringResource(R.string.settings_turn_off_passkey)) },
        confirmButton = { TextButton(onClick = {
            removePasskey = false
            repo.command { removeRecoveryPasskey(); withContext(Dispatchers.Main) { passkeyRevision++ } }
        }) { Text(stringResource(R.string.settings_turn_off)) } },
        dismissButton = { TextButton(onClick = { removePasskey = false }) { Text(stringResource(R.string.ui_cancel)) } },
    )
    if (renew) AlertDialog(
        onDismissRequest = { renew = false },
        title = { Text(stringResource(R.string.ui_create_new_recovery_key_ccd8c2)) },
        text = { Text(stringResource(R.string.ui_your_current_recovery_key_will_stop_working_after_the_new_on)) },
        confirmButton = { TextButton(onClick = { renew = false; repo.command { prepareRecovery(true) }; secure = true }) {
            Text(stringResource(R.string.ui_create_new_recovery_key))
        } },
        dismissButton = { TextButton(onClick = { renew = false }) { Text(stringResource(R.string.ui_cancel)) } },
    )
}

/** Dark status bar icons over the light settings, given back to the list's white ones on leaving. */
@Composable internal fun SystemBars() {
    val view = androidx.compose.ui.platform.LocalView.current
    val light = !androidx.compose.foundation.isSystemInDarkTheme()
    DisposableEffect(light) {
        val window = (view.context as? Activity)?.window
        val bars = window?.let { androidx.core.view.WindowCompat.getInsetsController(it, view) }
        bars?.isAppearanceLightStatusBars = light; bars?.isAppearanceLightNavigationBars = light
        onDispose { bars?.isAppearanceLightStatusBars = false; bars?.isAppearanceLightNavigationBars = false }
    }
}

/** The account's card: avatar, the name never cut (it shrinks to half, as on iOS), the red 1 while unsecured, the code. */
@Composable private fun ProfileCard(s: AppState, onOpen: () -> Unit, onCode: () -> Unit) {
    Card(onClick = onOpen, modifier = Modifier.fillMaxWidth().padding(bottom = 20.dp), shape = MaterialTheme.shapes.extraLarge,
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceBright)) {
        Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            SettingsAvatar(s.username.orEmpty(), 64)
            BasicText(
                s.username.orEmpty(), Modifier.weight(1f),
                style = TextStyle(fontFamily = YoFont, fontSize = 26.sp, color = MaterialTheme.colorScheme.onSurface),
                maxLines = 1, softWrap = false,
                autoSize = TextAutoSize.StepBased(minFontSize = 13.sp, maxFontSize = 26.sp),
            )
            if (!s.secured) Badge { Text("1") }
            FilledTonalIconButton(onClick = onCode) { Icon(Icons.Default.QrCode, stringResource(R.string.ui_show_your_code)) }
        }
    }
}

@Composable private fun IconChoice(current: String?, choose: (String?) -> Unit) {
    Row(Modifier.fillMaxWidth().padding(bottom = 20.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        listOf(null, "Negative", "Original").forEach { name ->
            val selected = current == name
            OutlinedCard(
                Modifier.weight(1f).selectable(selected, onClick = { choose(name) }),
                border = if (selected) CardDefaults.outlinedCardBorder().copy(width = 2.dp, brush = androidx.compose.ui.graphics.SolidColor(MaterialTheme.colorScheme.primary))
                    else CardDefaults.outlinedCardBorder(),
            ) {
                Column(Modifier.fillMaxWidth().padding(vertical = 12.dp), horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    AppIconPreview(name, 64, 16)
                    Text(stringResource(when (name) { "Negative" -> R.string.icon_negative; "Original" -> R.string.icon_original; else -> R.string.icon_default }),
                        style = MaterialTheme.typography.labelLarge)
                }
            }
        }
    }
}

@Composable private fun AppIconPreview(name: String?, size: Int, radius: Int) {
    val resource = if (name == "Negative") R.drawable.icon_negative else R.drawable.launcher_wordmark
    val background = if (name == "Original") Color(0xFF9A44BE) else Amethyst
    Image(painterResource(resource), null, Modifier.size(size.dp).clip(RoundedCornerShape(radius.dp)).background(background))
}

@Composable private fun StatusPage(model: YoViewModel, connected: Boolean, open: (String) -> Unit) {
    val repo = model.repository
    val res = LocalResources.current
    var trip by remember { mutableStateOf<String?>(null) }
    var probing by remember { mutableStateOf(false) }
    var crashOn by remember { mutableStateOf(repo.crashes.enabled) }
    var granted by remember { mutableStateOf(false) }
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted = it; model.notificationResult(it) }
    val notifications = granted || contextNotificationsEnabled()
    run {
        Section(stringResource(R.string.ui_connection), null,
            Setting(stringResource(R.string.ui_hub), Icons.Default.Cloud, stringResource(if (connected) R.string.ui_connected else R.string.ui_waiting), good = connected),
            Setting(stringResource(R.string.ui_round_trip), Icons.Default.Speed, trip ?: stringResource(R.string.ui_not_checked)),
            Setting(stringResource(if (probing) R.string.ui_checking else R.string.ui_check_round_trip), Icons.Default.Refresh, enabled = !probing, onClick = {
                probing = true
                repo.command {
                    try { trip = roundTrip()?.let { "$it ms" } ?: res.getString(R.string.ui_no_reply) } finally { probing = false }
                }
            }),
        )
        Section(null, stringResource(R.string.ui_anonymous_crash_reports_include_android_version_os_exit_reason_an),
            if (android.os.Build.VERSION.SDK_INT >= 33 && !notifications) Setting(stringResource(R.string.enable_yo_notifications), Icons.Default.Notifications,
                onClick = { permission.launch(android.Manifest.permission.POST_NOTIFICATIONS) }) else null,
            Setting(stringResource(R.string.ui_send_crashes), Icons.Default.BugReport, checked = crashOn, onChecked = {
                crashOn = it; repo.crashes.enabled = it
                if (it) repo.command { crashes.flush() }
            }),
            Setting(stringResource(R.string.ui_crash_reports), Icons.Default.Description, onClick = { open("Crash Reports") }),
        )
        Section(null, null,
            Setting(stringResource(R.string.ui_encryption), Icons.Default.EnhancedEncryption, onClick = { open("Encryption") }),
            Setting(stringResource(R.string.ui_version), Icons.Default.Info, BuildConfig.VERSION_NAME),
        )
    }
}


@Composable private fun Monospace(text: String) {
    SelectionContainer { Text(text, Modifier.padding(vertical = 8.dp), fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall) }
}

@Composable internal fun SettingsAvatar(name: String, diameter: Int) {
    val seed = remember(name) { usernameSkeleton(name).codePoints().sum() }
    val initial = remember(name) { if (name.isEmpty()) "?" else name.substring(0, name.offsetByCodePoints(0, 1)).uppercase() }
    Box(Modifier.size(diameter.dp).background(RowColors[Math.floorMod(seed, RowColors.size)], CircleShape), contentAlignment = Alignment.Center) {
        Text(initial, style = TextStyle(fontFamily = YoFont, fontSize = (diameter * 0.5f).sp, color = Color.White))
    }
}

@Composable private fun SupportPage(model: YoViewModel, support: SupportThread?, loaded: (SupportThread?) -> Unit) {
    val repo = model.repository
    var offline by remember { mutableStateOf(false) }
    var retry by remember { mutableIntStateOf(0) }
    LaunchedEffect(retry) {
        while (true) {
            try {
                loaded(withContext(Dispatchers.IO) { repo.supportThread() }); offline = false
                support?.messages?.lastOrNull()?.let { withContext(Dispatchers.IO) { repo.supportRead(it.id) } }
            } catch (e: CancellationException) { throw e } catch (_: Exception) { offline = true }
            delay(5000)
        }
    }
    val messages = support?.messages.orEmpty()
    if (messages.isEmpty() && !offline) {
        Column(Modifier.fillMaxSize().padding(32.dp), horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically)) {
            Icon(Icons.Default.Forum, null, Modifier.size(48.dp), tint = MaterialTheme.colorScheme.primary)
            Text(stringResource(R.string.support_empty_title), style = MaterialTheme.typography.titleLarge)
            Text(stringResource(R.string.support_empty_subtitle), textAlign = TextAlign.Center,
                style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        return
    }
    LazyColumn(Modifier.fillMaxSize(), reverseLayout = true, contentPadding = PaddingValues(16.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (offline) item {
            Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.errorContainer)) {
                Row(Modifier.padding(start = 16.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(stringResource(R.string.support_offline), Modifier.weight(1f).padding(vertical = 12.dp))
                    TextButton(onClick = { retry++ }) { Text(stringResource(R.string.retry)) }
                }
            }
        }
        items(messages.asReversed(), key = { it.id.toString() }) { message ->
            Box(Modifier.fillMaxWidth(), contentAlignment = if (message.fromTeam) Alignment.CenterStart else Alignment.CenterEnd) {
                Surface(
                    shape = MaterialTheme.shapes.large,
                    color = if (message.fromTeam) MaterialTheme.colorScheme.surfaceContainerHighest else MaterialTheme.colorScheme.primary,
                    contentColor = if (message.fromTeam) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onPrimary,
                ) { Text(message.text, Modifier.padding(horizontal = 16.dp, vertical = 10.dp)) }
            }
        }
    }
}

@Composable private fun SupportComposer(value: String, change: (String) -> Unit, sending: Boolean, send: () -> Unit) {
    Surface(color = MaterialTheme.colorScheme.surfaceContainer) {
        Column(Modifier.fillMaxWidth().navigationBarsPadding().imePadding().padding(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(4.dp)) {
            OutlinedTextField(
                value, change, Modifier.fillMaxWidth(), enabled = !sending, maxLines = 6,
                placeholder = { Text(stringResource(R.string.support_message)) },
                shape = MaterialTheme.shapes.extraLarge,
                trailingIcon = {
                    if (sending) CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                    else IconButton(onClick = send, enabled = value.isNotBlank()) {
                        Icon(Icons.AutoMirrored.Filled.Send, stringResource(R.string.ui_send))
                    }
                },
            )
            Text(stringResource(R.string.support_privacy), Modifier.fillMaxWidth(), textAlign = TextAlign.Center,
                style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

/** Deleting: what happens, the name typed to confirm, then the screen lock. The button stays above the keyboard. */
@Composable private fun DeletePage(model: YoViewModel, s: AppState, passkey: Boolean, authenticate: () -> Unit) {
    val res = LocalResources.current
    var typed by rememberSaveable { mutableStateOf("") }
    val confirmed = typed.isNotEmpty() && usernameSkeleton(typed) == usernameSkeleton(s.username.orEmpty())
    val button = remember { BringIntoViewRequester() }
    val scope = rememberCoroutineScope()
    Column(Modifier.fillMaxWidth().padding(vertical = 16.dp), horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Icon(Icons.Default.Warning, null, Modifier.size(56.dp), tint = MaterialTheme.colorScheme.error)
        Text(stringResource(R.string.settings_delete_your_account), style = MaterialTheme.typography.headlineMedium, textAlign = TextAlign.Center)
    }
    Section(null, null,
        Setting(res.getString(R.string.name_freed, s.username.orEmpty()), Icons.Default.PersonOff),
        Setting(stringResource(R.string.ui_your_friends_can_t_reach_you_anymore), Icons.Default.GroupOff),
        Setting(stringResource(R.string.ui_this_can_t_be_undone), Icons.Default.WarningAmber),
        if (passkey) Setting(stringResource(R.string.settings_delete_passkey_yourself), Icons.Default.Key) else null,
    )
    Column(Modifier.fillMaxWidth().bringIntoViewRequester(button), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        OutlinedTextField(
            typed, { typed = it }, Modifier.fillMaxWidth().onFocusChanged { if (it.isFocused) scope.launch { delay(350); button.bringIntoView() } },
            singleLine = true, label = { Text(res.getString(R.string.type_to_confirm, s.username.orEmpty())) },
        )
        Button(onClick = authenticate, enabled = confirmed, modifier = Modifier.fillMaxWidth(),
            colors = ButtonDefaults.buttonColors(containerColor = MaterialTheme.colorScheme.error, contentColor = MaterialTheme.colorScheme.onError)) {
            Text(stringResource(R.string.ui_delete_account))
        }
        Spacer(Modifier.height(16.dp))
    }
}

@Composable private fun EncryptionPage(model: YoViewModel, connected: Boolean) {
    val res = LocalResources.current
    val status by produceState<SecurityStatus?>(null) { value = withContext(Dispatchers.IO) { model.repository.security() } }
    Text(stringResource(R.string.ui_every_yo_photo_and_video_is_end_to_end_encrypted_only_you_an), Modifier.padding(16.dp, 8.dp, 16.dp, 20.dp),
        style = MaterialTheme.typography.bodyLarge)
    status?.let { st ->
        Section(null, null,
            Setting(stringResource(R.string.ui_hub_connected), Icons.Default.Cloud, stringResource(if (connected) R.string.ui_connected else R.string.ui_waiting), good = connected),
            Setting(stringResource(R.string.ui_hub_signed), Icons.Default.Verified, stringResource(if (st.hubSigned) R.string.ui_yes else R.string.ui_not_checked), good = st.hubSigned),
            Setting(stringResource(R.string.ui_post_quantum), Icons.Default.Shield, res.getString(R.string.part_of_total, st.pqSessions.toInt(), st.devices.toInt())),
            Setting(stringResource(R.string.ui_keys_at_hub), Icons.Default.Key, st.prekeysAtHub.toString()),
            Setting(stringResource(R.string.key_age), Icons.Default.Schedule, st.signedPrekeyDays.toString()),
            Setting(stringResource(R.string.refill_pending), Icons.Default.Autorenew, stringResource(if (st.refillPending) R.string.ui_yes else R.string.ui_no)),
            Setting(stringResource(R.string.ui_verified_contacts), Icons.Default.VerifiedUser, res.getString(R.string.part_of_total, st.verified.toInt(), st.contacts.toInt())),
        )
    }
}

/** A contact (iOS ContactPage): what needs a look, the safety number to compare, technical details, undo before sending. */
@Composable private fun ContactPage(model: YoViewModel, friend: Friend) {
    val repo = model.repository
    val res = LocalResources.current
    var revision by remember { mutableIntStateOf(0) }
    var technical by rememberSaveable { mutableStateOf(false) }
    val details by produceState<ContactDetails?>(null, friend.user, revision) { value = withContext(Dispatchers.IO) { repo.contact(friend) } }
    val state by repo.state.collectAsStateWithLifecycle()
    val current = state.friends.firstOrNull { it.key == friend.key } ?: friend
    val contact = details?.contact
    Section(null, null,
        if (friend.gone || contact?.gone == true) Setting(stringResource(R.string.ui_account), Icons.Default.PersonOff, stringResource(R.string.ui_deleted), good = false) else null,
        if (contact?.outdated == true) Setting("Yo", Icons.Default.Update, copy(ProductText.NEEDS_UPDATE), good = false) else null,
        if (contact?.unconfirmed == true) Setting(stringResource(R.string.ui_confirm_new_key), Icons.Default.Key, danger = true,
            onClick = { repo.command { confirm(friend); revision++ } }) else null,
    )
    details?.safetyNumber?.let { number ->
        val verified = contact?.verified == true
        Card(Modifier.fillMaxWidth().padding(bottom = 20.dp), shape = MaterialTheme.shapes.extraLarge) {
            Column(Modifier.fillMaxWidth().padding(16.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(R.string.compare_code), style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
                SelectionContainer {
                    Text(number.filter(Char::isDigit).chunked(5).chunked(3).joinToString("\n") { it.joinToString(" ") },
                        fontFamily = FontFamily.Monospace, textAlign = TextAlign.Center)
                }
                Text(res.getString(R.string.compare_safety, friend.name), style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center)
                if (verified) OutlinedButton(onClick = { repo.command { verify(friend, false); revision++ } }) { Text(stringResource(R.string.ui_mark_as_not_verified)) }
                else Button(onClick = { repo.command { verify(friend, true); revision++ } }) { Text(stringResource(R.string.ui_mark_as_verified)) }
            }
        }
    }
    Section(null, null,
        Setting(stringResource(R.string.ui_undo_before_sending), Icons.Default.Undo, checked = current.undo,
            onChecked = { on -> repo.command { undoSettings(current, on, current.undoSeconds) } }),
    )
    if (current.undo) Column(Modifier.padding(horizontal = 16.dp).padding(bottom = 20.dp)) {
        Text(res.getQuantityString(R.plurals.undo_seconds, current.undoSeconds, current.undoSeconds))
        Slider(current.undoSeconds.toFloat(), { repo.command { undoSettings(current, true, it.toInt()) } }, valueRange = 2f..10f, steps = 7)
    }
    details?.let { d ->
        Section(null, null,
            Setting(stringResource(R.string.technical_details), Icons.Default.Code,
                value = null, onClick = { technical = !technical }),
        )
        if (technical) {
            Section(null, null,
                Setting(stringResource(R.string.ui_account), Icons.Default.Person, stringResource(if (d.contact?.gone == true) R.string.ui_deleted else R.string.ui_on_yo)),
                Setting(stringResource(R.string.ui_identity_key), Icons.Default.Key, supporting = stringResource(if (d.contact?.unconfirmed == true) R.string.ui_changed_confirm_before_sending else R.string.ui_unchanged)),
                Setting(stringResource(R.string.ui_sessions), Icons.Default.Devices, supporting = d.sessions.joinToString("\n") {
                    (if (it.usable) "✓" else res.getString(R.string.ui_waiting)) + "  " + it.device
                }.ifEmpty { res.getString(R.string.ui_no_chat_yet) }),
                d.accountKey?.let { Setting(stringResource(R.string.ui_account_key), Icons.Default.Fingerprint, supporting = it) },
            )
        }
    }
}

@Composable private fun LicensesPage() {
    val context = LocalContext.current
    val chunks = rememberLicenseChunks(true)
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(16.dp)) {
        item {
            Text(stringResource(R.string.ui_yo_is_mit_licensed_this_app_includes_libsignal_licensed_unde), style = MaterialTheme.typography.bodyLarge)
            Text(stringResource(R.string.ui_montserrat_sil_open_font_license_androidx_kotlin_jna_okhttp_), Modifier.padding(vertical = 16.dp),
                style = MaterialTheme.typography.bodyMedium)
            Section(null, null,
                Setting(stringResource(R.string.ui_source), Icons.Default.Code, onClick = {
                    context.startActivity(Intent(Intent.ACTION_VIEW, "https://github.com/yoagain/yo".toUri()))
                }),
            )
        }
        items(chunks.size, key = { "license-$it" }) { index ->
            Text(chunks[index], fontFamily = FontFamily.Monospace, fontSize = 11.sp, lineHeight = 14.sp)
        }
    }
}

/** Setting up recovery: the key (shown, saved, shared) and a passkey. Standard sheet, list items and buttons. */
@Composable fun RecoverySheet(model: YoViewModel, s: AppState, onClose: () -> Unit) {
    val res = LocalResources.current
    val context = LocalContext.current
    var shown by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var saved by remember { mutableStateOf(false) }
    var sharing by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    fun saveWith(write: suspend (java.io.OutputStream, String) -> Unit) = { uri: Uri? ->
        if (uri != null) model.repository.command {
            busy = true
            try {
                val key = state.value.recovery ?: error(res.getString(R.string.ui_recovery_key_missing))
                context.contentResolver.openOutputStream(uri)?.use { write(it, key) } ?: error(res.getString(R.string.ui_couldn_t_save_the_recovery_key))
                keepRecovery(); saved = true
            } finally { busy = false }
        }
        Unit
    }
    val save = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("text/plain"), saveWith { out, key ->
        out.write(res.getString(R.string.ui_yo_recovery_key_for, s.username.orEmpty(), key.chunked(4).joinToString(" ")).toByteArray())
    })
    val saveImage = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("image/png"), saveWith { out, key ->
        val bitmap = recoveryCard(context, s.username.orEmpty(), key)
        try { check(bitmap.compress(Bitmap.CompressFormat.PNG, 100, out)) } finally { bitmap.recycle() }
    })
    val lifecycle = androidx.lifecycle.compose.LocalLifecycleOwner.current
    DisposableEffect(lifecycle) {
        val observer = androidx.lifecycle.LifecycleEventObserver { _, event -> if (event == androidx.lifecycle.Lifecycle.Event.ON_PAUSE) shown = false }
        lifecycle.lifecycle.addObserver(observer); onDispose { lifecycle.lifecycle.removeObserver(observer) }
    }
    val key = s.recovery
    val ready = key?.length == 64
    ModalBottomSheet(onDismissRequest = { if (!busy) onClose() }, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            Icon(Icons.Default.Key, null, Modifier.size(48.dp), tint = MaterialTheme.colorScheme.primary)
            Text(stringResource(R.string.set_up_account_recovery).replace('\n', ' '), Modifier.padding(vertical = 12.dp),
                style = MaterialTheme.typography.headlineSmall, textAlign = TextAlign.Center)
            Section(null, stringResource(R.string.ui_anyone_with_this_key_can_sign_in_as_you_keep_it_safe_and_don),
                Setting(stringResource(R.string.ui_recovery_key), Icons.Default.Key, enabled = ready && !busy,
                    value = stringResource(if (shown) R.string.recovery_hide else R.string.recovery_show), onClick = { shown = !shown }),
                Setting(stringResource(if (saved) R.string.saved else R.string.ui_save), Icons.Default.SaveAlt, enabled = ready && !busy,
                    good = if (saved) true else null, onClick = { sharing = true }),
                Setting(stringResource(R.string.recovery_passkey), Icons.Default.Fingerprint, enabled = !busy,
                    value = stringResource(R.string.recovery_set_up), onClick = {
                        scope.launch {
                            busy = true
                            try {
                                val (name, user) = model.repository.passkeyAccount() ?: return@launch
                                val answer = Passkeys.create(context, name, user)
                                withContext(Dispatchers.IO) { model.repository.keepPasskey(answer) }
                                saved = true
                            } catch (e: CancellationException) { throw e } catch (e: Exception) {
                                model.error(e.localizedMessage ?: res.getString(R.string.passkey_prf_required))
                            } finally { busy = false }
                        }
                    }),
            )
            if (shown && key != null) OutlinedCard(Modifier.fillMaxWidth().padding(bottom = 20.dp)) {
                Column(Modifier.fillMaxWidth().padding(16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    Qr(key, Modifier.size(140.dp))
                    key.chunked(4).chunked(4).forEach { blocks ->
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceEvenly) {
                            blocks.forEach { Text(it, fontFamily = FontFamily.Monospace, fontSize = 17.sp) }
                        }
                    }
                }
            }
            Button(onClick = onClose, enabled = saved && !busy, modifier = Modifier.fillMaxWidth().padding(bottom = 16.dp)) {
                Text(stringResource(R.string.ui_done))
            }
        }
    }
    if (sharing) AlertDialog(
        onDismissRequest = { sharing = false },
        icon = { Icon(Icons.Default.Key, null) },
        title = { Text(stringResource(R.string.ui_recovery_key)) },
        text = { Text(stringResource(R.string.ui_anyone_with_this_key_can_sign_in_as_you_keep_it_safe_and_don)) },
        confirmButton = { TextButton(onClick = { sharing = false; save.launch(res.getString(R.string.ui_yo_recovery_key_txt)) }) { Text(stringResource(R.string.ui_save)) } },
        dismissButton = {
            Row {
                TextButton(onClick = { sharing = false; saveImage.launch(res.getString(R.string.ui_yo_recovery_key_png)) }) { Text(stringResource(R.string.ui_save_image)) }
                if (key != null && ready) TextButton(onClick = {
                    sharing = false
                    val share = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT,
                        res.getString(R.string.ui_yo_recovery_key_for, s.username.orEmpty(), key.chunked(4).joinToString(" ")))
                    context.startActivity(Intent.createChooser(share, res.getString(R.string.recovery_share)))
                }) { Text(stringResource(R.string.recovery_share)) }
            }
        },
    )
}

fun qrBitmap(text: String,side: Int=512): Bitmap {
    val matrix=QRCodeWriter().encode(text,BarcodeFormat.QR_CODE,side,side,mapOf(com.google.zxing.EncodeHintType.CHARACTER_SET to "ISO-8859-1"))
    val pixels=IntArray(side*side) { i -> if(matrix[i%side,i/side]) android.graphics.Color.BLACK else android.graphics.Color.WHITE }
    return androidx.core.graphics.createBitmap(side,side).apply { setPixels(pixels,0,side,0,0,side,side) }
}
@Composable fun Qr(text: String,modifier: Modifier=Modifier) {
    val uiResources=androidx.compose.ui.platform.LocalResources.current

    val bitmap by produceState<Bitmap?>(null,text) { value=withContext(Dispatchers.IO) { qrBitmap(text) } }
    bitmap?.let { Image(it.asImageBitmap(),uiResources.getString(R.string.ui_qr_code),modifier.background(Color.White).padding(8.dp)) }
}
fun recoveryCard(context: Context,name: String,key: String): Bitmap {
    val bitmap=androidx.core.graphics.createBitmap(1170,1320)
    val canvas=android.graphics.Canvas(bitmap)
    canvas.drawColor(android.graphics.Color.rgb(155,89,182))
    val paint=android.graphics.Paint(android.graphics.Paint.ANTI_ALIAS_FLAG).apply {
        color=android.graphics.Color.WHITE
        typeface=androidx.core.content.res.ResourcesCompat.getFont(context,R.font.montserrat_bold)
        textSize=132f
    }
    canvas.drawText("Yo",90f,190f,paint)
    paint.textSize=66f
    canvas.drawText(name,90f,300f,paint)
    canvas.drawText(context.getString(R.string.ui_recovery_key),90f,470f,paint)
    val qr=qrBitmap(key,330)
    canvas.drawBitmap(qr,750f,270f,null);qr.recycle()
    paint.typeface=android.graphics.Typeface.MONOSPACE;paint.textSize=62f
    key.chunked(4).forEachIndexed { i,block -> canvas.drawText(block,90f+(i%4)*260,710f+(i/4)*100,paint) }
    paint.typeface=android.graphics.Typeface.DEFAULT;paint.textSize=34f
    canvas.drawText(context.getString(R.string.ui_use_this_recovery_key_to_sign_in_to_yo_on_a_new_phone),90f,1170f,paint)
    canvas.drawText(context.getString(R.string.ui_keep_it_safe_and_don_t_share_it),90f,1220f,paint)
    return bitmap
}

suspend fun readQr(context: Context,uri: Uri): String? = withContext(Dispatchers.IO) {
    val bitmap=android.graphics.ImageDecoder.decodeBitmap(android.graphics.ImageDecoder.createSource(context.contentResolver,uri)) { decoder,info,_ ->
        decoder.allocator=android.graphics.ImageDecoder.ALLOCATOR_SOFTWARE
        val ratio=minOf(1.0,2048.0/maxOf(info.size.width,info.size.height));decoder.setTargetSize((info.size.width*ratio).toInt().coerceAtLeast(1),(info.size.height*ratio).toInt().coerceAtLeast(1))
    }
    try { val pixels=IntArray(bitmap.width*bitmap.height);bitmap.getPixels(pixels,0,bitmap.width,0,0,bitmap.width,bitmap.height);runCatching { MultiFormatReader().decode(BinaryBitmap(HybridBinarizer(RGBLuminanceSource(bitmap.width,bitmap.height,pixels)))).text }.getOrNull() }
    finally { bitmap.recycle() }
}
@Composable private fun rememberLicenseChunks(enabled: Boolean): List<String> {
    val context=LocalContext.current
    val chunks by produceState<List<String>>(emptyList(),enabled) {
        value=if(enabled) withContext(Dispatchers.IO) {
            listOf("Yo-LICENSE.txt","Montserrat-OFL.txt","Apache-2.0.txt","Rust-Licenses.txt","Maven-Licenses.txt").flatMap { file ->
                context.assets.open(file).bufferedReader().use { it.readText().chunked(3000) }
            }
        } else emptyList()
    }
    return chunks
}

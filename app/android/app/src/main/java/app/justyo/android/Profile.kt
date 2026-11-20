@file:OptIn(androidx.compose.material3.ExperimentalMaterial3ExpressiveApi::class, androidx.compose.material3.ExperimentalMaterial3Api::class)
package app.justyo.android

import android.app.Activity
import android.app.KeyguardManager
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.net.Uri
import androidx.compose.material.icons.outlined.Info
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedContent
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
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
    /** A second action on the same line, before the switch: its icon and its label. */
    val action: Pair<ImageVector, String>? = null,
    val onAction: (() -> Unit)? = null,
    val onChecked: ((Boolean) -> Unit)? = null,
    val onClick: (() -> Unit)? = null,
)

@Composable internal fun Section(title: String? = null, footer: String? = null, vararg rows: Setting?) {
    val shown = rows.filterNotNull()
    run {
        Column(Modifier.fillMaxWidth().padding(bottom = 20.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
            if (title != null) Text(title, Modifier.padding(start = 16.dp, bottom = 6.dp),
                style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
            shown.forEachIndexed { index, row -> SettingRow(row, ListItemDefaults.segmentedShapes(index, shown.size)) }
            if (footer != null) Text(footer, Modifier.padding(horizontal = 16.dp, vertical = 6.dp),
                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@Composable
private fun SettingRow(row: Setting, shapes: ListItemShapes) {
    val leading: (@Composable () -> Unit)? = row.leading ?: row.icon?.let { icon -> { Icon(icon, null) } }
    val hasTrailing = row.checked != null || row.value != null || row.badge || row.good != null || row.action != null
    val trailing: (@Composable () -> Unit)? = if (!hasTrailing) null else { {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            row.value?.let { Text(it, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1) }
            row.good?.let { ok ->
                Icon(if (ok) Icons.Default.CheckCircle else Icons.Default.ErrorOutline, null, Modifier.size(18.dp),
                    tint = if (ok) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.error)
            }
            if (row.badge) Badge { Text("1") }
            row.action?.let { (icon, label) -> IconButton(onClick = { row.onAction?.invoke() }, enabled = row.enabled) { Icon(icon, label) } }
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

/** A standard modal bottom sheet as high as its content (iOS's fitted detent): no half step, and the navigation bar
 *  below it in the sheet's own colour, not the system's white scrim. A page taller than the screen scrolls. */
@Composable internal fun FittedSheet(
    onDismissRequest: () -> Unit,
    state: SheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
    containerColor: Color = BottomSheetDefaults.ContainerColor,
    contentColor: Color = contentColorFor(containerColor),
    dragHandle: @Composable (() -> Unit)? = { BottomSheetDefaults.DragHandle() },
    /** A fixed height, as a share of the tallest a sheet may be (iOS's detents), instead of the content's own. */
    height: Float? = null,
    content: @Composable ColumnScope.() -> Unit,
) = ModalBottomSheet(
    onDismissRequest = onDismissRequest, sheetState = state, containerColor = containerColor, contentColor = contentColor,
    dragHandle = dragHandle, contentWindowInsets = { WindowInsets.safeDrawing.only(WindowInsetsSides.Bottom) },
) {
    val window = (androidx.compose.ui.platform.LocalView.current.parent as? androidx.compose.ui.window.DialogWindowProvider)?.window
    SideEffect { if (android.os.Build.VERSION.SDK_INT >= 29) window?.isNavigationBarContrastEnforced = false }
    // Never under the status bar: the tallest sheet stops below it (its handle included) and scrolls.
    val density = androidx.compose.ui.platform.LocalDensity.current
    val screen = with(density) { androidx.compose.ui.platform.LocalWindowInfo.current.containerSize.height.toDp() }
    val insets = with(density) { (WindowInsets.statusBars.getTop(this) + WindowInsets.navigationBars.getBottom(this)).toDp() }
    val handle = if (dragHandle != null) 48.dp else 0.dp
    val tallest = (screen - insets - handle).coerceAtLeast(0.dp)
    Column(if (height != null) Modifier.height(tallest * height) else Modifier.heightIn(max = tallest), content = content)
}

/** Settings (iOS SettingsView): a fixed height per page, never measured: half for the pages that fit there (the first
 *  page and App Icon), full for every other; its pages pushed in with a back arrow. */
@Composable fun ProfileSheet(model: YoViewModel, onClose: () -> Unit, onAddFriends: () -> Unit) = YoSystemTheme {
    // The whole state (counters and the last sync included): the list underneath takes it without them.
    val s by model.repository.state.collectAsStateWithLifecycle()
    val sheet = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val scope = rememberCoroutineScope()
    fun hideThen(then: () -> Unit) { scope.launch { sheet.hide() }.invokeOnCompletion { then() } }
    // The way here: a contact's page is reached from Contacts.
    val trail = rememberSaveable { mutableStateOf(listOf("Profile")) }
    FittedSheet(onDismissRequest = onClose, state = sheet, containerColor = MaterialTheme.colorScheme.surfaceContainer,
        height = if (trail.value.last() in HalfPages) 0.5f else 1f) {
        Settings(model, s, trail, onClose = { hideThen(onClose) }, onAddFriends = { hideThen(onAddFriends) })
    }
}

/** The settings pages that fit at half height without scrolling. */
private val HalfPages = setOf("Profile", "Appearance")

@Composable private fun Settings(model: YoViewModel, s: AppState, trailState: MutableState<List<String>>, onClose: () -> Unit,
    onAddFriends: () -> Unit) {
    val res = LocalResources.current
    val context = LocalContext.current
    val repo = model.repository
    var trail by trailState
    val page = trail.last()
    fun open(next: String) { trail = trail + next }
    val back = { if (trail.size > 1) trail = trail.dropLast(1) else onClose() }
    // A pushed page goes back one; on the first page the sheet's own back closes it.
    androidx.activity.compose.BackHandler(enabled = trail.size > 1) { trail = trail.dropLast(1) }

    var secure by remember { mutableStateOf(false) }
    var passkeyPage by remember { mutableStateOf(false) }
    // What the screen lock opens once it confirmed it is you: the recovery key's or the passkey's screen.
    var afterAuth by remember { mutableStateOf<String?>(null) }
    var googleOff by remember { mutableStateOf(false) }
    var code by remember { mutableStateOf(false) }
    var logout by remember { mutableStateOf(false) }
    var showingInfo by remember { mutableStateOf(false) }
    var unblocking by remember { mutableStateOf<Friend?>(null) }
    var blockingNew by remember { mutableStateOf<Friend?>(null) }
    var passkeyRevision by remember { mutableIntStateOf(0) }
    val contacts = remember(s.friends) { s.friends.filter { it.group == null && !it.blocked }.sortedBy { it.name.lowercase() } }
    val auth = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        if (result.resultCode == Activity.RESULT_OK) {
            when {
                page == "Delete Account" -> repo.command { deleteAccount(); onClose() }
                afterAuth == "passkey" -> passkeyPage = true
                else -> { repo.command { prepareRecovery() }; secure = true }
            }
        }
        afterAuth = null
    }
    fun authenticate() {
        @Suppress("DEPRECATION")
        val intent = context.getSystemService(KeyguardManager::class.java).createConfirmDeviceCredentialIntent(
            res.getString(if (page == "Delete Account") R.string.ui_delete_account else R.string.ui_recovery_key),
            res.getString(R.string.ui_confirm_it_s_you))
        if (intent != null) auth.launch(intent) else { afterAuth = null; model.error(res.getString(R.string.ui_set_a_screen_lock_in_android_settings_to_continue)) }
    }
    val passkeyConfigured by produceState(false, s.secured, passkeyPage, passkeyRevision, page) {
        if (!passkeyPage && s.secured && page in setOf("Account Security", "Delete Account")) {
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
        page == "Account" -> s.username.orEmpty()
        page.startsWith("contact:") -> s.friends.firstOrNull { it.key == page.removePrefix("contact:") }?.name.orEmpty()
        else -> stringResource(when (page) {
            "Contacts" -> R.string.ui_contacts
            "Account Security" -> R.string.account_security
            "Appearance" -> R.string.ui_app_icon
            "Blocked" -> R.string.ui_blocked
            "Block Contact" -> R.string.block_contact
            "Contact Us" -> R.string.ui_yo_team
            "Crash Reports" -> R.string.ui_crash_reports
            "Delete Account" -> R.string.ui_delete_account
            "Encryption" -> R.string.ui_encryption
            "Help" -> R.string.help_and_about
            "Licenses" -> R.string.about_and_licenses
            "Profile" -> R.string.ui_settings
            else -> R.string.ui_status
        })
    }
    // What a page's rows mean, behind its (i) instead of under every row (iOS: "How Account Recovery Works").
    val info: String? = when (page) {
        "Account Security" -> listOf(
            R.string.account_recovery_explanation,
            when { s.backedUp -> R.string.google_backup_footer
                s.googleBackup && s.backupMissingReason == CloudMissing.DEVICE_BACKUP -> R.string.google_backup_off_device_backup
                s.googleBackup -> R.string.google_backup_off
                else -> R.string.google_backup_turned_off },
            R.string.account_backup_footer,
        ).joinToString("\n\n") { res.getString(it) }
        "Blocked" -> res.getString(R.string.ui_blocked_friends_can_t_send_you_yos_tap_unblock_to_unblock)
        "Appearance" -> res.getString(R.string.ui_yo_and_photos_at_the_left_end_of_a_bar_undo_at_the_right_the_came)
        "Status" -> res.getString(R.string.ui_anonymous_crash_reports_include_android_version_os_exit_reason_an)
        "Encryption" -> res.getString(R.string.ui_every_yo_photo_and_video_is_end_to_end_encrypted_only_you_an)
        else -> if (page.startsWith("contact:")) res.getString(R.string.compare_safety, title) else null
    }
    var feedback by rememberSaveable { mutableStateOf("") }
    var sendingFeedback by remember { mutableStateOf(false) }
    var support by remember { mutableStateOf<SupportThread?>(null) }

    Column(Modifier.fillMaxWidth()) {
        // The first page has no bar (iOS): its rows start at the sheet's top.
        if (page != "Profile") TopAppBar(
            title = { Text(title, maxLines = 1) },
            navigationIcon = { IconButton(onClick = back) { Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.ui_back)) } },
            actions = {
                if (info != null) IconButton(onClick = { showingInfo = true }) {
                    Icon(Icons.Outlined.Info, stringResource(R.string.settings_page_info))
                }
            },
            colors = TopAppBarDefaults.topAppBarColors(containerColor = Color.Transparent),
            windowInsets = WindowInsets(0),
        )
        AnimatedContent(page, label = "settings") { shown ->
            when {
                // The chat and the licences take the whole height, as on iOS.
                shown == "Contact Us" -> Column(Modifier.fillMaxSize()) {
                    Box(Modifier.weight(1f)) { SupportPage(model, support) { support = it } }
                    SupportComposer(feedback, { feedback = utf8Limit(it, 2000) }, sendingFeedback) {
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
                }
                shown == "Licenses" -> LicensesPage()
                // At half height the first page never scrolls: the profile card takes the room the rows leave, so the
                // gap under the last section is the one at the top. Accessibility font sizes keep the compact card and scroll.
                shown == "Profile" && androidx.compose.ui.platform.LocalDensity.current.fontScale < 1.5f -> Column(Modifier.fillMaxSize().padding(horizontal = 16.dp).padding(top = 20.dp)) {
                    ProfileCard(s, onOpen = { open("Account") }, onCode = { code = true }, Modifier.weight(1f), grow = true)
                    ProfileRows(s, repo, ::open, onAddFriends)
                }
                else -> Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).padding(top = 8.dp)) { when {
                    shown == "Profile" -> {
                        ProfileCard(s, onOpen = { open("Account") }, onCode = { code = true })
                        ProfileRows(s, repo, ::open, onAddFriends)
                    }
                    // iOS AccountPage: who, then Account Security and leaving; deleting is a link under them (App Review 5.1.1(v)).
                    shown == "Account" -> {
                        Section(null, null,
                            Setting(stringResource(R.string.ui_contacts), Icons.Default.People,
                                encrypted?.let { res.getString(R.string.part_of_total, it.first, it.second) }, onClick = { open("Contacts") }),
                            Setting(stringResource(R.string.ui_blocked), Icons.Default.Block, s.friends.count { it.blocked }.toString(), onClick = { open("Blocked") }),
                        )
                        // Backup and the ways back on a new phone on a page of their own; the row says whether they are in place.
                        Section(null, null,
                            Setting(stringResource(R.string.account_security), Icons.Default.Shield,
                                badge = s.recoveryNeedsAttention, good = if (s.recoveryNeedsAttention) null else true,
                                onClick = { open("Account Security") }),
                        )
                        Section(null, null,
                            Setting(stringResource(R.string.ui_sign_out), Icons.AutoMirrored.Filled.Logout, onClick = { logout = true }),
                        )
                        TextButton(onClick = { open("Delete Account") }, Modifier.padding(start = 4.dp, bottom = 12.dp)) {
                            Text(stringResource(R.string.settings_leaving_yo), color = MaterialTheme.colorScheme.onSurfaceVariant,
                                style = MaterialTheme.typography.bodyMedium)
                            Text(" " + stringResource(R.string.settings_delete_your_account), style = MaterialTheme.typography.bodyMedium)
                        }
                    }
                    // iOS AccountSecurityPage: Yo's own backup, then the three ways back on a new phone.
                    shown == "Account Security" -> {
                        // Yo's own end-to-end encrypted copy of the contacts, groups and settings, on its own.
                        Section(null, null,
                            Setting(stringResource(R.string.account_backup), Icons.Default.CloudUpload, onClick = { repo.command { saveContacts() } }),
                        )
                        // iOS AccountPage: the three ways back on a new phone, each with its state; the (i) above explains them.
                        // A passkey here rides on a saved recovery key, so without one its row opens the key's screen.
                        Section(copy(ProductText.ACCOUNT_RECOVERY), copy(ProductText.RECOVERY_FOOTER),
                            Setting(copy(ProductText.RECOVERY_KEY), Icons.Default.Key,
                                copy(if (s.secured && s.recoverySaved) ProductText.SAVED else ProductText.NOT_SAVED), badge = s.recoveryNeedsAttention, onClick = {
                                    if (s.secured) { afterAuth = "key"; authenticate() } else { repo.command { prepareRecovery() }; secure = true }
                                }),
                            Setting(copy(ProductText.PASSKEY), Icons.Default.Fingerprint,
                                copy(if (passkeyConfigured) ProductText.ON else ProductText.OFF), onClick = {
                                    if (s.secured) { afterAuth = "passkey"; authenticate() } else { repo.command { prepareRecovery() }; secure = true }
                                }),
                            // Spec "Key material only in an end-to-end encrypted cloud": on only with a screen lock and Google's backup on.
                            Setting(stringResource(R.string.google_backup_row), Icons.Default.Cloud, when {
                                s.backedUp -> copy(ProductText.ON)
                                !s.googleBackup -> copy(ProductText.OFF)
                                s.backupMissingReason == CloudMissing.DEVICE_BACKUP -> stringResource(R.string.google_backup_turn_on_device_backup)
                                else -> stringResource(R.string.google_backup_needs_screen_lock_short)
                            }, badge = s.backupMissing, onClick = {
                                when {
                                    s.backedUp -> googleOff = true
                                    !s.googleBackup -> repo.command { googleBackup(true) }
                                    else -> openSystemSettings(context,
                                        if (s.backupMissingReason == CloudMissing.DEVICE_BACKUP) "android.settings.BACKUP_AND_RESET_SETTINGS"
                                        else android.provider.Settings.ACTION_SECURITY_SETTINGS)
                                }
                            }),
                        )
                    }
                    // iOS BlockedPage: who may ask and blocking one more, then the blocked contacts.
                    shown == "Blocked" -> {
                        Section(null, null,
                            Setting(stringResource(R.string.allow_yo_requests), Icons.Default.PersonAddAlt, checked = s.requestsOpen,
                                onChecked = { on -> repo.command { settings(requests = on) } }),
                            Setting(stringResource(R.string.block_contact_more), Icons.Default.Block, onClick = { open("Block Contact") }),
                        )
                        val blocked = s.friends.filter { it.blocked }
                        Section(stringResource(R.string.blocked_contacts), null,
                            *(if (blocked.isEmpty()) listOf(Setting(stringResource(R.string.no_blocked_contacts)))
                            else blocked.map { friend ->
                                Setting(friend.name, leading = { SettingsAvatar(friend.name, 40) }, value = res.getString(R.string.ui_unblock),
                                    onClick = { unblocking = friend })
                            }).toTypedArray())
                    }
                    shown == "Block Contact" -> Section(null, null,
                        *(if (contacts.isEmpty()) listOf(Setting(stringResource(R.string.no_contacts)))
                        else contacts.map { friend ->
                            Setting(friend.name, leading = { SettingsAvatar(friend.name, 40) }, onClick = { blockingNew = friend })
                        }).toTypedArray())
                    shown == "Appearance" -> {
                        IconChoice(s.appIcon) { name -> repo.command { icon(name) } }
                        Section(null, null,
                            Setting(stringResource(R.string.ui_left_handed), Icons.Default.PanTool, checked = s.leftHanded,
                                onChecked = { on -> repo.command { settings(left = on) } }),
                        )
                    }
                    // iOS HelpPage: the tutorial, then Contact Us, then Status and About & Licenses with the version.
                    shown == "Help" -> {
                        HelpHero(Modifier.padding(bottom = 20.dp))
                        Section(null, null,
                            Setting(stringResource(R.string.ui_contact_us), Icons.Default.Forum, onClick = { open("Contact Us") }),
                        )
                        Section(null, null,
                            Setting(stringResource(R.string.ui_status), Icons.Default.MonitorHeart,
                                stringResource(if (connected) R.string.ui_connected else R.string.ui_waiting), good = connected, onClick = { open("Status") }),
                            Setting(stringResource(R.string.about_and_licenses), Icons.Default.Info, BuildConfig.VERSION_NAME, onClick = { open("Licenses") }),
                        )
                    }
                    shown == "Status" -> StatusPage(model, s, connected) { open(it) }
                    shown == "Crash Reports" -> {
                        val reports = repo.crashes.history()
                        if (reports.isEmpty()) Note(res.getString(R.string.settings_no_crash_reports))
                        else reports.forEach { text -> Monospace(text) }
                    }
                    shown == "Delete Account" -> DeletePage(model, s, passkeyConfigured, ::authenticate)
                    shown == "Encryption" -> EncryptionPage(model, connected)
                    // iOS ContactsList: the contacts, then the groups (their members are on the list's own sheet).
                    shown == "Contacts" -> {
                        if (contacts.isEmpty()) Note(res.getString(R.string.no_contacts_yet))
                        else Section(null, null, *contacts.map { friend ->
                            Setting(friend.name, leading = { SettingsAvatar(friend.name, 40) },
                                good = when {
                                    friend.gone || friend.keyChanged -> false
                                    else -> null
                                }, onClick = { open("contact:${friend.key}") })
                        }.toTypedArray())
                        val groups = s.friends.filter { it.group != null }.sortedBy { it.name.lowercase() }
                        if (groups.isNotEmpty()) Section(stringResource(R.string.contacts_groups), null, *groups.map { group ->
                            Setting(group.name, leading = {
                                Box(Modifier.size(40.dp).background(RowColors[Math.floorMod(group.color, RowColors.size)], CircleShape), contentAlignment = Alignment.Center) {
                                    Icon(Icons.Default.Groups, null, Modifier.size(22.dp), tint = Color.White)
                                }
                            })
                        }.toTypedArray())
                    }
                    shown.startsWith("contact:") -> s.friends.firstOrNull { it.key == shown.removePrefix("contact:") }?.let { friend ->
                        ContactPage(model, friend)
                    }
                } }
            }
        }
    }

    if (secure) RecoverySheet(model, s, passkeyConfigured) { secure = false }
    if (passkeyPage) PasskeySheet(model, passkeyConfigured, onTurnedOff = { passkeyRevision++ }) { passkeyPage = false; passkeyRevision++ }
    if (googleOff) AlertDialog(
        onDismissRequest = { googleOff = false },
        title = { Text(stringResource(R.string.google_backup_turn_off)) },
        text = { Text(stringResource(R.string.google_backup_turned_off)) },
        confirmButton = { TextButton(onClick = { googleOff = false; repo.command { googleBackup(false) } },
            colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error)) {
            Text(stringResource(R.string.settings_turn_off))
        } },
        dismissButton = { TextButton(onClick = { googleOff = false }) { Text(stringResource(R.string.ui_cancel)) } },
    )
    if (code) YourCodeSheet(s.username.orEmpty()) { code = false }
    if (showingInfo && info != null) AlertDialog(
        onDismissRequest = { showingInfo = false },
        icon = { Icon(Icons.Outlined.Info, null) },
        title = { Text(title) },
        text = { Text(info) },
        confirmButton = { TextButton(onClick = { showingInfo = false }) { Text(stringResource(R.string.ui_done)) } },
    )
    blockingNew?.let { friend ->
        AlertDialog(
            onDismissRequest = { blockingNew = null },
            title = { Text(res.getString(R.string.block_name, friend.name)) },
            text = { Text(stringResource(R.string.block_their_yos)) },
            confirmButton = { TextButton(onClick = { blockingNew = null; repo.command { block(friend, true) } },
                colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error)) { Text(stringResource(R.string.ui_block)) } },
            dismissButton = { TextButton(onClick = { blockingNew = null }) { Text(stringResource(R.string.ui_cancel)) } },
        )
    }
    unblocking?.let { friend ->
        AlertDialog(
            onDismissRequest = { unblocking = null },
            title = { Text(res.getString(R.string.unblock_name, friend.name)) },
            confirmButton = { TextButton(onClick = { unblocking = null; repo.command { block(friend, false) } }) { Text(stringResource(R.string.ui_unblock)) } },
            dismissButton = { TextButton(onClick = { unblocking = null }) { Text(stringResource(R.string.ui_cancel)) } },
        )
    }
    // iOS: a secured account goes without a word; an unsecured one is told what it loses.
    if (logout) AlertDialog(
        onDismissRequest = { logout = false },
        icon = { Icon(Icons.AutoMirrored.Filled.Logout, null) },
        title = { Text(stringResource(R.string.ui_sign_out_cd2476)) },
        text = if (s.secured) null else { { Text(stringResource(R.string.sign_out_unsecured)) } },
        confirmButton = { TextButton(onClick = { logout = false; repo.command { signOut(byPerson = true); onClose() } },
            colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error)) {
            Text(stringResource(R.string.ui_sign_out))
        } },
        dismissButton = { TextButton(onClick = { logout = false }) { Text(stringResource(R.string.ui_cancel)) } },
    )
}

/** Your code (iOS YourCodePage): Amethyst, "X wants to Yo", the QR code, the link, and sharing it in the bar. */
@Composable private fun YourCodeSheet(name: String, onClose: () -> Unit) {
    val context = LocalContext.current
    val link = "https://justyo.co/${Uri.encode(name)}"
    FittedSheet(onDismissRequest = onClose, containerColor = Amethyst, contentColor = Color.White) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp).padding(bottom = 24.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            Box(Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.ui_your_code), Modifier.align(Alignment.Center), style = MaterialTheme.typography.titleMedium)
                IconButton(onClick = { shareCode(context, name) }, Modifier.align(Alignment.CenterEnd)) {
                    Icon(Icons.Default.Share, stringResource(R.string.share_your_code))
                }
            }
            BasicText(stringResource(R.string.code_wants_to_yo, name), Modifier.padding(top = 12.dp), maxLines = 1, softWrap = false,
                style = TextStyle(fontFamily = YoFont, fontSize = 28.sp, color = Color.White, textAlign = TextAlign.Center),
                autoSize = TextAutoSize.StepBased(minFontSize = 9.sp, maxFontSize = 28.sp))
            Qr(link, Modifier.padding(vertical = 24.dp).size(260.dp))
            BasicText(link, maxLines = 1, softWrap = false, style = TextStyle(fontFamily = YoFont, fontSize = 28.sp, color = Color.White),
                autoSize = TextAutoSize.StepBased(minFontSize = 6.sp, maxFontSize = 28.sp))
        }
    }
}

/** The invite as iOS shares it: "Yo me on Yo: <link>". */
private fun shareCode(context: Context, name: String) {
    val send = Intent(Intent.ACTION_SEND).setType("text/plain")
        .putExtra(Intent.EXTRA_TEXT, context.getString(R.string.code_share_text, "https://justyo.co/${Uri.encode(name)}"))
        .putExtra(Intent.EXTRA_TITLE, context.getString(R.string.code_wants_to_yo, name))
    context.startActivity(Intent.createChooser(send, context.getString(R.string.share_your_code)))
}

@Composable private fun KeyCard(key: String, copied: Boolean? = null, onCopy: () -> Unit = {}) {
    OutlinedCard(Modifier.fillMaxWidth()) {
        Column(Modifier.fillMaxWidth().padding(16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            Qr(key, Modifier.size(140.dp))
            SelectionContainer {
                Column {
                    key.chunked(4).chunked(4).forEach { blocks ->
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceEvenly) {
                            blocks.forEach { Text(it, fontFamily = FontFamily.Monospace, fontSize = 17.sp) }
                        }
                    }
                }
            }
            if (copied != null) TextButton(onClick = onCopy) { Text(stringResource(if (copied) R.string.recovery_copied else R.string.recovery_copy)) }
        }
    }
}


/** The account's card: avatar, the name never cut (it shrinks to half, as on iOS), the red 1 while unsecured, the code. */
/** The first page's rows under the profile card. */
@Composable private fun ProfileRows(s: AppState, repo: Repository, open: (String) -> Unit, onAddFriends: () -> Unit) {
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

/** The account's card. Given room (a weight at half height) it grows: the avatar from 64 up to 96 dp, the name with it,
 *  centred in the card; with none it is the compact card. */
@Composable private fun ProfileCard(s: AppState, onOpen: () -> Unit, onCode: () -> Unit, modifier: Modifier = Modifier, grow: Boolean = false) {
    @Composable fun content(avatar: androidx.compose.ui.unit.Dp) {
        val name = (26 * avatar.value / 64).coerceAtMost(36f).sp
        Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            SettingsAvatar(s.username.orEmpty(), avatar.value.toInt())
            BasicText(
                s.username.orEmpty(), Modifier.weight(1f),
                style = TextStyle(fontFamily = YoFont, fontSize = name, color = MaterialTheme.colorScheme.onSurface),
                maxLines = 1, softWrap = false,
                autoSize = TextAutoSize.StepBased(minFontSize = 13.sp, maxFontSize = name),
            )
            if (s.recoveryNeedsAttention) Badge { Text("1") }
            FilledTonalIconButton(onClick = onCode) { Icon(Icons.Default.QrCode, stringResource(R.string.ui_show_your_code)) }
        }
    }
    Card(onClick = onOpen, modifier = modifier.fillMaxWidth().padding(bottom = 20.dp), shape = MaterialTheme.shapes.extraLarge,
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceBright)) {
        if (grow) BoxWithConstraints(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            content((maxHeight - 32.dp).coerceIn(64.dp, 96.dp))
        } else content(64.dp)
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

/** Status (iOS StatusPage): crashes, usage, then the server and this phone's part. Tapping the Yos row cycles its view. */
@Composable private fun StatusPage(model: YoViewModel, s: AppState, connected: Boolean, open: (String) -> Unit) {
    val repo = model.repository
    val res = LocalResources.current
    val context = LocalContext.current
    var trip by remember { mutableStateOf<String?>(null) }
    var probing by remember { mutableStateOf(false) }
    var crashOn by remember { mutableStateOf(repo.crashes.enabled) }
    var yosView by rememberSaveable { mutableIntStateOf(0) }
    val notifications by model.notificationsEnabled.collectAsStateWithLifecycle()
    val top = s.byFriend.maxByOrNull { it.value }
    val mostYos = top?.let { most -> s.friends.firstOrNull { it.key == most.key }?.name } ?: "—"
    Section(stringResource(R.string.status_crashes), null,
        Setting(stringResource(R.string.ui_send_crashes), Icons.Default.BugReport, checked = crashOn, onChecked = {
            crashOn = it; repo.crashes.enabled = it
            if (it) repo.command { crashes.flush() }
        }),
        Setting(stringResource(R.string.ui_crash_reports), Icons.Default.Description, onClick = { open("Crash Reports") }),
    )
    Section(stringResource(R.string.status_usage), null,
        Setting(stringResource(listOf(R.string.status_total_yos, R.string.status_yos, R.string.top_friend)[yosView]), Icons.Default.BarChart,
            listOf("${s.sent + s.received}", "↑${s.sent} · ↓${s.received}", mostYos)[yosView], onClick = { yosView = (yosView + 1) % 3 }),
        Setting(stringResource(R.string.ui_friends), Icons.Default.People, s.friends.count { !it.blocked && it.group == null }.toString()),
    )
    Section(stringResource(R.string.status_server), null,
        Setting(stringResource(R.string.status_notifications), Icons.Default.Notifications,
            stringResource(if (notifications) R.string.status_ready else R.string.status_not_allowed), good = notifications,
            onClick = if (notifications) null else { { openNotificationSettings(context) } }),
        Setting(stringResource(R.string.ui_hub), Icons.Default.Cloud, stringResource(if (connected) R.string.ui_connected else R.string.ui_waiting), good = connected),
        Setting(stringResource(R.string.ui_round_trip), Icons.Default.Speed,
            if (probing) stringResource(R.string.ui_checking) else trip ?: stringResource(R.string.ui_not_checked), enabled = !probing, onClick = {
                probing = true
                repo.command {
                    try { trip = roundTrip()?.let { "$it ms" } ?: res.getString(R.string.ui_no_reply) } finally { probing = false }
                }
            }),
        Setting(stringResource(R.string.ui_encryption), Icons.Default.EnhancedEncryption, onClick = { open("Encryption") }),
    )
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
        // The empty chat: one incoming message from the team, centred, and a quiet word on privacy under it.
        Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp, Alignment.CenterVertically)) {
            SupportBubble(stringResource(R.string.support_empty_title), fromTeam = true)
            Text(stringResource(R.string.support_privacy), Modifier.padding(start = 16.dp),
                style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
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
            SupportBubble(message.text, message.fromTeam)
        }
    }
}

/** A chat bubble: the team's at the start in a neutral container, yours at the end in the primary colour. */
@Composable private fun SupportBubble(text: String, fromTeam: Boolean) {
    Box(Modifier.fillMaxWidth().padding(start = if (fromTeam) 0.dp else 48.dp, end = if (fromTeam) 48.dp else 0.dp),
        contentAlignment = if (fromTeam) Alignment.CenterStart else Alignment.CenterEnd) {
        Surface(
            shape = MaterialTheme.shapes.large,
            color = if (fromTeam) MaterialTheme.colorScheme.surfaceContainerHighest else MaterialTheme.colorScheme.primary,
            contentColor = if (fromTeam) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onPrimary,
        ) { Text(text, Modifier.padding(horizontal = 16.dp, vertical = 10.dp)) }
    }
}

@Composable private fun SupportComposer(value: String, change: (String) -> Unit, sending: Boolean, send: () -> Unit) {
    Surface(color = MaterialTheme.colorScheme.surfaceContainer) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp),
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
        Setting(res.getString(R.string.delete_name_free, s.username.orEmpty()), Icons.Default.PersonOff),
        Setting(stringResource(R.string.delete_friends_cant_reach), Icons.Default.GroupOff),
        Setting(stringResource(R.string.delete_cant_undo), Icons.Default.WarningAmber),
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
    var scanning by remember { mutableStateOf(false) }
    var confirmingVerify by remember { mutableStateOf(false) }
    var verifiedMenu by remember { mutableStateOf(false) }
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
        Card(Modifier.fillMaxWidth().padding(bottom = 20.dp), shape = MaterialTheme.shapes.extraLarge,
            colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceBright)) {
            Column(Modifier.fillMaxWidth().padding(16.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(R.string.compare_code), style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
                SelectionContainer {
                    Text(number.filter(Char::isDigit).chunked(5).chunked(3).joinToString("\n") { it.joinToString(" ") },
                        fontFamily = FontFamily.Monospace, textAlign = TextAlign.Center)
                }
                // Or scan each other's code: verified only when it matches (the QR code's own bytes, compared by the core).
                details?.safetyCode?.let { Qr(it.toString(Charsets.ISO_8859_1), Modifier.size(150.dp)) }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    TextButton(onClick = { scanning = true }) { Text(stringResource(R.string.scan_qr)) }
                    // iOS: Verified opens a menu with Clear Verification; marking asks first.
                    if (verified) Box {
                        TextButton(onClick = { verifiedMenu = true }) {
                            Icon(Icons.Default.Verified, null, Modifier.size(18.dp), tint = Color(0xFF2ECC71))
                            Text(stringResource(R.string.verified), Modifier.padding(start = 6.dp), color = Color(0xFF2ECC71))
                        }
                        DropdownMenu(verifiedMenu, onDismissRequest = { verifiedMenu = false }) {
                            DropdownMenuItem(text = { Text(stringResource(R.string.clear_verification), color = MaterialTheme.colorScheme.error) },
                                leadingIcon = { Icon(Icons.Default.RemoveModerator, null, tint = MaterialTheme.colorScheme.error) },
                                onClick = { verifiedMenu = false; repo.command { verify(friend, false); revision++ } })
                        }
                    }
                    else Button(onClick = { confirmingVerify = true }) { Text(stringResource(R.string.ui_mark_as_verified)) }
                }
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
    if (confirmingVerify) AlertDialog(
        onDismissRequest = { confirmingVerify = false },
        title = { Text(res.getString(R.string.verify_name, friend.name)) },
        text = { Text(res.getString(R.string.verify_message, friend.name)) },
        confirmButton = { TextButton(onClick = { confirmingVerify = false; repo.command { verify(friend, true); revision++ } }) {
            Text(stringResource(R.string.ui_mark_as_verified))
        } },
        dismissButton = { TextButton(onClick = { confirmingVerify = false }) { Text(stringResource(R.string.ui_cancel)) } },
    )
    if (scanning) QrScanner(onClose = { scanning = false }, onCode = { qr ->
        scanning = false
        repo.command {
            if (verifyScan(friend, qr.bytes)) withContext(Dispatchers.Main) { revision++ }
            else error(res.getString(R.string.safety_mismatch))
        }
    })
}

@Composable private fun LicensesPage() {
    val context = LocalContext.current
    val chunks = rememberLicenseChunks(true)
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(16.dp)) {
        // iOS LicensesPage: who makes Yo, its version, who operates it, the website.
        item {
            Section(null, null,
                Setting(stringResource(R.string.about_yo_by), onClick = {
                    context.startActivity(Intent(Intent.ACTION_VIEW, "https://justyo.co/p/imprint".toUri()))
                }),
                Setting(stringResource(R.string.about_version), value = BuildConfig.VERSION_NAME),
                Setting(stringResource(R.string.about_operated_by)),
                Setting(stringResource(R.string.about_website), value = "justyo.co", onClick = {
                    context.startActivity(Intent(Intent.ACTION_VIEW, "https://justyo.co".toUri()))
                }),
            )
        }
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

/** A step of recovery: its number (a tick once done), its name and state; beneath, its buttons side by side. */
@Composable private fun RecoveryStep(number: Int, title: String, status: String?, done: Boolean, buttons: @Composable RowScope.() -> Unit = {}) {
    Surface(Modifier.fillMaxWidth().padding(top = 8.dp), shape = RoundedCornerShape(16.dp), color = Color.White.copy(alpha = 0.14f), contentColor = Color.White) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Box(Modifier.size(26.dp).background(if (done) Color(0xFF2ECC71) else Color.White, CircleShape), contentAlignment = Alignment.Center) {
                    if (done) Icon(Icons.Default.Check, null, Modifier.size(16.dp), tint = Color.White)
                    else Text("$number", style = TextStyle(fontFamily = YoFont, fontSize = 13.sp, color = Color(0xFF8E44AD)))
                }
                Column(Modifier.weight(1f)) {
                    Text(title, style = MaterialTheme.typography.titleMedium)
                    if (status != null) Text(status, style = MaterialTheme.typography.bodySmall, color = Color.White.copy(alpha = 0.85f))
                }
            }
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp), content = buttons)
        }
    }
}

/** A card's button, as wide as the others: white with a black label (no purple on purple), or outlined in white. */
@Composable private fun RowScope.StepButton(label: String, icon: ImageVector?, prominent: Boolean = true, busy: Boolean = false,
    enabled: Boolean = true, onClick: () -> Unit) {
    val content: @Composable RowScope.() -> Unit = {
        if (busy) CircularProgressIndicator(Modifier.size(18.dp), color = if (prominent) Color.Black else Color.White, strokeWidth = 2.dp)
        else {
            if (icon != null) Icon(icon, null, Modifier.size(18.dp).padding(end = 4.dp))
            Text(label, maxLines = 1)
        }
    }
    if (prominent) Button(onClick = onClick, enabled = enabled, modifier = Modifier.weight(1f), content = content,
        colors = ButtonDefaults.buttonColors(containerColor = Color.White, contentColor = Color.Black,
            disabledContainerColor = Color.White.copy(alpha = 0.4f), disabledContentColor = Color.Black.copy(alpha = 0.6f)))
    else OutlinedButton(onClick = onClick, enabled = enabled, modifier = Modifier.weight(1f), content = content,
        border = androidx.compose.foundation.BorderStroke(1.5.dp, Color.White.copy(alpha = 0.6f)),
        colors = ButtonDefaults.outlinedButtonColors(contentColor = Color.White, disabledContentColor = Color.White.copy(alpha = 0.5f)))
}

/** The recovery screens (iOS RecoveryScreen): Amethyst at half height, its symbol, title and line, then the cards. */
@Composable private fun RecoveryScreen(icon: ImageVector, title: String, line: String, idle: Boolean, onClose: () -> Unit,
    content: @Composable ColumnScope.() -> Unit) {
    FittedSheet(onDismissRequest = { if (idle) onClose() }, containerColor = Amethyst, contentColor = Color.White, dragHandle = null, height = 0.5f) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp).padding(top = 12.dp, bottom = 16.dp),
            horizontalAlignment = Alignment.CenterHorizontally) {
            Box(Modifier.fillMaxWidth()) {
                FilledTonalIconButton(onClick = onClose, enabled = idle, modifier = Modifier.align(Alignment.TopEnd),
                    colors = IconButtonDefaults.filledTonalIconButtonColors(containerColor = Color.White.copy(alpha = 0.18f), contentColor = Color.White)) {
                    Icon(Icons.Default.Close, stringResource(R.string.ui_close))
                }
                Icon(icon, null, Modifier.align(Alignment.Center).padding(top = 16.dp).size(40.dp))
            }
            Text(title, Modifier.padding(top = 8.dp), style = TextStyle(fontFamily = YoFont, fontSize = 24.sp), textAlign = TextAlign.Center, maxLines = 2)
            Text(line, Modifier.padding(top = 4.dp, bottom = 10.dp), textAlign = TextAlign.Center,
                style = MaterialTheme.typography.bodySmall.copy(fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold), color = Color.White.copy(alpha = 0.85f))
            content()
        }
    }
}

/** The white Done at the bottom of a recovery screen: it closes at any time, not while a step works. */
@Composable private fun RecoveryDone(idle: Boolean, onClose: () -> Unit) =
    Button(onClick = onClose, enabled = idle, modifier = Modifier.fillMaxWidth().padding(top = 16.dp).heightIn(min = 48.dp),
        colors = ButtonDefaults.buttonColors(containerColor = Color.White, contentColor = Color.Black,
            disabledContainerColor = Color.White.copy(alpha = 0.4f), disabledContentColor = Color.Black.copy(alpha = 0.6f))) {
        Text(copy(ProductText.DONE), fontSize = 18.sp, fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold)
    }

/** The recovery key (iOS RecoveryKeySheet), from the red badge and the account page: save it, show it. */
@Composable fun RecoverySheet(model: YoViewModel, s: AppState, passkey: Boolean = false, onClose: () -> Unit) {
    val res = LocalResources.current
    val context = LocalContext.current
    var shown by remember { mutableStateOf(false) }
    var copied by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var keySaved by remember { mutableStateOf(false) }
    var sharing by remember { mutableStateOf(false) }
    var turningOff by remember { mutableStateOf(false) }
    // Opened on a secured account: the key shown is the one already saved.
    LaunchedEffect(s.recovery, s.recoverySaved) {
        if (s.secured && s.recoverySaved && s.recovery != null && withContext(Dispatchers.IO) { model.repository.savedRecovery() } == s.recovery) keySaved = true
    }
    fun saveWith(write: suspend (java.io.OutputStream, String) -> Unit) = { uri: Uri? ->
        if (uri != null) model.repository.command {
            busy = true
            try {
                val key = state.value.recovery ?: error(res.getString(R.string.ui_recovery_key_missing))
                context.contentResolver.openOutputStream(uri)?.use { write(it, key) } ?: error(res.getString(R.string.ui_couldn_t_save_the_recovery_key))
                if (!keySaved) { if (state.value.secured) markRecoverySaved() else keepRecovery() }
                keySaved = true
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
    val idle = !busy
    RecoveryScreen(Icons.Default.Key, copy(ProductText.RECOVERY_KEY_TITLE), copy(ProductText.RECOVERY_KEY_LINE), idle, onClose) {
        RecoveryStep(1, copy(ProductText.RECOVERY_KEY), copy(if (keySaved) ProductText.SAVED else ProductText.NOT_SAVED_YET), keySaved) {
            StepButton(copy(if (shown) ProductText.HIDE else ProductText.SHOW), if (shown) Icons.Default.VisibilityOff else Icons.Default.Visibility,
                prominent = false, enabled = ready && idle) { shown = !shown }
            StepButton(copy(if (keySaved) ProductText.SAVE_AGAIN else ProductText.SAVE), Icons.Default.Share, busy = busy,
                enabled = ready && idle) { sharing = true }
        }
        if (shown && key != null) Box(Modifier.padding(top = 8.dp)) {
            YoSystemTheme {
                KeyCard(key, copied) {
                    val clip = android.content.ClipData.newPlainText(res.getString(R.string.ui_recovery_key), key.chunked(4).joinToString(" "))
                    clip.description.extras = android.os.PersistableBundle().apply { putBoolean("android.content.extra.IS_SENSITIVE", true) }
                    context.getSystemService(android.content.ClipboardManager::class.java).setPrimaryClip(clip)
                    copied = true
                }
            }
        }
        RecoveryDone(idle, onClose)
        // Off: the backup moves to a new key at once, so the saved one stops working now; the new one is not saved yet.
        if (keySaved) TextButton(onClick = { turningOff = true }, enabled = idle, modifier = Modifier.padding(top = 4.dp),
            colors = ButtonDefaults.textButtonColors(contentColor = Color.White.copy(alpha = 0.85f))) { Text(stringResource(R.string.turn_off_recovery_key_action)) }
    }
    if (turningOff) AlertDialog(
        onDismissRequest = { turningOff = false },
        title = { Text(stringResource(R.string.turn_off_recovery_key)) },
        text = { Text(stringResource(if (passkey || !s.backedUp) R.string.turn_off_recovery_key_message else R.string.turn_off_recovery_key_google)) },
        confirmButton = { TextButton(onClick = {
            turningOff = false
            model.repository.command {
                busy = true
                try { turnOffRecovery(); keySaved = false; shown = false; copied = false } finally { busy = false }
            }
        }, colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error)) { Text(stringResource(R.string.settings_turn_off)) } },
        dismissButton = { TextButton(onClick = { turningOff = false }) { Text(stringResource(R.string.ui_cancel)) } },
    )
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


/** The passkey (iOS PasskeySheet), from the account page: create it, then try it once, as not every password manager
 *  gives the PRF output Yo needs and asking for it up front isn't reliable; on, it can be turned off again. */
@Composable private fun PasskeySheet(model: YoViewModel, configured: Boolean, onTurnedOff: () -> Unit, onClose: () -> Unit) {
    val res = LocalResources.current
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var on by remember(configured) { mutableStateOf(configured) }
    // Saved in the password manager, not yet tried: its id.
    var created by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var problem by remember { mutableStateOf<String?>(null) }
    var turningOff by remember { mutableStateOf(false) }
    val idle = !busy
    fun step(work: suspend () -> Unit) = scope.launch {
        busy = true; problem = null
        try { work() } catch (e: CancellationException) { throw e } catch (_: androidx.credentials.exceptions.GetCredentialCancellationException) {
        } catch (_: androidx.credentials.exceptions.CreateCredentialCancellationException) {
        } catch (_: IllegalArgumentException) { problem = res.getString(R.string.passkey_no_prf)
        } catch (e: Exception) { problem = e.localizedMessage ?: res.getString(R.string.passkey_try_again)
        } finally { busy = false }
    }
    RecoveryScreen(Icons.Default.Fingerprint, copy(ProductText.PASSKEY), stringResource(R.string.passkey_line), idle, onClose) {
        RecoveryStep(1, copy(ProductText.CREATE_PASSKEY), if (on || created != null) copy(ProductText.CREATED) else null, on || created != null) {
            if (!on && created == null) StepButton(copy(ProductText.CREATE), null, busy = busy, enabled = idle) {
                step {
                    val (name, user) = model.repository.passkeyAccount() ?: return@step
                    created = Passkeys.create(context, name, user)
                }
            }
        }
        RecoveryStep(2, copy(ProductText.TRY_IT_ONCE), copy(ProductText.TRY_IT_ONCE_LINE), on) {
            if (!on) StepButton(stringResource(R.string.passkey_try), Icons.Default.Fingerprint, busy = busy && created != null,
                enabled = idle && created != null) {
                step {
                    val id = created ?: return@step
                    val (_, user) = model.repository.passkeyAccount() ?: return@step
                    val answer = Passkeys.confirm(context, id, user)
                    withContext(Dispatchers.IO) { model.repository.keepPasskey(answer) }
                    created = null; on = true
                }
            }
        }
        problem?.let { Text(it, Modifier.padding(top = 8.dp), textAlign = TextAlign.Center, style = MaterialTheme.typography.bodySmall.copy(
            fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold)) }
        RecoveryDone(idle, onClose)
        if (on) TextButton(onClick = { turningOff = true }, enabled = idle, modifier = Modifier.padding(top = 4.dp),
            colors = ButtonDefaults.textButtonColors(contentColor = Color.White.copy(alpha = 0.85f))) { Text(copy(ProductText.TURN_OFF_PASSKEY)) }
    }
    // A passkey here always rides on a saved recovery key, so only the password manager is left to clean up.
    if (turningOff) AlertDialog(
        onDismissRequest = { turningOff = false },
        title = { Text(stringResource(R.string.settings_turn_off_passkey)) },
        text = { Text(stringResource(R.string.turn_off_passkey_message)) },
        confirmButton = { TextButton(onClick = {
            turningOff = false
            step { withContext(Dispatchers.IO) { model.repository.removeRecoveryPasskey() }; on = false; onTurnedOff() }
        }, colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error)) { Text(stringResource(R.string.settings_turn_off)) } },
        dismissButton = { TextButton(onClick = { turningOff = false }) { Text(stringResource(R.string.ui_cancel)) } },
    )
}

/** A page of Android's own settings, or its first page where this phone has no such page. */
private fun openSystemSettings(context: Context, action: String) {
    runCatching { context.startActivity(Intent(action)) }.onFailure { context.startActivity(Intent(android.provider.Settings.ACTION_SETTINGS)) }
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

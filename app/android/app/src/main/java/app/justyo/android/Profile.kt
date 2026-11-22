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
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.colorResource
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.heading
import android.view.HapticFeedbackConstants
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
    /** The red dot without a number (an unread support reply). */
    val dot: Boolean = false,
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

@Composable internal fun Section(title: String? = null, footer: String? = null, vararg rows: Setting?,
    titleAction: (@Composable () -> Unit)? = null, gap: androidx.compose.ui.unit.Dp = 20.dp) {
    val shown = rows.filterNotNull()
    run {
        Column(Modifier.fillMaxWidth().padding(bottom = gap), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
            if (title != null) Row(Modifier.fillMaxWidth().padding(start = 16.dp).heightIn(min = 24.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(title, Modifier.weight(1f).padding(bottom = if (titleAction == null) 6.dp else 0.dp),
                    style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
                titleAction?.invoke()
            }
            shown.forEachIndexed { index, row -> SettingRow(row, ListItemDefaults.segmentedShapes(index, shown.size)) }
            if (footer != null) Text(footer, Modifier.padding(horizontal = 16.dp, vertical = 6.dp),
                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

/** A row's leading symbol takes the App Icon picture's width, so the titles line up. */
private val LeadingSide = 32.dp

@Composable
private fun SettingRow(row: Setting, shapes: ListItemShapes) {
    val leading: (@Composable () -> Unit)? = row.leading ?: row.icon?.let { icon -> {
        Box(Modifier.size(LeadingSide), contentAlignment = Alignment.Center) { Icon(icon, null) }
    } }
    val hasTrailing = row.checked != null || row.value != null || row.badge || row.dot || row.good != null || row.action != null
    val trailing: (@Composable () -> Unit)? = if (!hasTrailing) null else { {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            row.value?.let { Text(it, color = MaterialTheme.colorScheme.onSurfaceVariant,
                style = MaterialTheme.typography.bodyMedium, maxLines = 1) }
            row.good?.let { ok ->
                Icon(if (ok) Icons.Default.CheckCircle else Icons.Default.ErrorOutline, null, Modifier.size(18.dp),
                    tint = if (ok) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.error)
            }
            if (row.badge) Badge { Text("1") }
            if (row.dot) RedDot()
            row.action?.let { (icon, label) -> IconButton(onClick = { row.onAction?.invoke() }, enabled = row.enabled,
                colors = IconButtonDefaults.iconButtonColors(contentColor = MaterialTheme.colorScheme.primary)) { Icon(icon, label) } }
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

/** The red dot of an unread support reply (and, on the Yo button, a missing backup): iOS's system red, opaque,
 *  with a thin white ring so it stands apart from the coloured rows behind the button. */
@Composable internal fun RedDot(modifier: Modifier = Modifier) {
    Box(modifier.size(12.dp).background(Color.White, CircleShape).padding(1.5.dp).background(Color(0xFFFF3B30), CircleShape))
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
    /** With `height`: a page longer than that grows the sheet, up to the tallest, instead of hiding its end below. */
    grows: Boolean = false,
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
    Column(when {
        height == null -> Modifier.heightIn(max = tallest)
        grows -> Modifier.heightIn(min = tallest * height, max = tallest)
        else -> Modifier.height(tallest * height)
    }, content = content)
}

/** Settings (iOS SettingsView): a height chosen per page: half for the pages that fit there (the first page, Account,
 *  Blocked, Account Security and App Icon; all but the first grow on a screen too short for them), full for every
 *  other; its pages pushed in with a back arrow. */
@Composable fun ProfileSheet(model: YoViewModel, onClose: () -> Unit, onAddFriends: () -> Unit) = YoSystemTheme {
    // The whole state (counters and the last sync included): the list underneath takes it without them.
    val s by model.repository.state.collectAsStateWithLifecycle()
    val sheet = rememberModalBottomSheetState(skipPartiallyExpanded = true)
    val scope = rememberCoroutineScope()
    fun hideThen(then: () -> Unit) { scope.launch { sheet.hide() }.invokeOnCompletion { then() } }
    // The way here: a contact's page is reached from Contacts.
    val trail = rememberSaveable { mutableStateOf(listOf("Profile")) }
    FittedSheet(onDismissRequest = onClose, state = sheet, containerColor = MaterialTheme.colorScheme.surfaceContainer,
        // On a short screen a half page may not fit (Account's "Delete Your Account" fell below): it grows to what it
        // needs. The first page alone stays at half, its card filling the room the rows leave.
        height = if (trail.value.last() in HalfPages) 0.5f else 1f, grows = trail.value.last() != "Profile") {
        Settings(model, s, trail, onClose = { hideThen(onClose) }, onAddFriends = { hideThen(onAddFriends) })
    }
}

/** The settings pages that fit at half height without scrolling. */
private val HalfPages = setOf("Profile", "Account", "Blocked", "Account Security", "Appearance")

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
    var turningOffKey by remember { mutableStateOf(false) }
    var turningOffPasskey by remember { mutableStateOf(false) }
    var code by remember { mutableStateOf(false) }
    var logout by remember { mutableStateOf(false) }
    var showingInfo by remember { mutableStateOf(false) }
    var needsLock by remember { mutableStateOf(false) }
    var showingRecoveryHelp by remember { mutableStateOf(false) }
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
        // No screen lock: said in the red banner (a snackbar would hide under this sheet); a tap on it opens Android's page.
        if (intent != null) auth.launch(intent) else if (page == "Delete Account") {
            // The banner lies under this full-height page: the button did nothing to see. Said here instead.
            needsLock = true
        } else {
            afterAuth = null
            Problems.say(res.getString(R.string.problem_needs_screen_lock)) {
                openSystemSettings(context, android.provider.Settings.ACTION_SECURITY_SETTINGS)
            }
        }
    }
    /** The recovery key's screen; a secured account's key only after the screen lock. */
    fun openKey() { if (s.secured) { afterAuth = "key"; authenticate() } else { repo.command { prepareRecovery() }; secure = true } }
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
        "Blocked" -> res.getString(R.string.ui_blocked_friends_can_t_send_you_yos_tap_unblock_to_unblock)
        "Status" -> res.getString(R.string.ui_anonymous_crash_reports_include_android_version_os_exit_reason_an)
        "Encryption" -> res.getString(R.string.ui_every_yo_photo_and_video_is_end_to_end_encrypted_only_you_an)
        else -> if (page.startsWith("contact:")) res.getString(R.string.compare_safety, title) else null
    }
    var feedback by rememberSaveable { mutableStateOf("") }
    var sendingFeedback by remember { mutableStateOf(false) }
    var support by remember { mutableStateOf<SupportThread?>(null) }
    // Settings opened: whether the Yo team answered since (iOS: the settings root's `refreshSupport`).
    LaunchedEffect(Unit) { repo.refreshSupport() }

    Column(Modifier.fillMaxWidth()) {
        // The first page: its title alone, centred (iOS "Yodentity"), no way back; the sheet's handle or back closes it.
        // A bottom sheet's title line, not a whole app bar: the page fits at half height.
        if (page == "Profile") Box(Modifier.fillMaxWidth().height(40.dp), contentAlignment = Alignment.TopCenter) {
            Text(stringResource(R.string.settings_title), Modifier.semantics { heading() }, style = MaterialTheme.typography.titleMedium, maxLines = 1)
        } else TopAppBar(
            title = { Text(title, maxLines = 1) },
            navigationIcon = {
                // A full page over a half one is iOS's own sheet on top: it closes (✕) back to where it was opened.
                val closes = page !in HalfPages && trail.dropLast(1).lastOrNull() in HalfPages
                IconButton(onClick = back) {
                    if (closes) Icon(Icons.Default.Close, stringResource(R.string.ui_close))
                    else Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.ui_back))
                }
            },
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
                shown == "Profile" && androidx.compose.ui.platform.LocalDensity.current.fontScale < 1.5f -> CardOverRows(
                    card = { ProfileCard(s, onOpen = { open("Account") }, onCode = { code = true }, Modifier.fillMaxHeight(), grow = true) },
                    rows = { Column { ProfileRows(s, repo, ::open, onAddFriends) } },
                )
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
                            Setting(stringResource(R.string.ui_blocked), Icons.Default.Block, onClick = { open("Blocked") }),
                        )
                        // Backup and the ways back on a new phone on a page of their own; the row says whether they are in place.
                        Section(null, null,
                            Setting(stringResource(R.string.account_security), Icons.Default.Shield,
                                badge = s.recoveryNeedsAttention, good = if (s.recoveryNeedsAttention) null else true,
                                // The red badge goes straight to the recovery key's screen, as before.
                                onClick = { if (s.recoveryNeedsAttention) openKey() else open("Account Security") }),
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
                        Section(null, null, accountBackupRow(repo))
                        // iOS AccountSecurityPage: the three ways back on a new phone, each with its switch; the (i) above
                        // explains them. A passkey here rides on a saved recovery key, so without one its switch opens the key's screen.
                        val keyOn = s.secured && s.recoverySaved
                        Section(copy(ProductText.ACCOUNT_RECOVERY), null,
                            // On opens the key's screen, saved there; off makes the saved key stop working.
                            // While on, the eye left of the switch opens the key's screen again, to show or save it.
                            Setting(copy(ProductText.RECOVERY_KEY), Icons.Default.Key, badge = s.recoveryNeedsAttention, checked = keyOn,
                                action = if (keyOn) Icons.Default.Visibility to copy(ProductText.SHOW_OR_SAVE_KEY) else null,
                                onAction = ::openKey,
                                onChecked = { on -> if (on) openKey() else turningOffKey = true }),
                            // On opens the passkey's screen; it shows on once tried there.
                            Setting(copy(ProductText.PASSKEY), Icons.Default.Fingerprint, checked = passkeyConfigured, onChecked = { on ->
                                when {
                                    !on -> turningOffPasskey = true
                                    s.secured -> { afterAuth = "passkey"; authenticate() }
                                    else -> { repo.command { prepareRecovery() }; secure = true }
                                }
                            }),
                            // Spec "Key material only in an end-to-end encrypted cloud": on only with a screen lock and Google's backup on.
                            Setting(stringResource(R.string.google_backup_row), Icons.Default.Cloud, checked = s.backedUp, badge = s.backupMissing,
                                supporting = when {
                                    s.backedUp || !s.googleBackup -> null
                                    s.backupMissingReason == CloudMissing.DEVICE_BACKUP -> stringResource(R.string.google_backup_turn_on_device_backup)
                                    else -> stringResource(R.string.google_backup_needs_screen_lock_short)
                                }, onChecked = { on ->
                                    when {
                                        !on -> googleOff = true
                                        !s.googleBackup -> repo.command { googleBackup(true) }
                                        else -> openSystemSettings(context,
                                            if (s.backupMissingReason == CloudMissing.DEVICE_BACKUP) "android.settings.BACKUP_AND_RESET_SETTINGS"
                                            else android.provider.Settings.ACTION_SECURITY_SETTINGS)
                                    }
                                }),
                            titleAction = {
                                IconButton(onClick = { showingRecoveryHelp = true }, Modifier.size(36.dp)) {
                                    Icon(Icons.Outlined.Info, stringResource(R.string.how_recovery_works))
                                }
                            },
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
                    shown == "Appearance" -> IconChoice(s.appIcon) { name -> repo.command { icon(name) } }
                    // iOS HelpPage: the tutorial, then Status and About & Licenses with the version.
                    shown == "Help" -> {
                        HelpHero(Modifier.padding(bottom = 20.dp))
                        Section(null, null,
                            Setting(stringResource(R.string.ui_status), Icons.Default.MonitorHeart,
                                stringResource(if (connected) R.string.ui_connected else R.string.ui_waiting), good = connected, onClick = { open("Status") }),
                            Setting(stringResource(R.string.about_and_licenses), Icons.Default.Info, rowVersion(), onClick = { open("Licenses") }),
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

    // Done there (the key kept) lands inside Account Security, where it now shows on; the ✕ stays where it was.
    if (secure) RecoverySheet(model, s, onClose = { secure = false },
        onDone = { secure = false; if (page != "Account Security") open("Account Security") })
    if (passkeyPage) PasskeySheet(model, passkeyConfigured) { passkeyPage = false; passkeyRevision++ }
    // Off: the backup moves to a new key at once, so the saved one stops working now; the new one is not saved yet.
    if (turningOffKey) AlertDialog(
        onDismissRequest = { turningOffKey = false },
        title = { Text(stringResource(R.string.turn_off_recovery_key)) },
        text = { Text(stringResource(if (passkeyConfigured || !s.backedUp) R.string.turn_off_recovery_key_message else R.string.turn_off_recovery_key_google)) },
        confirmButton = { TextButton(onClick = { turningOffKey = false; repo.command { turnOffRecovery() } },
            colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error)) { Text(stringResource(R.string.settings_turn_off)) } },
        dismissButton = { TextButton(onClick = { turningOffKey = false }) { Text(stringResource(R.string.ui_cancel)) } },
    )
    // A passkey here always rides on a saved recovery key, so only the password manager is left to clean up.
    if (turningOffPasskey) AlertDialog(
        onDismissRequest = { turningOffPasskey = false },
        title = { Text(stringResource(R.string.settings_turn_off_passkey)) },
        text = { Text(stringResource(R.string.turn_off_passkey_message)) },
        confirmButton = { TextButton(onClick = {
            turningOffPasskey = false
            repo.command { try { removeRecoveryPasskey() } finally { withContext(Dispatchers.Main) { passkeyRevision++ } } }
        }, colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error)) { Text(stringResource(R.string.settings_turn_off)) } },
        dismissButton = { TextButton(onClick = { turningOffPasskey = false }) { Text(stringResource(R.string.ui_cancel)) } },
    )
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
    if (showingRecoveryHelp) RecoveryHelpSheet(s) { showingRecoveryHelp = false }
    if (needsLock) AlertDialog(
        onDismissRequest = { needsLock = false },
        title = { Text(stringResource(R.string.problem_needs_screen_lock)) },
        text = { Text(stringResource(R.string.delete_needs_screen_lock)) },
        confirmButton = { TextButton(onClick = {
            needsLock = false
            openSystemSettings(context, android.provider.Settings.ACTION_SECURITY_SETTINGS)
        }) { Text(stringResource(R.string.ui_settings)) } },
        dismissButton = { TextButton(onClick = { needsLock = false }) { Text(stringResource(R.string.ui_cancel)) } },
    )
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
    FittedSheet(onDismissRequest = onClose, containerColor = Amethyst, contentColor = Color.White,
        // The tall card, like iOS's large detent: people screenshot it, so no friend names show behind it.
        height = 1f) {
        Column(Modifier.fillMaxSize().padding(horizontal = 20.dp).padding(bottom = 24.dp), horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.SpaceBetween) {
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
    Surface(Modifier.fillMaxWidth(), shape = RoundedCornerShape(18.dp), color = Color.White.copy(alpha = 0.14f), contentColor = Color.White) {
        Column(Modifier.fillMaxWidth().padding(16.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Qr(key, Modifier.size(120.dp).clip(RoundedCornerShape(8.dp)))
            SelectionContainer {
                Column {
                    key.chunked(4).chunked(4).forEach { blocks ->
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceEvenly) {
                            blocks.forEach { Text(it, fontFamily = FontFamily.Monospace, fontSize = 17.sp) }
                        }
                    }
                }
            }
            if (copied != null) TextButton(onClick = onCopy, colors = ButtonDefaults.textButtonColors(contentColor = Color.White)) {
                Text(stringResource(if (copied) R.string.recovery_copied else R.string.recovery_copy), fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold)
            }
        }
    }
}


/** Account Backup (iOS AccountSyncRow): when Yo's server last held it, a check once it does; a tap backs up now. */
@Composable private fun accountBackupRow(repo: Repository): Setting {
    val res = LocalResources.current
    val view = androidx.compose.ui.platform.LocalView.current
    var savedAt by remember { mutableStateOf<Long?>(null) }
    var unavailable by remember { mutableStateOf(true) }
    var saving by remember { mutableStateOf(false) }
    var failed by remember { mutableStateOf(false) }
    var justSaved by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    suspend fun load() = withContext(Dispatchers.IO) {
        runCatching { repo.backupSavedAt() }.onSuccess { savedAt = it; unavailable = false }.onFailure { unavailable = true }
    }
    // A backup from elsewhere (the inbox's sync) shows up.
    LaunchedEffect(Unit) { while (true) { load(); delay(5000) } }
    LaunchedEffect(justSaved) { if (justSaved) { delay(60_000); justSaved = false } }
    val value = when {
        failed -> res.getString(R.string.backup_failed)
        unavailable -> res.getString(R.string.backup_unavailable)
        justSaved -> res.getString(R.string.backup_just_now)
        else -> savedAt?.let { at ->
            val then = java.time.Instant.ofEpochMilli(at).atZone(java.time.ZoneId.systemDefault())
            val today = java.time.LocalDate.now()
            when (then.toLocalDate()) {
                today -> android.text.format.DateFormat.getTimeFormat(view.context).format(java.util.Date(at))
                today.minusDays(1) -> res.getString(R.string.backup_yesterday)
                else -> java.time.format.DateTimeFormatter.ofPattern("d MMM").format(then)
            }
        } ?: res.getString(R.string.backup_pending)
    }
    return Setting(stringResource(R.string.account_backup), Icons.Default.Cloud, value,
        good = when { failed -> false; unavailable || savedAt == null -> null; else -> true },
        enabled = !saving, onClick = {
            saving = true; failed = false
            val before = savedAt
            scope.launch {
                val ok = withContext(Dispatchers.IO) { repo.backUpNow() }
                load()
                failed = !ok || unavailable || savedAt == null
                justSaved = !failed && savedAt != before
                if (android.os.Build.VERSION.SDK_INT >= 30) view.performHapticFeedback(if (failed) HapticFeedbackConstants.REJECT else HapticFeedbackConstants.CONFIRM)
                saving = false
            }
        })
}

/** How Account Recovery Works (iOS RecoveryHelpSheet): the account key and the ways it comes back, one by one. */
@Composable private fun RecoveryHelpSheet(s: AppState, onClose: () -> Unit) {
    FittedSheet(onDismissRequest = onClose, containerColor = MaterialTheme.colorScheme.surfaceContainer) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 24.dp).padding(bottom = 24.dp),
            verticalArrangement = Arrangement.spacedBy(20.dp)) {
            Text(stringResource(R.string.how_recovery_works), style = MaterialTheme.typography.titleLarge)
            Text(stringResource(R.string.recovery_help_intro), color = MaterialTheme.colorScheme.onSurfaceVariant)
            @Composable fun item(icon: ImageVector, title: String, text: String) {
                Row(Modifier.semantics(mergeDescendants = true) {}, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                    Icon(icon, null, Modifier.size(28.dp), tint = MaterialTheme.colorScheme.primary)
                    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        Text(title, style = MaterialTheme.typography.titleMedium)
                        Text(text, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            }
            item(Icons.Default.Cloud, stringResource(R.string.google_backup_row), stringResource(when {
                s.backedUp -> R.string.google_backup_footer
                s.googleBackup && s.backupMissingReason == CloudMissing.DEVICE_BACKUP -> R.string.google_backup_off_device_backup
                s.googleBackup -> R.string.google_backup_off
                else -> R.string.google_backup_turned_off
            }))
            item(Icons.Default.Fingerprint, copy(ProductText.PASSKEY), stringResource(R.string.recovery_help_passkey))
            item(Icons.Default.Key, copy(ProductText.RECOVERY_KEY), stringResource(R.string.recovery_help_key))
            item(Icons.Default.CloudSync, stringResource(R.string.account_backup), stringResource(R.string.recovery_help_backup))
            Text(stringResource(R.string.recovery_help_footer), style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

/** The first page's rows under the profile card. */
@Composable private fun ProfileRows(s: AppState, repo: Repository, open: (String) -> Unit, onAddFriends: () -> Unit) {
    Section(null, null,
        gap = 12.dp, rows = arrayOf(Setting(stringResource(R.string.ui_app_icon), leading = { AppIconPreview(s.appIcon, 32, 8) }, onClick = { open("Appearance") }),
        Setting(stringResource(R.string.ui_left_handed), Icons.Default.PanTool, checked = s.leftHanded,
            onChecked = { on -> repo.command { settings(left = on) } })),
    )
    Section(null, null,
        gap = 12.dp, rows = arrayOf(Setting(stringResource(R.string.help_and_about), Icons.AutoMirrored.Filled.Help, onClick = { open("Help") }),
        Setting(stringResource(R.string.ui_send_feedback), Icons.Default.Forum, dot = s.supportUnread, onClick = { open("Contact Us") })),
    )
    Section(null, null,
        gap = 12.dp, rows = arrayOf(Setting(stringResource(R.string.ui_add_friends), Icons.Default.PersonAdd, onClick = onAddFriends)),
    )
}

/** The first page at half height: the rows at their own height, the card in the room they leave, but never less than
 *  the compact card. A half sheet too short for both (a short screen, larger text) scrolls instead of squeezing the
 *  card to nothing. */
@Composable private fun CardOverRows(card: @Composable () -> Unit, rows: @Composable () -> Unit) {
    val scroll = rememberScrollState()
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val room = constraints.maxHeight
        androidx.compose.ui.layout.SubcomposeLayout(Modifier.verticalScroll(scroll).padding(horizontal = 16.dp)) { c ->
            val width = c.maxWidth
            val below = subcompose("rows", rows).map { it.measure(androidx.compose.ui.unit.Constraints(maxWidth = width)) }
            val rowsHeight = below.sumOf { it.height }
            val cardHeight = maxOf(room - rowsHeight, CompactCard.roundToPx())
            val top = subcompose("card", card).map { it.measure(androidx.compose.ui.unit.Constraints.fixed(width, cardHeight)) }
            layout(width, cardHeight + rowsHeight) {
                top.forEach { it.place(0, 0) }
                var y = cardHeight
                below.forEach { it.place(0, y); y += it.height }
            }
        }
    }
}

/** The compact card's height: a 48 dp avatar, the card's 16 dp padding around it and the 12 dp under the card. */
private val CompactCard = 48.dp + 32.dp + 12.dp

/** The account's card. Given room (a weight at half height) it grows: the avatar from 64 up to 96 dp, the name with it,
 *  centred in the card; with none it is the compact card. */
@Composable private fun ProfileCard(s: AppState, onOpen: () -> Unit, onCode: () -> Unit, modifier: Modifier = Modifier, grow: Boolean = false) {
    @Composable fun content(avatar: androidx.compose.ui.unit.Dp) {
        val name = (26 * avatar.value / 64).coerceAtMost(36f).sp
        Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            SettingsAvatar(s.username.orEmpty(), avatar.value.toInt())
            Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                BasicText(
                    s.username.orEmpty(), Modifier.weight(1f, fill = false),
                    style = TextStyle(fontFamily = YoFont, fontSize = name, color = MaterialTheme.colorScheme.onSurface),
                    maxLines = 1, softWrap = false,
                    // The smallest step as at the default text size: with large text the name had no room left
                    // beside the badge and the code button and was cut ("H3M").
                    autoSize = TextAutoSize.StepBased(minFontSize = with(LocalDensity.current) { 13.dp.toSp() }, maxFontSize = name),
                )
                if (s.recoveryNeedsAttention) Badge { Text("1") }
            }
            FilledTonalIconButton(onClick = onCode) { Icon(Icons.Default.QrCode, stringResource(R.string.ui_show_your_code)) }
        }
    }
    Card(onClick = onOpen, modifier = modifier.fillMaxWidth().padding(bottom = 12.dp), shape = MaterialTheme.shapes.extraLarge,
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceBright)) {
        if (grow) BoxWithConstraints(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
            content((maxHeight - 32.dp).coerceIn(48.dp, 96.dp))
        } else content(64.dp)
    }
}

/** The app icons iOS offers (AppIconPage.icons), by the same names: three to a row, as there. */
private val AppIcons = listOf<Pair<String?, Int>>(null to R.string.icon_default, "Negative" to R.string.icon_negative,
    "Original" to R.string.icon_original, "OG2014" to R.string.icon_og2014, "Lilac2015" to R.string.icon_lilac2015)

@Composable private fun IconChoice(current: String?, choose: (String?) -> Unit) {
    Column(Modifier.fillMaxWidth().padding(bottom = 20.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        AppIcons.chunked(3).forEach { row ->
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                row.forEach { (name, label) ->
                    val selected = current == name
                    OutlinedCard(
                        Modifier.weight(1f).selectable(selected, onClick = { choose(name) }),
                        border = if (selected) CardDefaults.outlinedCardBorder().copy(width = 2.dp, brush = androidx.compose.ui.graphics.SolidColor(MaterialTheme.colorScheme.primary))
                            else CardDefaults.outlinedCardBorder(),
                    ) {
                        Column(Modifier.fillMaxWidth().padding(vertical = 12.dp), horizontalAlignment = Alignment.CenterHorizontally,
                            verticalArrangement = Arrangement.spacedBy(8.dp)) {
                            AppIconPreview(name, 64, 16)
                            Text(stringResource(label), maxLines = 1, style = MaterialTheme.typography.labelLarge)
                        }
                    }
                }
                repeat(3 - row.size) { Spacer(Modifier.weight(1f)) }
            }
        }
    }
}

/** An icon as the launcher shows it: the Yo wordmark or its negative on Amethyst; Full Color, OG 2014 and 2015 are
 *  plain squares of their purple, as iOS's pictures are. */
@Composable private fun AppIconPreview(name: String?, size: Int, radius: Int) {
    val modifier = Modifier.size(size.dp).clip(RoundedCornerShape(radius.dp))
    when (name) {
        "Original" -> Box(modifier.background(colorResource(R.color.original_purple)))
        "OG2014" -> Box(modifier.background(Amethyst))
        "Lilac2015" -> Box(modifier.background(colorResource(R.color.lilac_2015)))
        else -> Image(painterResource(if (name == "Negative") R.drawable.icon_negative else R.drawable.launcher_wordmark), null,
            modifier.background(Amethyst))
    }
}

/** Status (iOS StatusPage): crashes, usage, then the server and this phone's part. Tapping the Yos row cycles its view. */
@Composable private fun StatusPage(model: YoViewModel, s: AppState, connected: Boolean, open: (String) -> Unit) {
    val repo = model.repository
    val res = LocalResources.current
    val context = LocalContext.current
    var trip by remember { mutableStateOf<String?>(null) }
    var probing by remember { mutableStateOf(false) }
    var renewing by remember { mutableStateOf(false) }
    var renewFailed by remember { mutableStateOf<String?>(null) }
    var crashOn by remember { mutableStateOf(repo.crashes.enabled) }
    var yosView by rememberSaveable { mutableIntStateOf(0) }
    val notifications by model.notificationsEnabled.collectAsStateWithLifecycle()
    val pushSession by repo.pushSession.collectAsStateWithLifecycle()
    val pushToken = pushSession == repo.sessionId
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
            stringResource(if (!notifications) R.string.status_not_allowed else if (pushToken) R.string.status_ready else R.string.status_no_token),
            good = if (!notifications) false else if (pushToken) true else null,
            onClick = if (notifications) null else { { openNotificationSettings(context) } }),
        // As Signal's: a new token to the hub. What failed is said on the row as well: the red banner is under this sheet.
        Setting(copy(ProductText.RENEW_PUSH), value = renewFailed, good = renewFailed?.let { false },
            enabled = !renewing && s.username != null, leading = {
            Box(Modifier.size(32.dp), contentAlignment = Alignment.Center) {
                if (renewing) CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp) else Icon(Icons.Default.Refresh, null)
            }
        }, onClick = {
            renewing = true; renewFailed = null
            repo.command {
                try { renewPush() } catch (e: CancellationException) { throw e } catch (e: Exception) {
                    renewFailed = productProblemText(Problems.problemOf(e), Words)
                    Problems.report(e) { command { renewPush() } }
                } finally { renewing = false }
            }
        }),
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
                val thread = withContext(Dispatchers.IO) { repo.supportThread() }
                loaded(thread); offline = false
                // Shown, so read: the red dot goes at once, not one round later.
                thread?.messages?.maxOfOrNull { it.id }?.let { withContext(Dispatchers.IO) { repo.supportRead(it) } }
            } catch (e: CancellationException) { throw e } catch (_: Exception) { offline = true }
            delay(5000)
        }
    }
    val messages = support?.messages.orEmpty()
    // Like a chat: the privacy line on top, the team's greeting as the first message, everything stacked from the
    // bottom above the field; the greeting stays first, so sending the first message doesn't move it.
    val scroll = rememberScrollState()
    LaunchedEffect(scroll) { snapshotFlow { scroll.maxValue }.collect { scroll.animateScrollTo(it) } }
    BoxWithConstraints(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxWidth().verticalScroll(scroll).heightIn(min = maxHeight).padding(16.dp),
            verticalArrangement = Arrangement.SpaceBetween) {
            Text(stringResource(R.string.support_privacy), Modifier.fillMaxWidth(), textAlign = TextAlign.Center,
                style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Column(Modifier.fillMaxWidth().padding(top = 16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text(stringResource(R.string.ui_yo_team), Modifier.padding(start = 16.dp),
                        style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    SupportBubble(stringResource(R.string.support_empty_title), fromTeam = true)
                }
                messages.forEach { message -> key(message.id) { SupportBubble(message.text, message.fromTeam) } }
                if (offline) Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.errorContainer)) {
                    Row(Modifier.padding(start = 16.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text(stringResource(R.string.support_offline), Modifier.weight(1f).padding(vertical = 12.dp))
                        TextButton(onClick = { retry++ }) { Text(stringResource(R.string.retry)) }
                    }
                }
            }
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
                Setting(stringResource(R.string.about_version), value = shownVersion()),
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

/** The recovery screens (iOS RecoveryScreen): a full-screen Amethyst assistant without cards, its symbol, title and
 *  line centred; at the bottom the step as a small line (a green tick once done), the problem if any, one big white
 *  button (black label, no purple on purple) and its quiet links. The close button top right leaves at any time. */
@Composable private fun RecoveryScreen(icon: ImageVector, title: String, line: String, step: String, stepDone: Boolean,
    problem: String?, idle: Boolean, button: String, buttonIcon: ImageVector?, onButton: () -> Unit, onClose: () -> Unit,
    links: @Composable RowScope.() -> Unit = {}, content: @Composable ColumnScope.() -> Unit = {}) {
    androidx.compose.ui.window.Dialog(onDismissRequest = { if (idle) onClose() }, properties = androidx.compose.ui.window.DialogProperties(
        usePlatformDefaultWidth = false, decorFitsSystemWindows = false, dismissOnClickOutside = false)) {
        val window = (androidx.compose.ui.platform.LocalView.current.parent as? androidx.compose.ui.window.DialogWindowProvider)?.window
        SideEffect { if (android.os.Build.VERSION.SDK_INT >= 29) window?.isNavigationBarContrastEnforced = false }
        Surface(Modifier.fillMaxSize(), color = Amethyst, contentColor = Color.White) {
            // Bars and cutout only, not the keyboard: the screen has no field, and the PIN pad of the screen lock just
            // before can still count as open when it appears, which pushed the button half up the screen.
            Box(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.systemBars.union(WindowInsets.displayCutout))) {
                Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 24.dp).padding(top = 56.dp, bottom = 24.dp),
                    horizontalAlignment = Alignment.CenterHorizontally) {
                    Spacer(Modifier.weight(1f))
                    Icon(icon, null, Modifier.size(56.dp))
                    Text(title, Modifier.padding(top = 20.dp), style = TextStyle(fontFamily = YoFont, fontSize = 34.sp), textAlign = TextAlign.Center, maxLines = 2)
                    Text(line, Modifier.padding(top = 8.dp, bottom = 24.dp), textAlign = TextAlign.Center,
                        style = TextStyle(fontSize = 15.sp, lineHeight = 20.sp, fontWeight = androidx.compose.ui.text.font.FontWeight.Medium), color = Color.White.copy(alpha = 0.85f))
                    content()
                    Spacer(Modifier.weight(1f))
                    Row(Modifier.padding(top = 24.dp).semantics(mergeDescendants = true) {}, verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        if (stepDone) Box(Modifier.size(16.dp).background(Color(0xFF2ECC71), CircleShape), contentAlignment = Alignment.Center) {
                            Icon(Icons.Default.Check, null, Modifier.size(11.dp), tint = Color.White)
                        }
                        Text(step, style = TextStyle(fontSize = 13.sp, fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold),
                            color = Color.White.copy(alpha = if (stepDone) 1f else 0.85f))
                    }
                    problem?.let { Text(it, Modifier.padding(top = 8.dp), textAlign = TextAlign.Center, style = MaterialTheme.typography.bodySmall.copy(
                        fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold)) }
                    Button(onClick = onButton, enabled = idle, modifier = Modifier.fillMaxWidth().padding(top = 10.dp).heightIn(min = 48.dp),
                        colors = ButtonDefaults.buttonColors(containerColor = Color.White, contentColor = Color.Black,
                            disabledContainerColor = Color.White.copy(alpha = 0.4f), disabledContentColor = Color.Black.copy(alpha = 0.6f))) {
                        if (!idle) CircularProgressIndicator(Modifier.size(20.dp), color = Color.Black, strokeWidth = 2.dp)
                        else {
                            if (buttonIcon != null) Icon(buttonIcon, null, Modifier.size(20.dp).padding(end = 6.dp))
                            Text(button, fontSize = 17.sp, fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold, maxLines = 1)
                        }
                    }
                    Row(Modifier.padding(top = 6.dp), verticalAlignment = Alignment.CenterVertically, content = links)
                }
                FilledTonalIconButton(onClick = onClose, enabled = idle, modifier = Modifier.align(Alignment.TopEnd).padding(top = 8.dp, end = 12.dp),
                    colors = IconButtonDefaults.filledTonalIconButtonColors(containerColor = Color.White.copy(alpha = 0.18f), contentColor = Color.White)) {
                    Icon(Icons.Default.Close, stringResource(R.string.ui_close))
                }
            }
        }
    }
}

/** A quiet link under the big button: white text with its icon, no fill. */
@Composable private fun RecoveryLink(label: String, icon: ImageVector, enabled: Boolean, onClick: () -> Unit) {
    TextButton(onClick = onClick, enabled = enabled, colors = ButtonDefaults.textButtonColors(contentColor = Color.White,
        disabledContentColor = Color.White.copy(alpha = 0.5f))) {
        Icon(icon, null, Modifier.size(18.dp).padding(end = 4.dp))
        Text(label, fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold, maxLines = 1)
    }
}

/** The recovery key (iOS RecoveryKeySheet), from the red badge and the account page: save it, show it. */
/** `onDone`: Done once the key is kept, when it leads somewhere else than `onClose` (the ✕). */
@Composable fun RecoverySheet(model: YoViewModel, s: AppState, onClose: () -> Unit, onDone: () -> Unit = onClose) {
    val res = LocalResources.current
    val context = LocalContext.current
    var shown by remember { mutableStateOf(false) }
    var copied by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var keySaved by remember { mutableStateOf(false) }
    // Kept this time by showing it, not by saving: the step says "Shown".
    var keptByShowing by remember { mutableStateOf(false) }
    var sharing by remember { mutableStateOf(false) }
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
                keySaved = true; keptByShowing = false
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
    fun show() {
        shown = !shown
        // Seen counts as kept, as saved does: the step is ticked and the button becomes Done.
        if (shown && !keySaved) model.repository.command {
            busy = true
            try { if (state.value.secured) markRecoverySaved() else keepRecovery(); keySaved = true; keptByShowing = true }
            finally { busy = false }
        }
    }
    RecoveryScreen(Icons.Default.Key, copy(ProductText.RECOVERY_KEY_TITLE), copy(ProductText.RECOVERY_KEY_LINE),
        when { !keySaved -> ""; keptByShowing -> copy(ProductText.SHOWN); else -> copy(ProductText.SAVED) }, keySaved,
        null, idle, copy(if (keySaved) ProductText.DONE else ProductText.SAVE_RECOVERY_KEY), if (keySaved) null else Icons.Default.Share,
        onButton = { if (keySaved) onDone() else if (ready) sharing = true }, onClose = onClose,
        links = {
            if (keySaved) {
                RecoveryLink(copy(ProductText.SAVE_AGAIN), Icons.Default.Share, ready && idle) { sharing = true }
                Text("·", Modifier.clearAndSetSemantics {})
            }
            RecoveryLink(copy(if (shown) ProductText.HIDE_KEY else ProductText.SHOW_KEY),
                if (shown) Icons.Default.VisibilityOff else Icons.Default.Visibility, ready && idle, ::show)
        }) {
        if (shown && key != null) KeyCard(key, copied) {
            val clip = android.content.ClipData.newPlainText(res.getString(R.string.ui_recovery_key), key.chunked(4).joinToString(" "))
            clip.description.extras = android.os.PersistableBundle().apply { putBoolean("android.content.extra.IS_SENSITIVE", true) }
            context.getSystemService(android.content.ClipboardManager::class.java).setPrimaryClip(clip)
            copied = true
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


/** The passkey (iOS PasskeySheet), from the account page: create it, then try it once, as not every password manager
 *  gives the PRF output Yo needs and asking for it up front isn't reliable; its switch on the page turns it off. */
@Composable private fun PasskeySheet(model: YoViewModel, configured: Boolean, onClose: () -> Unit) {
    val res = LocalResources.current
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var on by remember(configured) { mutableStateOf(configured) }
    // Saved in the password manager, not yet tried: its id.
    var created by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var problem by remember { mutableStateOf<String?>(null) }
    val idle = !busy
    // `failed`: the line when it went wrong (iOS: creating says so, trying says "That didn't work").
    fun step(failed: Int, work: suspend () -> Unit) = scope.launch {
        busy = true; problem = null
        try { work() } catch (e: CancellationException) { throw e } catch (_: androidx.credentials.exceptions.GetCredentialCancellationException) {
        } catch (_: androidx.credentials.exceptions.CreateCredentialCancellationException) {
        } catch (_: IllegalArgumentException) { problem = res.getString(R.string.passkey_no_prf)
        } catch (e: Exception) {
            // A short line on screen; what went wrong only in the log.
            android.util.Log.w("YoPasskey", "passkey step failed: ${e::class.simpleName}")
            problem = res.getString(failed)
        } finally { busy = false }
    }
    fun create() = step(R.string.passkey_create_failed) {
        val (name, user) = model.repository.passkeyAccount() ?: return@step
        created = Passkeys.create(context, name, user)
    }
    fun check() = step(R.string.passkey_try_again) {
        val id = created ?: return@step
        val (_, user) = model.repository.passkeyAccount() ?: return@step
        val answer = Passkeys.confirm(context, id, user)
        withContext(Dispatchers.IO) { model.repository.keepPasskey(answer) }
        created = null; on = true
    }
    RecoveryScreen(Icons.Default.Fingerprint, copy(ProductText.PASSKEY), stringResource(R.string.passkey_line),
        copy(when { on -> ProductText.PASSKEY_ON; created != null -> ProductText.PASSKEY_STEP_TRY; else -> ProductText.PASSKEY_STEP_CREATE }), on,
        problem, idle,
        when { on -> copy(ProductText.DONE); created != null -> stringResource(R.string.passkey_try); else -> copy(ProductText.CREATE_PASSKEY) },
        if (!on && created != null) Icons.Default.Fingerprint else null,
        onButton = { when { on -> onClose(); created != null -> check(); else -> create() } }, onClose = onClose)
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

/** The Help row's short form, one line: "1.0.0 RC19", else "1.0.0 dev" (the build number is on the About page). */
fun rowVersion(): String = shownVersion().let { if (it.contains(" (build ")) it.substringBefore(" (build ") + " dev" else it }

/** "1.0.0 RC19" for a release (versionName 1.0.0-RC19, from the tag), else "<versionName> (build 202611211230)". */
fun shownVersion(): String =
    if (BuildConfig.VERSION_NAME.contains('-') && !BuildConfig.VERSION_NAME.endsWith("-dev")) BuildConfig.VERSION_NAME.replaceFirst('-', ' ')
    else "${BuildConfig.VERSION_NAME} (build ${BuildConfig.YO_BUILD})"

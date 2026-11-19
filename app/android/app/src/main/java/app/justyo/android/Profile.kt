@file:OptIn(androidx.compose.material3.ExperimentalMaterial3ExpressiveApi::class, androidx.compose.material3.ExperimentalMaterial3Api::class)
package app.justyo.android

import kotlinx.coroutines.launch
import android.app.Activity
import android.app.KeyguardManager
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.*
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.BasicText
import androidx.compose.foundation.text.TextAutoSize
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.FileProvider
import androidx.core.graphics.set
import androidx.core.net.toUri
import com.google.zxing.BarcodeFormat
import com.google.zxing.MultiFormatReader
import com.google.zxing.RGBLuminanceSource
import com.google.zxing.BinaryBitmap
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeWriter
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import kotlinx.coroutines.*
import java.io.File
import uniffi.yo.*

@Composable fun ProfileSheet(model: YoViewModel,s: AppState,onClose: ()->Unit,onAddFriends: ()->Unit) {
    YoSystemTheme { ProfileContent(model, s, onClose, onAddFriends) }
}

@Composable private fun ProfileContent(model: YoViewModel,s: AppState,onClose: ()->Unit,onAddFriends: ()->Unit) {
    val uiResources=androidx.compose.ui.platform.LocalResources.current

    val repo=model.repository
    var page by rememberSaveable { mutableStateOf("Profile") }
    val licenseChunks=rememberLicenseChunks(page=="Licenses")
    var secure by remember { mutableStateOf(false) }
    var code by remember { mutableStateOf(false) }
    var logout by remember { mutableStateOf(false) }
    var renew by remember { mutableStateOf(false) }
    var removePasskey by remember { mutableStateOf(false) }
    var passkeyRevision by remember { mutableIntStateOf(0) }
    var deleting by rememberSaveable { mutableStateOf("") }
    var feedback by rememberSaveable { mutableStateOf("") }
    var sendingFeedback by remember { mutableStateOf(false) }
    var supportOffline by remember { mutableStateOf(false) }
    var supportRetry by remember { mutableIntStateOf(0) }
    var support by remember { mutableStateOf<uniffi.yo.SupportThread?>(null) }
    var trip by remember { mutableStateOf<String?>(null) }
    var probing by remember { mutableStateOf(false) }
    var crashOn by remember { mutableStateOf(repo.crashes.enabled) }
    var status by remember { mutableStateOf<SecurityStatus?>(null) }
    var selected by remember { mutableStateOf<Friend?>(null) }
    val contacts = remember(s.friends) {
        s.friends.filter { it.group == null && !it.blocked }.sortedBy { it.name.lowercase() }
    }
    val facts by produceState<Map<String, YoContact>>(emptyMap(), contacts, s.lastSync, selected, page) {
        if (page == "Contacts") value = withContext(Dispatchers.IO) {
            contacts.mapNotNull { friend -> repo.contact(friend)?.contact?.let { friend.key to it } }.toMap()
        }
    }
    var recoveryHelp by remember { mutableStateOf(false) }
    var encryptionFromStatus by rememberSaveable { mutableStateOf(false) }
    val goBack = {
        page = when (page) {
            "Contact Us", "Status", "Licenses" -> "Help"
            "Crash Reports" -> "Status"
            "Delete Account", "Contacts", "Blocked", "Media", "Statistics", "Account Recovery" -> "Account"
            "Encryption" -> if (encryptionFromStatus) "Status" else "Account"
            else -> "Profile"
        }
    }
    var notificationGranted by remember { mutableStateOf(false) }
    val notificationPermission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { notificationGranted = it; model.notificationResult(it) }
    val notificationsEnabled = notificationGranted || contextNotificationsEnabled()
    var encrypted by remember { mutableStateOf<Pair<Int,Int>?>(null) }
    LaunchedEffect(s.friends, s.lastSync) {
        encrypted=withContext(Dispatchers.IO) {
            val contacts=s.friends.filter { it.group==null && !it.blocked }
            var good=0
            for(friend in contacts) {
                val details=repo.contact(friend) ?: return@withContext null
                if(friend.user!=null && !friend.gone && !friend.keyChanged && details.contact?.gone!=true &&
                    details.contact?.unconfirmed!=true && details.contact?.outdated!=true &&
                    details.sessions.all { it.usable && !it.retrying }) good++
            }
            good to contacts.size
        }
    }
    val context=LocalContext.current
    val auth=rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        if(result.resultCode==Activity.RESULT_OK) {
            if(page=="Delete Account") repo.command { deleteAccount(); onClose() } else { repo.command { prepareRecovery() }; secure=true }
        }
    }
    fun authenticate() {
        val guard=context.getSystemService(KeyguardManager::class.java)
        @Suppress("DEPRECATION")
        val intent=guard.createConfirmDeviceCredentialIntent(if(page=="Delete Account") "Delete Account" else uiResources.getString(R.string.ui_recovery_key),uiResources.getString(R.string.ui_confirm_it_s_you))
        if(intent!=null) auth.launch(intent) else model.error(uiResources.getString(R.string.ui_set_a_screen_lock_in_android_settings_to_continue))
    }
    val passkeyConfigured by produceState(false, s.secured, secure, passkeyRevision, page) {
        if (!secure && s.secured && page in setOf("Account Recovery", "Delete Account")) {
            value = withContext(Dispatchers.IO) { runCatching { repo.hasRecoveryPasskey() }.getOrDefault(false) }
        }
    }
    val dark = isSystemInDarkTheme()
    val grouped = settingsGrouped()
    val foreground = if (dark) Color.White else Color.Black
    ModalBottomSheet(
        onDismissRequest = onClose,
        properties = ModalBottomSheetProperties(shouldDismissOnBackPress = page == "Profile"),
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = grouped,
        contentColor = foreground,
        dragHandle = null,
    ) {
        androidx.activity.compose.PredictiveBackHandler(enabled = page != "Profile") { events ->
            events.collect { }
            goBack()
        }
        Column(Modifier.fillMaxWidth().imePadding()) {
            if (page != "Profile") {
                val title = when (page) {
                    "Account" -> s.username.orEmpty()
                    "Account Recovery" -> stringResource(R.string.account_recovery)
                    "Contacts" -> stringResource(R.string.ui_contacts)
                    "Appearance" -> stringResource(R.string.ui_appearance)
                    "Blocked" -> stringResource(R.string.ui_blocked)
                    "Contact Us" -> stringResource(R.string.ui_yo_team)
                    "Crash Reports" -> stringResource(R.string.ui_crash_reports)
                    "Delete Account" -> stringResource(R.string.ui_delete_account)
                    "Encryption" -> stringResource(R.string.ui_encryption)
                    "Help" -> stringResource(R.string.help_and_about)
                    "Licenses" -> stringResource(R.string.about_and_licenses)
                    "Media" -> stringResource(R.string.ui_media)
                    "Statistics" -> stringResource(R.string.ui_statistics)
                    "Status" -> stringResource(R.string.ui_status)
                    else -> page
                }
                CenterAlignedTopAppBar(
                    title = { Text(title, fontSize = 17.sp, fontWeight = FontWeight.SemiBold, maxLines = 1,
                        overflow = TextOverflow.Ellipsis) },
                    navigationIcon = {
                        IconButton(onClick = goBack) {
                            Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.ui_back))
                        }
                    },
                    expandedHeight = 48.dp,
                    windowInsets = WindowInsets(0, 0, 0, 0),
                    colors = TopAppBarDefaults.topAppBarColors(
                        containerColor = grouped, titleContentColor = foreground, navigationIconContentColor = foreground,
                    ),
                )
            }
            key(page) {
            LazyColumn(
                Modifier.fillMaxWidth().weight(1f, fill = page == "Contact Us"),
                contentPadding = PaddingValues(horizontal = 20.dp, vertical = 20.dp),
                verticalArrangement = Arrangement.spacedBy(if (page == "Profile") 20.dp else 28.dp),
            ) {
                when(page) {
                    "Profile" -> {
                        item { SettingsSection {
                            Row(
                                Modifier.fillMaxWidth().padding(horizontal = 16.dp),
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(12.dp),
                            ) {
                                Row(
                                    Modifier.weight(1f).clickable { page = "Account" }.padding(vertical = 20.dp),
                                    horizontalArrangement = Arrangement.spacedBy(14.dp),
                                    verticalAlignment = Alignment.CenterVertically,
                                ) {
                                    SettingsAvatar(s.username.orEmpty(), 76)
                                    BasicText(
                                        s.username.orEmpty(),
                                        modifier = Modifier.weight(1f),
                                        style = TextStyle(fontFamily = YoFont, fontSize = 26.sp, color = LocalContentColor.current),
                                        maxLines = 1,
                                        softWrap = false,
                                        overflow = TextOverflow.Ellipsis,
                                        autoSize = TextAutoSize.StepBased(minFontSize = 13.sp, maxFontSize = 26.sp),
                                    )
                                    if (!s.secured) RecoveryBadge()
                                }
                                IconButton(onClick = { code = true }) {
                                    Icon(Icons.Default.QrCode, stringResource(R.string.ui_show_your_code), Modifier.size(22.dp))
                                }
                                SettingsChevron()
                            }
                        } }
                        item { SettingsSection(compact = true) {
                            SettingsLine(
                                title = stringResource(R.string.ui_app_icon), compact = true,
                                leading = { AppIconPreview(s.appIcon, 29, 7) },
                                onClick = { page = "Appearance" },
                            )
                            SettingsDivider()
                            SettingToggle(stringResource(R.string.ui_left_handed), s.leftHanded,
                                icon = Icons.Default.PanTool, tileColor = TileBlue, compact = true) {
                                repo.command { settings(left = it) }
                            }
                        } }
                        item { SettingsSection(compact = true) {
                            SettingRow(stringResource(R.string.help_and_about), Icons.Default.Help, compact = true) { page = "Help" }
                            SettingsDivider()
                            SettingRow(stringResource(R.string.ui_add_friends), Icons.Default.PersonAdd, TileGreen,
                                chevron = false, compact = true, onClick = onAddFriends)
                        } }
                    }
                    "Account" -> {
                        item { SettingsSection {
                            SettingRow(stringResource(R.string.ui_contacts), Icons.Default.People, TileGreen,
                                detail = contacts.size.toString()) { page = "Contacts" }
                            SettingsDivider()
                            SettingRow(stringResource(R.string.ui_blocked), Icons.Default.Block, TileRed,
                                detail = s.friends.count { it.blocked }.toString()) { page = "Blocked" }
                        } }
                        item { SettingsSection(footerContent = {
                            Row(Modifier.clickable { deleting = ""; page = "Delete Account" }
                                .heightIn(min = 48.dp), verticalAlignment = Alignment.CenterVertically) {
                                Text(stringResource(R.string.settings_leaving), fontSize = 13.sp, lineHeight = 18.sp, color = settingsSecondary())
                                Text(stringResource(R.string.settings_delete_your_account), fontSize = 13.sp, color = Amethyst)
                            }
                        }) {
                            SettingRow(stringResource(R.string.account_recovery), Icons.Default.Lock, TileOrange,
                                badge = !s.secured) { page = "Account Recovery" }
                            SettingsDivider()
                            SettingRow(stringResource(R.string.ui_sign_out), Icons.Default.Logout, TileGray,
                                chevron = false) { logout = true }
                        } }
                        item { SettingsSection {
                            SettingRow(stringResource(R.string.ui_media), Icons.Default.PhotoLibrary, TileGreen) { page = "Media" }
                            SettingsDivider()
                            SettingRow(stringResource(R.string.ui_encryption), Icons.Default.Lock, TileGreen,
                                detail = encrypted?.let { "${it.first}/${it.second}" }) { encryptionFromStatus=false;page="Encryption" }
                            SettingsDivider()
                            SettingRow(stringResource(R.string.ui_statistics), Icons.Default.BarChart, TileGray,
                                detail = (s.sent + s.received).toString()) { page = "Statistics" }
                            SettingsDivider()
                            SettingToggle(stringResource(R.string.ui_requests_from_strangers), s.requestsOpen,
                                icon = Icons.Default.PersonAdd, tileColor = TileBlue) { repo.command { settings(requests = it) } }
                        } }
                    }
                    "Account Recovery" -> {
                        item { SettingsSection(footer = stringResource(R.string.account_backup_footer)) {
                            SettingRow(stringResource(R.string.account_backup), Icons.Default.CloudUpload, TileGreen, chevron = false) {
                                repo.command { saveContacts() }
                            }
                        } }
                        item { SettingsSection(footerContent = {
                            Text(
                                stringResource(R.string.how_account_recovery_works),
                                Modifier.clickable { recoveryHelp = true }.heightIn(min = 24.dp),
                                fontSize = 13.sp, color = Amethyst,
                            )
                        }) {
                            SettingToggle(stringResource(R.string.recovery_passkey), passkeyConfigured,
                                icon = Icons.Default.Fingerprint, checkedAction = { secure = true }) { enabled ->
                                if (enabled) { repo.command { prepareRecovery() }; secure = true }
                                else removePasskey = true
                            }
                            SettingsDivider()
                            SettingToggle(stringResource(R.string.ui_recovery_key), s.secured, icon = Icons.Default.Key,
                                checkedAction = { authenticate() }) { enabled ->
                                if (enabled) { repo.command { prepareRecovery() }; secure = true }
                                else renew = true
                            }
                        } }
                    }
                    "Blocked" -> {
                        val blocked=s.friends.filter { it.blocked }
                        if(blocked.isEmpty()) item { Text(stringResource(R.string.ui_no_blocked_friends),modifier=Modifier.fillMaxWidth().padding(32.dp),textAlign=androidx.compose.ui.text.style.TextAlign.Center) }
                        else item { SettingsSection(footer=uiResources.getString(R.string.ui_blocked_friends_can_t_send_you_yos_tap_unblock_to_unblock)) {
                            blocked.forEach { friend -> ListItem(colors=ListItemDefaults.colors(containerColor=Color.Transparent),headlineContent={Text(friend.name)},trailingContent={TextButton(onClick={repo.command { block(friend,false) }}){Text(stringResource(R.string.ui_unblock))}}) }
                        } }
                    }
                    "Media" -> item { SettingsSection(footer=uiResources.getString(R.string.ui_saves_photos_and_videos_you_capture_to_your_photo_library_on_andr)) {
                        SettingToggle(uiResources.getString(R.string.ui_keep_sent_photos),s.saveSent){repo.command { settings(save=it) }}
                    } }
                    "Appearance" -> {
                        item {
                            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                listOf(null, "Negative", "Original").forEach { name ->
                                    val label = stringResource(when (name) {
                                        "Negative" -> R.string.icon_negative
                                        "Original" -> R.string.icon_original
                                        else -> R.string.icon_default
                                    })
                                    val selectedIcon = s.appIcon == name
                                    val shape = RoundedCornerShape(20.dp)
                                    Column(
                                        Modifier.weight(1f).clip(shape)
                                            .border(2.dp, if (selectedIcon) Amethyst else Color.Transparent, shape)
                                            .selectable(selected = selectedIcon, onClick = { repo.command { icon(name) } })
                                            .padding(vertical = 10.dp),
                                        horizontalAlignment = Alignment.CenterHorizontally,
                                        verticalArrangement = Arrangement.spacedBy(8.dp),
                                    ) {
                                        AppIconPreview(name, 64, 14)
                                        Text(label, fontSize = 15.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                    }
                                }
                            }
                        }
                        item { SettingsSection(footer=uiResources.getString(R.string.ui_yo_and_photos_at_the_left_end_of_a_bar_undo_at_the_right_the_came)) {
                            SettingToggle(stringResource(R.string.ui_left_handed),s.leftHanded,
                                icon = Icons.Default.PanTool, tileColor = TileBlue) { repo.command { settings(left=it) } }
                        } }
                    }
                    "Statistics" -> {
                        val top=s.byFriend.maxByOrNull { it.value }
                        if(top!=null) item { SettingsSection { ValueRow(stringResource(R.string.top_friend),s.friends.firstOrNull { it.key==top.key }?.name.orEmpty());ValueRow(stringResource(R.string.yos_together),top.value.toString()) } }

                        item { SettingsSection(footer=uiResources.getString(R.string.ui_counted_on_this_phone)) { ValueRow(uiResources.getString(R.string.ui_yos_sent),s.sent.toString());ValueRow(uiResources.getString(R.string.ui_yos_received),s.received.toString());ValueRow(uiResources.getString(R.string.ui_friends),s.friends.count { !it.blocked }.toString()) } }
                    }
                    "Help" -> {
                        item {
                            Surface(shape = RoundedCornerShape(24.dp), color = Amethyst, contentColor = Color.White) {
                                YoTutorial(Modifier.fillMaxWidth().height(330.dp), compact = true)
                            }
                        }
                        item { SettingsSection {
                            SettingRow(stringResource(R.string.ui_contact_us), Icons.Default.Email, TileBlue) { page = "Contact Us" }
                        } }
                        item { SettingsSection {
                            SettingRow(stringResource(R.string.ui_status), Icons.Default.CheckCircle, TileGreen,
                                detail = stringResource(if (s.lastSync != null && System.currentTimeMillis() - s.lastSync < 60000)
                                    R.string.ui_connected else R.string.ui_waiting)) { page = "Status" }
                            SettingsDivider()
                            SettingRow(
                                stringResource(R.string.about_and_licenses),
                                Icons.Default.Info,
                                TileGray,
                                detail = stringResource(R.string.about_version, BuildConfig.VERSION_NAME),
                            ) { page = "Licenses" }
                        } }
                    }
                    "Status" -> {
                        item { SettingsSection(title=uiResources.getString(R.string.ui_connection)) {
                            val connected = s.lastSync != null && System.currentTimeMillis() - s.lastSync < 60000
                            ValueRow(stringResource(R.string.ui_hub),
                                stringResource(if (connected) R.string.ui_connected else R.string.ui_waiting), connected)
                            SettingsDivider()
                            ValueRow(stringResource(R.string.ui_round_trip), trip ?: stringResource(R.string.ui_not_checked))
                            SettingsDivider()
                            SettingsLine(
                                title = stringResource(if (probing) R.string.ui_checking else R.string.ui_check_round_trip),
                                onClick = if (probing) null else {
                                    {
                                        probing = true
                                        repo.command {
                                            try { trip = roundTrip()?.let { "$it ms" } ?: uiResources.getString(R.string.ui_no_reply) }
                                            finally { probing = false }
                                        }
                                    }
                                },
                                chevron = false,
                            )
                        } }
                        item { SettingsSection(footer=uiResources.getString(R.string.ui_anonymous_crash_reports_include_android_version_os_exit_reason_an)) {
                            SettingRow(uiResources.getString(R.string.ui_crash_reports),Icons.Default.Description){page="Crash Reports"}
                            if (android.os.Build.VERSION.SDK_INT >= 33 && !notificationsEnabled) {
                                SettingRow(stringResource(R.string.enable_yo_notifications), Icons.Default.Notifications) {
                                    notificationPermission.launch(android.Manifest.permission.POST_NOTIFICATIONS)
                                }
                            }
                            SettingsDivider()
                            SettingToggle(stringResource(R.string.ui_send_crashes), crashOn, icon = Icons.Default.BugReport) {
                                crashOn = it
                                repo.crashes.enabled = it
                                if (it) repo.command { crashes.flush() }
                            }
                        } }
                        item { SettingsSection {
                            SettingRow(stringResource(R.string.ui_encryption),Icons.Default.Lock){encryptionFromStatus=true;page="Encryption"}
                            SettingsDivider()
                            ValueRow(uiResources.getString(R.string.ui_version), BuildConfig.VERSION_NAME)
                        } }
                    }
                    "Crash Reports" -> item {
                        val reports=remember { repo.crashes.history() }
                        if (reports.isEmpty()) Text(
                            stringResource(R.string.settings_no_crash_reports),
                            Modifier.fillMaxWidth().padding(32.dp), textAlign = TextAlign.Center,
                        ) else SettingsSection { reports.forEach { text -> SelectionContainerCompat(text) } }
                    }
                    "Contact Us" -> {
                        item { LaunchedEffect(supportRetry) {
                            while(true) {
                                try { support=withContext(Dispatchers.IO) { repo.supportThread() };supportOffline=false
                                    support?.messages?.lastOrNull()?.let { withContext(Dispatchers.IO) { repo.supportRead(it.id) } }
                                } catch(e: kotlinx.coroutines.CancellationException) { throw e } catch(_: Exception) { supportOffline=true }
                                delay(5000)
                            }
                        } }
                        if(supportOffline) item { Text(stringResource(R.string.support_offline));TextButton(onClick={supportRetry++}){Text(stringResource(R.string.retry))} }
                        if (support?.messages.isNullOrEmpty()) item {
                            Column(
                                Modifier.fillMaxWidth().padding(horizontal = 32.dp, vertical = 64.dp),
                                horizontalAlignment = Alignment.CenterHorizontally,
                                verticalArrangement = Arrangement.spacedBy(10.dp),
                            ) {
                                Icon(Icons.Default.Forum, null, Modifier.size(44.dp), tint = Amethyst)
                                Text(stringResource(R.string.support_empty_title), fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
                                Text(stringResource(R.string.support_empty_subtitle), fontSize = 15.sp, lineHeight = 20.sp,
                                    color = settingsSecondary(), textAlign = TextAlign.Center)
                            }
                        }
                        items(support?.messages.orEmpty(), key = { it.id.toString() }) { message ->
                            Box(
                                Modifier.fillMaxWidth(),
                                contentAlignment = if (message.fromTeam) Alignment.CenterStart else Alignment.CenterEnd,
                            ) {
                                Surface(
                                    shape = RoundedCornerShape(20.dp),
                                    color = if (message.fromTeam) settingsInputFill() else Amethyst,
                                    contentColor = if (message.fromTeam) foreground else Color.White,
                                ) { Text(message.text, Modifier.padding(horizontal = 16.dp, vertical = 9.dp)) }
                            }
                        }
                    }
                    "Delete Account" -> {
                        item {
                            Column(
                                Modifier.fillMaxWidth().padding(vertical = 16.dp),
                                horizontalAlignment = Alignment.CenterHorizontally,
                                verticalArrangement = Arrangement.spacedBy(16.dp),
                            ) {
                                Icon(Icons.Default.Warning, null, Modifier.size(56.dp), tint = TileRed)
                                Text(stringResource(R.string.settings_delete_your_account), fontSize = 34.sp,
                                    lineHeight = 40.sp, fontWeight = FontWeight.Bold, textAlign = TextAlign.Center)
                            }
                        }
                        item { SettingsSection {
                            DeleteConsequence(uiResources.getString(R.string.name_freed, s.username.orEmpty()), Icons.Default.PersonOff)
                            SettingsDivider()
                            DeleteConsequence(stringResource(R.string.ui_your_friends_can_t_reach_you_anymore), Icons.Default.GroupOff)
                            SettingsDivider()
                            DeleteConsequence(stringResource(R.string.ui_this_can_t_be_undone), Icons.Default.WarningAmber)
                            if (passkeyConfigured) {
                                SettingsDivider()
                                DeleteConsequence(stringResource(R.string.settings_delete_passkey_yourself), Icons.Default.Key)
                            }
                        } }
                        item { SettingsSection {
                            SettingsTextField(
                                deleting, { deleting = it },
                                placeholder = uiResources.getString(R.string.type_to_confirm, s.username.orEmpty()),
                            )
                        } }
                        item {
                            val confirmed = deleting.isNotEmpty() && usernameSkeleton(deleting) == usernameSkeleton(s.username.orEmpty())
                            SettingsSection {
                                TextButton(
                                    onClick = ::authenticate, enabled = confirmed,
                                    modifier = Modifier.fillMaxWidth().heightIn(min = 54.dp),
                                    colors = ButtonDefaults.textButtonColors(contentColor = TileRed,
                                        disabledContentColor = settingsSecondary().copy(alpha = 0.4f)),
                                ) { Text(stringResource(R.string.ui_delete_account), fontSize = 17.sp) }
                            }
                        }
                    }
                    "Encryption" -> {
                        item { SettingsSection {
                            LaunchedEffect(Unit) { status=withContext(Dispatchers.IO){repo.security()} }
                            Text(stringResource(R.string.ui_every_yo_photo_and_video_is_end_to_end_encrypted_only_you_an),modifier=Modifier.padding(16.dp))
                            status?.let { st ->
                                val connected = s.lastSync != null && System.currentTimeMillis() - s.lastSync < 60000
                                ValueRow(stringResource(R.string.ui_hub_connected),
                                    stringResource(if (connected) R.string.ui_connected else R.string.ui_waiting), connected)
                                SettingsDivider()
                                ValueRow(stringResource(R.string.ui_hub_signed),
                                    stringResource(if (st.hubSigned) R.string.ui_yes else R.string.ui_not_checked), st.hubSigned)
                                SettingsDivider()
                                ValueRow(uiResources.getString(R.string.ui_post_quantum),uiResources.getString(R.string.part_of_total ,st.pqSessions.toLong(), st.devices.toLong()))
                                SettingsDivider()
                                ValueRow(uiResources.getString(R.string.ui_keys_at_hub),st.prekeysAtHub.toString())
                                SettingsDivider()
                                ValueRow(stringResource(R.string.key_age),st.signedPrekeyDays.toString())
                                SettingsDivider()
                                ValueRow(stringResource(R.string.refill_pending),if(st.refillPending) stringResource(R.string.ui_yes) else stringResource(R.string.ui_no))
                                SettingsDivider()
                                ValueRow(uiResources.getString(R.string.ui_verified_contacts),uiResources.getString(R.string.part_of_total,st.verified.toLong(), st.contacts.toLong()))
                            }
                        }
                        }
                    }
                    "Contacts" -> {
                        item { SettingsSection {
                            contacts.forEachIndexed { index, friend ->
                                if (index > 0) SettingsDivider()
                                val fact = facts[friend.key]
                                SettingsLine(
                                    title = friend.name,
                                    leading = { SettingsAvatar(friend.name, 36) },
                                    accessory = {
                                        if (fact?.unconfirmed == true || friend.gone || fact?.outdated == true) {
                                            Icon(Icons.Default.ErrorOutline, null, Modifier.size(18.dp), tint = TileOrange)
                                        } else if (fact?.verified == true) {
                                            Icon(Icons.Default.Verified, stringResource(R.string.ui_verified), Modifier.size(18.dp), tint = TileGreen)
                                        }
                                    },
                                    onClick = { selected = friend },
                                )
                            }
                        } }
                    }
                    "Licenses" -> {
                        item { SettingsSection { LicenseIntro() } }
                        itemsIndexed(licenseChunks,key={index,_ -> "license-$index"}) { _,text ->
                            SettingsSection { Text(text, Modifier.padding(16.dp), fontFamily=FontFamily.Monospace,fontSize=11.sp,lineHeight=14.3.sp) }
                        }
                    }
                }
            }
            }
            if (page == "Contact Us") {
                SupportComposer(feedback, { feedback = utf8Limit(it, 2000) }, sendingFeedback) {
                    val text = feedback.trim()
                    sendingFeedback = true
                    repo.command {
                        try {
                            feedback(text)
                            val updated = supportThread()
                            withContext(Dispatchers.Main) { feedback = ""; support = updated }
                        } finally {
                            withContext(Dispatchers.Main) { sendingFeedback = false }
                        }
                    }
                }
            }
        }
    }
    selected?.let { friend ->
        ModalBottomSheet(
            onDismissRequest = { selected = null },
            sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
            containerColor = grouped,
            contentColor = foreground,
        ) {
            Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp)) {
                Row(Modifier.fillMaxWidth().padding(top = 12.dp), horizontalArrangement = Arrangement.spacedBy(6.dp, Alignment.CenterHorizontally),
                    verticalAlignment = Alignment.CenterVertically) {
                    Text(friend.name, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                    Icon(Icons.Default.Lock, null, Modifier.size(13.dp), tint = settingsSecondary())
                }
                ContactPage(model, friend)
            }
        }
    }
    if (recoveryHelp) AlertDialog(
        onDismissRequest = { recoveryHelp = false },
        title = { Text(stringResource(R.string.how_account_recovery_works)) },
        text = { Text(stringResource(R.string.account_recovery_explanation)) },
        confirmButton = { TextButton(onClick = { recoveryHelp = false }) { Text(stringResource(R.string.ui_done)) } },
    )
    if(secure) RecoverySheet(model,s){secure=false}
    if(code) ModalBottomSheet(onDismissRequest={code=false}) { Column(Modifier.fillMaxWidth().padding(32.dp),horizontalAlignment=Alignment.CenterHorizontally) { Text(stringResource(R.string.ui_your_code),style=MaterialTheme.typography.titleLarge);Qr("https://justyo.co/${Uri.encode(s.username)}",Modifier.padding(24.dp).size(240.dp));BrandText(s.username.orEmpty(),30) } }
    if(logout) AlertDialog(onDismissRequest={logout=false},title={Text(stringResource(R.string.ui_sign_out_cd2476))},text={Text(if(s.secured) uiResources.getString(R.string.ui_sign_back_in_with_your_recovery_key_friends_on_this_phone_are_rem) else uiResources.getString(R.string.ui_your_account_isn_t_secured_without_a_recovery_key_you_won_t_be_ab))},
        confirmButton={TextButton(onClick={logout=false;repo.command { signOut();onClose() }}){Text(if(s.secured) uiResources.getString(R.string.ui_sign_out) else uiResources.getString(R.string.ui_sign_out_anyway))}},dismissButton={TextButton(onClick={logout=false}){Text(stringResource(R.string.ui_cancel))}})
    if (removePasskey) AlertDialog(
        onDismissRequest = { removePasskey = false },
        title = { Text(stringResource(R.string.settings_turn_off_passkey)) },
        confirmButton = { TextButton(onClick = {
            removePasskey = false
            repo.command { removeRecoveryPasskey(); withContext(Dispatchers.Main) { passkeyRevision++ } }
        }) { Text(stringResource(R.string.settings_turn_off)) } },
        dismissButton = { TextButton(onClick = { removePasskey = false }) { Text(stringResource(R.string.ui_cancel)) } },
    )
    if(renew) AlertDialog(onDismissRequest={renew=false},title={Text(stringResource(R.string.ui_create_new_recovery_key_ccd8c2))},text={Text(stringResource(R.string.ui_your_current_recovery_key_will_stop_working_after_the_new_on))},confirmButton={TextButton(onClick={renew=false;repo.command { prepareRecovery(true) };secure=true}){Text(stringResource(R.string.ui_create_new_recovery_key))}},dismissButton={TextButton(onClick={renew=false}){Text(stringResource(R.string.ui_cancel))}})
}
private val TileOrange=Color(0xFFFF9500)
private val TileGreen=Color(0xFF34C759)
private val TileRed=Color(0xFFFF3B30)
private val TileBlue=Color(0xFF007AFF)
private val TileGray=Color(0xFF8E8E93)

@Composable
internal fun settingsGrouped() = if (isSystemInDarkTheme()) Color.Black else Color(0xFFF2F2F7)

@Composable
internal fun settingsInputFill() = Color(0xFF767680).copy(alpha = if (isSystemInDarkTheme()) 0.24f else 0.12f)

@Composable
private fun RecoveryBadge() {
    Surface(Modifier.sizeIn(minWidth = 20.dp, minHeight = 20.dp), shape = androidx.compose.foundation.shape.CircleShape,
        color = TileRed, contentColor = Color.White) {
        Box(contentAlignment = Alignment.Center) {
            Text("1", fontSize = 13.sp, lineHeight = 16.sp, fontWeight = FontWeight.SemiBold)
        }
    }
}

@Composable
private fun AppIconPreview(name: String?, size: Int, radius: Int) {
    val resource = if (name == "Negative") R.drawable.icon_negative else R.drawable.launcher_wordmark
    val background = if (name == "Original") Color(0xFF9A44BE) else Amethyst
    Image(
        androidx.compose.ui.res.painterResource(resource), null,
        Modifier.size(size.dp).clip(RoundedCornerShape(radius.dp)).background(background),
    )
}

@Composable
private fun DeleteConsequence(text: String, icon: ImageVector) {
    Row(
        Modifier.fillMaxWidth().heightIn(min = 54.dp).padding(horizontal = 16.dp, vertical = 5.dp),
        horizontalArrangement = Arrangement.spacedBy(14.dp), verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(icon, null, Modifier.size(21.dp))
        Text(text, fontSize = 17.sp, lineHeight = 22.sp)
    }
}

@Composable
private fun SupportComposer(value: String, change: (String) -> Unit, sending: Boolean, send: () -> Unit) {
    Column(
        Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, bottom = 12.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Text(stringResource(R.string.support_privacy), fontSize = 11.sp, lineHeight = 14.sp,
            color = settingsSecondary(), textAlign = TextAlign.Center)
        Row(
            Modifier.fillMaxWidth().background(settingsInputFill(), RoundedCornerShape(20.dp))
                .padding(start = 16.dp, end = 4.dp),
            verticalAlignment = Alignment.Bottom,
        ) {
            val placeholder = stringResource(R.string.support_message)
            BasicTextField(
                value, change, Modifier.weight(1f).padding(vertical = 9.dp)
                    .semantics { contentDescription = placeholder },
                enabled = !sending, minLines = 1, maxLines = 6,
                textStyle = TextStyle(color = LocalContentColor.current, fontSize = 17.sp, lineHeight = 22.sp),
                cursorBrush = SolidColor(Amethyst),
                decorationBox = { field ->
                    Box(Modifier.fillMaxWidth()) {
                        if (value.isEmpty()) Text(placeholder, fontSize = 17.sp, color = settingsSecondary())
                        field()
                    }
                },
            )
            val canSend = value.isNotBlank() && !sending
            IconButton(onClick = send, enabled = canSend, modifier = Modifier.size(40.dp)) {
                Surface(Modifier.size(30.dp), shape = androidx.compose.foundation.shape.CircleShape,
                    color = Amethyst.copy(alpha = if (canSend) 1f else 0.35f), contentColor = Color.White) {
                    Box(contentAlignment = Alignment.Center) {
                        if (sending) CircularProgressIndicator(Modifier.size(15.dp), color = Color.White, strokeWidth = 2.dp)
                        else Icon(Icons.Default.ArrowUpward, stringResource(R.string.ui_send), Modifier.size(17.dp))
                    }
                }
            }
        }
    }
}

@Composable
internal fun SettingsTextField(
    value: String, change: (String) -> Unit, placeholder: String,
    modifier: Modifier = Modifier, height: Int = 54, enabled: Boolean = true,
    keyboardOptions: androidx.compose.foundation.text.KeyboardOptions = androidx.compose.foundation.text.KeyboardOptions.Default,
    keyboardActions: androidx.compose.foundation.text.KeyboardActions = androidx.compose.foundation.text.KeyboardActions.Default,
    leading: (@Composable () -> Unit)? = null,
) {
    BasicTextField(
        value, change, singleLine = true, enabled = enabled, keyboardOptions = keyboardOptions,
        keyboardActions = keyboardActions,
        modifier = modifier.fillMaxWidth().semantics { contentDescription = placeholder },
        textStyle = TextStyle(fontSize = 17.sp, color = LocalContentColor.current),
        cursorBrush = SolidColor(Amethyst),
        decorationBox = { field ->
            Row(Modifier.fillMaxWidth().heightIn(min = height.dp).padding(horizontal = 16.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                leading?.invoke()
                Box(Modifier.weight(1f)) {
                    if (value.isEmpty()) Text(placeholder, fontSize = 17.sp, color = settingsSecondary(), maxLines = 1,
                        overflow = TextOverflow.Ellipsis)
                    field()
                }
            }
        },
    )
}

@Composable
internal fun settingsSecondary() = LocalContentColor.current.copy(alpha = 0.6f)

@Composable
internal fun SettingsDivider() {
    HorizontalDivider(Modifier.padding(horizontal = 16.dp), thickness = 0.5.dp,
        color = LocalContentColor.current.copy(alpha = 0.18f))
}

@Composable
private fun SettingsChevron() {
    val shade = if (isSystemInDarkTheme()) Color(0xFFEBEBF5) else Color(0xFF3C3C43)
    val tint = shade.copy(alpha = 0.30f)
    val direction = androidx.compose.ui.platform.LocalLayoutDirection.current
    Canvas(Modifier.size(width = 8.dp, height = 13.dp)) {
        val right = direction == androidx.compose.ui.unit.LayoutDirection.Ltr
        val start = if (right) 1.dp.toPx() else size.width - 1.dp.toPx()
        val end = if (right) size.width - 1.dp.toPx() else 1.dp.toPx()
        val path = androidx.compose.ui.graphics.Path().apply {
            moveTo(start, 1.dp.toPx())
            lineTo(end, size.height / 2)
            lineTo(start, size.height - 1.dp.toPx())
        }
        drawPath(path, tint, style = androidx.compose.ui.graphics.drawscope.Stroke(
            width = 1.6.dp.toPx(), cap = androidx.compose.ui.graphics.StrokeCap.Round,
            join = androidx.compose.ui.graphics.StrokeJoin.Round,
        ))
    }
}

@Composable
internal fun SettingsSection(
    title: String? = null,
    footer: String? = null,
    compact: Boolean = false,
    footerContent: (@Composable () -> Unit)? = null,
    content: @Composable ColumnScope.() -> Unit,
) {
    val dark = isSystemInDarkTheme()
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (title != null) Text(title, Modifier.padding(horizontal = 16.dp), fontSize = 13.sp, lineHeight = 18.sp, color = settingsSecondary())
        Surface(
            Modifier.fillMaxWidth(), shape = RoundedCornerShape(26.dp),
            color = if (dark) Color(0xFF1C1C1E) else Color.White,
            contentColor = if (dark) Color.White else Color.Black,
        ) {
            Column(Modifier.padding(vertical = if (compact) 12.dp else 0.dp), content = content)
        }
        if (footer != null) Text(footer, Modifier.padding(horizontal = 16.dp), fontSize = 13.sp, lineHeight = 18.sp, color = settingsSecondary())
        if (footerContent != null) Box(Modifier.padding(horizontal = 16.dp)) { footerContent() }
    }
}

@Composable
internal fun SettingsAvatar(name: String, diameter: Int) {
    val seed = remember(name) { usernameSkeleton(name).codePoints().sum() }
    val initial = remember(name) {
        if (name.isEmpty()) "?" else name.substring(0, name.offsetByCodePoints(0, 1)).uppercase()
    }
    Surface(Modifier.size(diameter.dp), shape = androidx.compose.foundation.shape.CircleShape,
        color = RowColors[Math.floorMod(seed, RowColors.size)]) {
        Box(contentAlignment = Alignment.Center) {
            Text(initial, style = TextStyle(fontFamily = YoFont, fontSize = (diameter * 0.55f).sp, color = Color.White))
        }
    }
}

@Composable
private fun SettingsTile(icon: ImageVector, color: Color) {
    Surface(Modifier.size(29.dp), shape = RoundedCornerShape(7.dp), color = color, contentColor = Color.White) {
        Box(contentAlignment = Alignment.Center) { Icon(icon, null, Modifier.size(17.dp)) }
    }
}

@Composable
private fun SettingsLine(
    title: String,
    detail: String? = null,
    leading: (@Composable () -> Unit)? = null,
    accessory: (@Composable () -> Unit)? = null,
    chevron: Boolean = true,
    compact: Boolean = false,
    onClick: (() -> Unit)? = null,
) {
    Row(
        Modifier.fillMaxWidth().then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier)
            .heightIn(min = if (compact) 48.dp else 54.dp).padding(horizontal = 16.dp, vertical = 5.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (leading != null) {
            leading()
            Spacer(Modifier.width(14.dp))
        }
        androidx.compose.ui.layout.Layout(
            modifier = Modifier.weight(1f),
            content = {
                BasicText(
                    title,
                    style = TextStyle(fontSize = 17.sp, lineHeight = 22.sp, color = LocalContentColor.current),
                    maxLines = 1, softWrap = false, overflow = TextOverflow.Ellipsis,
                    autoSize = TextAutoSize.StepBased(minFontSize = 14.sp, maxFontSize = 17.sp),
                )
                if (detail != null) Text(
                    detail, fontSize = 17.sp, color = settingsSecondary(), maxLines = 1,
                    softWrap = false, overflow = TextOverflow.Ellipsis,
                )
            },
        ) { items, constraints ->
            val available = constraints.maxWidth
            val textConstraints = constraints.copy(minWidth = 0, minHeight = 0)
            val label = items.first().measure(textConstraints)
            val detailWidth = (available - label.width - 8.dp.roundToPx()).coerceAtLeast(0)
            val value = items.getOrNull(1)?.measure(textConstraints.copy(maxWidth = detailWidth))
            val height = maxOf(label.height, value?.height ?: 0).coerceAtLeast(constraints.minHeight)
            layout(available, height) {
                label.placeRelative(0, (height - label.height) / 2)
                value?.placeRelative(available - value.width, (height - value.height) / 2)
            }
        }
        val showChevron = chevron && onClick != null
        if (accessory != null || showChevron) {
            Spacer(Modifier.width(8.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                accessory?.invoke()
                if (showChevron) SettingsChevron()
            }
        }
    }
}

@Composable
private fun SettingRow(
    title: String, icon: ImageVector, tileColor: Color = Amethyst, detail: String? = null,
    chevron: Boolean = true, compact: Boolean = false, badge: Boolean = false, onClick: () -> Unit,
) {
    SettingsLine(title, detail, leading = { SettingsTile(icon, tileColor) },
        accessory = if (badge) { { RecoveryBadge() } } else null,
        chevron = chevron, compact = compact, onClick = onClick)
}

@Composable
private fun SettingToggle(
    title: String, on: Boolean, icon: ImageVector? = null, tileColor: Color = Amethyst,
    compact: Boolean = false, detail: String? = null, checkedAction: (() -> Unit)? = null,
    changed: (Boolean) -> Unit,
) {
    SettingsLine(
        title, detail,
        leading = icon?.let { { SettingsTile(it, tileColor) } },
        accessory = {
            Switch(
                checked = on,
                modifier = Modifier.height(44.dp),
                onCheckedChange = changed,
                colors = SwitchDefaults.colors(
                    checkedTrackColor = Amethyst, checkedThumbColor = Color.White,
                    checkedBorderColor = Color.Transparent,
                    uncheckedTrackColor = if (isSystemInDarkTheme()) Color(0xFF636366) else Color(0xFFAEAEB2),
                    uncheckedThumbColor = Color.White, uncheckedBorderColor = Color.Transparent,
                ),
            )
        },
        chevron = false, compact = compact,
        onClick = if (on && checkedAction != null) checkedAction else { { changed(!on) } },
    )
}

@Composable
internal fun ValueRow(title: String, value: String, good: Boolean? = null) {
    SettingsLine(title, value, accessory = good?.let { ok ->
        { Icon(if (ok) Icons.Default.CheckCircle else Icons.Default.ErrorOutline, null,
            Modifier.size(17.dp), tint = if (ok) TileGreen else TileOrange) }
    })
}

@Composable private fun ContactPage(model: YoViewModel,friend: Friend) {
    val uiResources=androidx.compose.ui.platform.LocalResources.current

    var scanning by remember { mutableStateOf(false) }
    var safetyBytes by remember(friend.user) { mutableStateOf<ByteArray?>(null) }
    val contactScope=rememberCoroutineScope()
    var details by remember(friend.user) { mutableStateOf<ContactDetails?>(null) }
    var revision by remember { mutableIntStateOf(0) }
    var comparing by remember { mutableStateOf(false) }
    var technical by remember { mutableStateOf(false) }
    val contactState by model.repository.state.collectAsStateWithLifecycle()
    val current=contactState.friends.firstOrNull { it.key==friend.key } ?: friend
    LaunchedEffect(friend.user,revision) { details=withContext(Dispatchers.IO){model.repository.contact(friend)}
        if(details?.safetyNumber!=null) safetyBytes=withContext(Dispatchers.IO){model.repository.safetyCode(friend)} }
    Column(Modifier.fillMaxWidth().padding(vertical = 20.dp), verticalArrangement = Arrangement.spacedBy(20.dp)) {
        details?.let { d ->
            SettingsSection {
                val number = d.safetyNumber
                if (number != null) {
                    SettingRow(stringResource(R.string.compare_code), Icons.Default.QrCode) { comparing = !comparing }
                    if (comparing) Column(Modifier.padding(16.dp)) {
                        Text(uiResources.getString(R.string.compare_safety, friend.name))
                        safetyBytes?.let { Qr(it.toString(Charsets.ISO_8859_1), Modifier.size(220.dp)) }
                        TextButton(onClick = { scanning = true }) { Text(stringResource(R.string.scan_qr)) }
                        SelectionContainerCompat(number.filter(Char::isDigit).chunked(5).chunked(3).joinToString("\n") { it.joinToString(" ") })
                    }
                    TextButton(onClick = { model.repository.command { verify(friend, d.contact?.verified != true); revision++ } }) {
                        Text(stringResource(if (d.contact?.verified == true) R.string.ui_mark_as_not_verified else R.string.ui_mark_as_verified))
                    }
                }
                if (d.contact?.unconfirmed == true) TextButton(onClick = { model.repository.command { confirm(friend); revision++ } }) {
                    Text(stringResource(R.string.ui_confirm_new_key))
                }
            }
            SettingsSection {
                SettingsLine(stringResource(R.string.technical_details), onClick = { technical = !technical })
                if (technical) {
                    SettingsDivider()
                    ValueRow(stringResource(R.string.ui_account), stringResource(if (d.contact?.gone == true) R.string.ui_deleted else R.string.ui_on_yo))
                    SettingsDivider()
                    ValueRow(stringResource(R.string.ui_identity_key), stringResource(if (d.contact?.unconfirmed == true) R.string.ui_changed_confirm_before_sending else R.string.ui_unchanged))
                    SettingsDivider()
                    Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text(stringResource(R.string.ui_sessions), fontSize = 17.sp)
                        SelectionContainerCompat(d.sessions.joinToString("\n") {
                            (if (it.usable) "✓" else uiResources.getString(R.string.ui_waiting)) + "  " + it.device
                        }.ifEmpty { uiResources.getString(R.string.ui_no_chat_yet) })
                        d.accountKey?.let {
                            Text(stringResource(R.string.ui_account_key), modifier = Modifier.padding(top = 10.dp))
                            SelectionContainerCompat(it)
                        }
                    }
                }
            }
        }
        SettingsSection {
            SettingToggle(stringResource(R.string.ui_undo_before_sending), current.undo) {
                model.repository.command { undoSettings(current, it, current.undoSeconds) }
            }
            if (current.undo) Column(Modifier.padding(horizontal = 16.dp)) {
                Text(uiResources.getQuantityString(R.plurals.undo_seconds, current.undoSeconds, current.undoSeconds))
                Slider(current.undoSeconds.toFloat(), { model.repository.command { undoSettings(current, true, it.toInt()) } },
                    valueRange = 2f..10f, steps = 7)
            }
        }
    }
    if(scanning) QrScanner(onClose={scanning=false},onCode={ qr ->
        scanning=false
        contactScope.launch {
            try {
                val matched=withContext(Dispatchers.IO) { model.repository.verifyScan(friend,qr.bytes) }
                if(matched) revision++ else model.error(uiResources.getString(R.string.safety_mismatch))
            } catch(e: CancellationException) { throw e } catch(e: Exception) { model.error(e.message ?: uiResources.getString(R.string.generic_failure)) }
        }
    })
}
@Composable private fun SelectionContainerCompat(text: String) { androidx.compose.foundation.text.selection.SelectionContainer { Text(text,fontFamily=FontFamily.Monospace,modifier=Modifier.padding(vertical=12.dp)) } }

@Composable fun RecoverySheet(model: YoViewModel,s: AppState,onClose: ()->Unit) {
    val uiResources=androidx.compose.ui.platform.LocalResources.current

    var shown by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    var saved by remember { mutableStateOf(false) }
    val context=LocalContext.current
    val save=rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("text/plain")) { uri ->
        if(uri!=null) model.repository.command {
            busy=true
            try {
                val key=state.value.recovery ?: error(uiResources.getString(R.string.ui_recovery_key_missing))
                context.contentResolver.openOutputStream(uri)?.use { it.write(uiResources.getString(R.string.ui_yo_recovery_key_for,state.value.username.orEmpty(),key.chunked(4).joinToString(" ")).toByteArray()) } ?: error(uiResources.getString(R.string.ui_couldn_t_save_the_recovery_key))
                keepRecovery();saved=true
            } finally { busy=false }
        }
    }
    val saveImage=rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("image/png")) { uri ->
        if(uri!=null) model.repository.command {
            busy=true
            try {
                val key=state.value.recovery ?: error(uiResources.getString(R.string.ui_recovery_key_missing))
                val bitmap=recoveryCard(context,state.value.username.orEmpty(),key)
                try {
                    context.contentResolver.openOutputStream(uri)?.use { stream -> check(bitmap.compress(Bitmap.CompressFormat.PNG,100,stream)) } ?: error(uiResources.getString(R.string.ui_couldn_t_save_recovery_card))
                } finally { bitmap.recycle() }
                keepRecovery();saved=true
            } finally { busy=false }
        }
    }
    val lifecycle=androidx.lifecycle.compose.LocalLifecycleOwner.current
    DisposableEffect(lifecycle) {
        val observer=androidx.lifecycle.LifecycleEventObserver { _,event -> if(event==androidx.lifecycle.Lifecycle.Event.ON_PAUSE) shown=false }
        lifecycle.lifecycle.addObserver(observer);onDispose { lifecycle.lifecycle.removeObserver(observer) }
    }
    val key = s.recovery
    val ready = key?.length == 64
    var sharing by remember { mutableStateOf(false) }
    val passkeyScope = rememberCoroutineScope()
    ModalBottomSheet(
        onDismissRequest = { if (!busy) onClose() },
        containerColor = Amethyst,
        contentColor = Color.White,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
    ) {
        LazyColumn(
            Modifier.fillMaxWidth().weight(1f, fill = false),
            contentPadding = PaddingValues(horizontal = 20.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            item {
                Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                    Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.padding(horizontal = 44.dp)) {
                        Icon(Icons.Default.Key, null, Modifier.size(72.dp))
                        Text(stringResource(R.string.set_up_account_recovery), style = MaterialTheme.typography.headlineSmall,
                            textAlign = androidx.compose.ui.text.style.TextAlign.Center)
                    }
                    IconButton(onClick = onClose, enabled = !busy, modifier = Modifier.align(Alignment.TopEnd)) {
                        Icon(Icons.Default.Close, stringResource(R.string.ui_close))
                    }
                }
            }
            item {
                RecoveryStep(1, stringResource(R.string.ui_recovery_key), titleAction = {
                    IconButton(onClick = { shown = !shown }, enabled = ready && !busy) {
                        Icon(if (shown) Icons.Default.VisibilityOff else Icons.Default.Visibility,
                            stringResource(if (shown) R.string.recovery_hide else R.string.recovery_show))
                    }
                }) {
                    TextButton(onClick = { sharing = true }, enabled = ready && !busy) {
                        Text(stringResource(if (saved) R.string.saved else R.string.ui_save), color = Color.White)
                    }
                }
            }
            if (shown && key != null) item {
                Surface(color = Color.White.copy(alpha = .14f), contentColor = Color.White, shape = RoundedCornerShape(20.dp)) {
                    Column(Modifier.fillMaxWidth().padding(16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                        Qr(key, Modifier.size(140.dp))
                        key.chunked(4).chunked(4).forEach { blocks ->
                            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceEvenly) {
                                blocks.forEach { Text(it, fontFamily = FontFamily.Monospace, fontSize = 17.sp) }
                            }
                        }
                    }
                }
            }
            item {
                RecoveryStep(2, stringResource(R.string.recovery_passkey)) {
                    TextButton(onClick = {
                        passkeyScope.launch {
                            busy = true
                            try {
                                val (name, user) = model.repository.passkeyAccount() ?: return@launch
                                val answer = Passkeys.create(context, name, user)
                                kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.IO) { model.repository.keepPasskey(answer) }
                                saved = true
                            } catch (e: kotlinx.coroutines.CancellationException) {
                                throw e
                            } catch (e: Exception) {
                                model.error(e.localizedMessage ?: uiResources.getString(R.string.passkey_prf_required))
                            } finally { busy = false }
                        }
                    }, enabled = !busy) {
                        Text(stringResource(R.string.recovery_set_up), color = Color.White)
                    }
                }
            }
        }
        Button(
            onClick = onClose,
            enabled = saved && !busy,
            modifier = Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 12.dp),
            colors = ButtonDefaults.buttonColors(containerColor = Color.White, contentColor = Color.Black),
        ) { Text(stringResource(R.string.ui_done)) }
    }
    if (sharing) AlertDialog(
        onDismissRequest = { sharing = false },
        title = { Text(stringResource(R.string.ui_recovery_key)) },
        text = {
            Column {
                Text(stringResource(R.string.ui_anyone_with_this_key_can_sign_in_as_you_keep_it_safe_and_don))
                if (key != null && ready) TextButton(onClick = {
                    sharing = false
                    val share = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(
                        Intent.EXTRA_TEXT,
                        uiResources.getString(R.string.ui_yo_recovery_key_for, s.username.orEmpty(), key.chunked(4).joinToString(" ")),
                    )
                    context.startActivity(Intent.createChooser(share, uiResources.getString(R.string.recovery_share)))
                }) { Text(stringResource(R.string.recovery_share)) }
            }
        },
        confirmButton = {
            TextButton(onClick = {
                sharing = false
                save.launch(uiResources.getString(R.string.ui_yo_recovery_key_txt))
            }) { Text(stringResource(R.string.ui_save)) }
        },
        dismissButton = {
            TextButton(onClick = {
                sharing = false
                saveImage.launch(uiResources.getString(R.string.ui_yo_recovery_key_png))
            }) { Text(stringResource(R.string.ui_save_image)) }
        },
    )
}

@Composable
private fun RecoveryStep(
    number: Int,
    title: String,
    titleAction: @Composable () -> Unit = {},
    actions: @Composable RowScope.() -> Unit,
) {
    Column {
        Text(stringResource(R.string.recovery_step, number), style = MaterialTheme.typography.labelMedium,
            modifier = Modifier.padding(start = 12.dp, bottom = 4.dp))
        Surface(color = Color.White.copy(alpha = .14f), contentColor = Color.White, shape = RoundedCornerShape(20.dp)) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically) {
                Text(title, Modifier.weight(1f, fill = false), fontFamily = YoFont)
                titleAction()
                Spacer(Modifier.weight(1f))
                actions()
            }
        }
    }
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
@Composable private fun LicenseIntro() {
    val context=LocalContext.current
    Column(Modifier.padding(16.dp)) {
        Text(stringResource(R.string.ui_yo_is_mit_licensed_this_app_includes_libsignal_licensed_unde))
        Text(stringResource(R.string.ui_montserrat_sil_open_font_license_androidx_kotlin_jna_okhttp_),modifier=Modifier.padding(vertical=16.dp))
    }
    SettingsDivider()
    SettingRow(stringResource(R.string.ui_source), Icons.Default.Code, TileGray) {
        context.startActivity(Intent(Intent.ACTION_VIEW, "https://github.com/yoagain/yo".toUri()))
    }
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

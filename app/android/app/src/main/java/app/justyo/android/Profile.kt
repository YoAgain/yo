@file:OptIn(androidx.compose.material3.ExperimentalMaterial3ExpressiveApi::class, androidx.compose.material3.ExperimentalMaterial3Api::class)
package app.justyo.android

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
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
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
    val colors=if(isSystemInDarkTheme()) darkColorScheme(primary=Amethyst) else lightColorScheme(primary=Amethyst)
    MaterialTheme(colorScheme=colors) { ProfileContent(model,s,onClose,onAddFriends) }
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
    var deleting by rememberSaveable { mutableStateOf("") }
    var feedback by rememberSaveable { mutableStateOf("") }
    var thanks by remember { mutableStateOf(false) }
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
    val goBack = {
        page = when (page) {
            "Contact Us", "Status", "Licenses" -> "Help"
            "Crash Reports" -> "Status"
            "Delete Account" -> "Account Recovery"
            else -> "Profile"
        }
    }
    var notificationGranted by remember { mutableStateOf(false) }
    val notificationPermission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { notificationGranted = it }
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
    ModalBottomSheet(onDismissRequest=onClose,sheetState=rememberModalBottomSheetState(skipPartiallyExpanded=true)) {
    androidx.activity.compose.PredictiveBackHandler(enabled=page!="Profile") { events -> events.collect { };goBack() }
        Column(Modifier.fillMaxWidth()) {
            TopAppBar(title={Text(when(page){"Account Recovery" -> uiResources.getString(R.string.account_recovery);"Contacts" -> uiResources.getString(R.string.ui_contacts);"Appearance" -> uiResources.getString(R.string.ui_appearance);"Blocked" -> uiResources.getString(R.string.ui_blocked);"Contact Us" -> uiResources.getString(R.string.ui_contact_us);"Crash Reports" -> uiResources.getString(R.string.ui_crash_reports);"Delete Account" -> uiResources.getString(R.string.ui_delete_account);"Encryption" -> uiResources.getString(R.string.ui_encryption);"Help" -> uiResources.getString(R.string.help_and_about);"Licenses" -> uiResources.getString(R.string.about_and_licenses);"Media" -> uiResources.getString(R.string.ui_media);"Profile" -> uiResources.getString(R.string.ui_profile);"Statistics" -> uiResources.getString(R.string.ui_statistics);"Status" -> uiResources.getString(R.string.ui_status);else -> page})},navigationIcon={if(page!="Profile") IconButton(onClick=goBack){Icon(Icons.AutoMirrored.Filled.ArrowBack,uiResources.getString(R.string.ui_back))}},actions={IconButton(onClick=onClose){Icon(Icons.Default.Close,uiResources.getString(R.string.ui_close))}})
            LazyColumn(Modifier.fillMaxWidth().weight(1f, fill = false).background(MaterialTheme.colorScheme.surfaceContainerLow),contentPadding=PaddingValues(start=16.dp,end=16.dp,top=8.dp,bottom=32.dp),verticalArrangement=Arrangement.spacedBy(20.dp)) {
                when(page) {
                    "Profile" -> {
                        item { SettingsSection {
                            ListItem(colors=ListItemDefaults.colors(containerColor=Color.Transparent),headlineContent={Text(s.username.orEmpty(),fontFamily=YoFont,fontSize=26.sp)},leadingContent={Surface(shape=androidx.compose.foundation.shape.CircleShape,color=Amethyst,contentColor=Color.White){BrandText(s.username?.take(1).orEmpty(),26,Modifier.padding(14.dp))}},trailingContent={IconButton(onClick={code=true}){Icon(Icons.Default.QrCode,uiResources.getString(R.string.ui_show_your_code))}})
                        } }
                        item { SettingsSection {
                            SettingRow(stringResource(R.string.ui_app_icon), Icons.Default.Palette, TileBlue) { page = "Appearance" }
                            SettingToggle(stringResource(R.string.ui_left_handed), s.leftHanded) { repo.command { settings(left = it) } }
                        } }
                        item { SettingsSection {
                            SettingRow(stringResource(R.string.help_and_about), Icons.Default.Help) { page = "Help" }
                            SettingRow(stringResource(R.string.ui_add_friends_by_name), Icons.Default.PersonAdd, TileGreen, onClick = onAddFriends)
                        } }
                        item { SettingsSection { SettingRow(stringResource(R.string.ui_invite_friends),Icons.Default.Share,Amethyst){shareInvite(context,s.username.orEmpty())} } }
                        if(!s.secured) item { SettingsSection(footer=uiResources.getString(R.string.secure_name)) {
                            SettingRow(stringResource(R.string.ui_secure_your_account_1),Icons.Default.Lock,TileOrange){repo.command { prepareRecovery() };secure=true}
                        } }
                        item { SettingsSection {
                            SettingRow(stringResource(R.string.account_recovery), Icons.Default.Key, if(s.secured) TileGreen else TileOrange) { page = "Account Recovery" }
                            SettingRow(uiResources.getString(R.string.blocked_count ,s.friends.count { it.blocked }),Icons.Default.Block,TileRed){page="Blocked"}
                        } }
                        item { SettingsSection(title=uiResources.getString(R.string.ui_contacts),footer=uiResources.getString(R.string.ui_your_encrypted_friend_list_and_contact_checks)) {
                            SettingRow(uiResources.getString(R.string.contacts_count ,s.friends.count { !it.blocked && it.group == null }),Icons.Default.People,TileGreen){page="Contacts"}
                            SettingRow(stringResource(R.string.ui_save_contacts),Icons.Default.CloudUpload,TileGreen){repo.command { saveContacts() }}
                        } }
                        item { SettingsSection {
                            SettingRow(stringResource(R.string.ui_media),Icons.Default.PhotoLibrary,TileGreen){page="Media"}
                            SettingRow(stringResource(R.string.ui_encryption)+(encrypted?.let { "  ·  ${it.first}/${it.second}" } ?: ""),Icons.Default.Lock,TileGreen){page="Encryption"}
                        } }
                        item { SettingsSection {
                            SettingRow(uiResources.getString(R.string.statistics_count ,s.sent+s.received),Icons.Default.BarChart,TileGray){page="Statistics"}
                        } }
                    }
                    "Account Recovery" -> {
                        item { SettingsSection(footer = stringResource(R.string.account_backup_footer)) {
                            SettingRow(stringResource(R.string.account_backup), Icons.Default.CloudUpload, TileGreen) {
                                repo.command { saveContacts() }
                            }
                        } }
                        item { SettingsSection {
                            SettingRow(stringResource(R.string.recovery_passkey), Icons.Default.Key, TileOrange,
                                detail = stringResource(R.string.google_password_manager)) {
                                repo.command { prepareRecovery() }
                                secure = true
                            }
                            SettingRow(uiResources.getString(R.string.recovery_saved ,if(s.secured) uiResources.getString(R.string.saved) else uiResources.getString(R.string.not_saved)),Icons.Default.Key,if(s.secured) TileGreen else TileOrange){if(s.secured) authenticate() else {repo.command { prepareRecovery() };secure=true}}
                            if(s.secured) SettingRow(stringResource(R.string.ui_create_new_recovery_key),Icons.Default.Refresh,TileGreen){renew=true}
                        } }
                        item { TextButton(onClick = { recoveryHelp = true }) {
                            Text(stringResource(R.string.how_account_recovery_works))
                        } }
                        item { SettingsSection(footer=uiResources.getString(R.string.ui_off_only_your_friends_can_send_you_yos_anyone_else_is_turned_away)) {
                            SettingToggle(uiResources.getString(R.string.ui_requests_from_strangers),s.requestsOpen){repo.command { settings(requests=it) }}
                        } }
                        item { Column(verticalArrangement=Arrangement.spacedBy(10.dp)) {
                            OutlinedButton(onClick={logout=true},modifier=Modifier.fillMaxWidth()){Text(stringResource(R.string.ui_sign_out),color=MaterialTheme.colorScheme.error)}
                            OutlinedButton(onClick={page="Delete Account"},modifier=Modifier.fillMaxWidth()){Text(stringResource(R.string.ui_delete_account),color=MaterialTheme.colorScheme.error)}
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
                        item { SettingsSection(title=uiResources.getString(R.string.ui_app_icon)) {
                        Row(Modifier.fillMaxWidth(),horizontalArrangement=Arrangement.SpaceEvenly) {
                            listOf(null to R.drawable.icon_standard, "Negative" to R.drawable.icon_negative, "Original" to R.drawable.icon_original).forEach { (name,resource) ->
                                Column(horizontalAlignment=Alignment.CenterHorizontally,modifier=Modifier.clickable { repo.command { icon(name) } }.padding(12.dp)) {
                                    Image(androidx.compose.ui.res.painterResource(resource),stringResource(when(name){"Negative" -> R.string.icon_negative;"Original" -> R.string.icon_original;else -> R.string.icon_default}),Modifier.size(64.dp))
                                    RadioButton(s.appIcon==name,{repo.command { icon(name) }})
                                }
                            }
                        }
                        } }
                        item { SettingsSection(footer=uiResources.getString(R.string.ui_yo_and_photos_at_the_left_end_of_a_bar_undo_at_the_right_the_came)) {
                            SettingToggle(uiResources.getString(R.string.ui_left_handed),s.leftHanded){repo.command { settings(left=it) }}
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
                            SettingRow(stringResource(R.string.ui_status), Icons.Default.CheckCircle, TileGreen) { page = "Status" }
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
                            ValueRow(uiResources.getString(R.string.ui_hub),if(s.lastSync!=null && System.currentTimeMillis()-s.lastSync<60000) uiResources.getString(R.string.ui_connected) else uiResources.getString(R.string.ui_waiting))
                            ValueRow(uiResources.getString(R.string.ui_round_trip),trip ?: uiResources.getString(R.string.ui_not_checked))
                            TextButton(onClick={probing=true;repo.command {
                                try { trip=roundTrip()?.let { "$it ms" } ?: uiResources.getString(R.string.ui_no_reply) } finally { probing=false }
                            }},enabled=!probing) { Text(if(probing) uiResources.getString(R.string.ui_checking) else uiResources.getString(R.string.ui_check_round_trip)) }
                        } }
                        item { SettingsSection(footer=uiResources.getString(R.string.ui_anonymous_crash_reports_include_android_version_os_exit_reason_an)) {
                            SettingRow(uiResources.getString(R.string.ui_crash_reports),Icons.Default.Description){page="Crash Reports"}
                            if (android.os.Build.VERSION.SDK_INT >= 33 && !notificationsEnabled) {
                                SettingRow(stringResource(R.string.enable_yo_notifications), Icons.Default.Notifications) {
                                    notificationPermission.launch(android.Manifest.permission.POST_NOTIFICATIONS)
                                }
                            }
                            SettingToggle(uiResources.getString(R.string.ui_send_crashes),crashOn){crashOn=it;repo.crashes.enabled=it;if(it) repo.command { crashes.flush() }}
                        } }
                        item { SettingsSection {
                            SettingRow(stringResource(R.string.ui_encryption),Icons.Default.Lock){page="Encryption"}
                            ValueRow(uiResources.getString(R.string.ui_version),BuildConfig.VERSION_NAME)
                        } }
                    }
                    "Crash Reports" -> item {
                        val reports=remember { repo.crashes.history() }
                        reports.forEach { text -> SelectionContainerCompat(text) }
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
                        item { Text(stringResource(R.string.ui_yo_team),style=MaterialTheme.typography.titleLarge,modifier=Modifier.padding(16.dp)) }
                        items(support?.messages.orEmpty(),key={it.id.toString()}) { message ->
                            SettingsSection(title=if(message.fromTeam) stringResource(R.string.ui_yo_team) else stringResource(R.string.you)) {
                                Text(message.text,modifier=Modifier.padding(16.dp))
                            }
                        }

                        item { SettingsSection(footer=uiResources.getString(R.string.ui_goes_to_the_yo_team_not_end_to_end_encrypted)) {
                            OutlinedTextField(feedback,{feedback=utf8Limit(it,2000)},placeholder={Text(stringResource(R.string.ui_what_should_yo_do_better))},minLines=6,modifier=Modifier.fillMaxWidth().padding(16.dp))
                        } }
                        item { Button(onClick={val text=feedback;repo.command { feedback(text);feedback="";support=supportThread();thanks=false }},enabled=feedback.isNotBlank(),modifier=Modifier.fillMaxWidth()){Text(if(thanks) uiResources.getString(R.string.ui_thanks) else uiResources.getString(R.string.ui_send))} }
                    }
                    "Delete Account" -> {
                        item { SettingsSection {
                            ListItem(colors=ListItemDefaults.colors(containerColor=Color.Transparent),headlineContent={Text(uiResources.getString(R.string.name_freed ,s.username.orEmpty()))},leadingContent={Icon(Icons.Default.PersonOff,null)})
                            ListItem(colors=ListItemDefaults.colors(containerColor=Color.Transparent),headlineContent={Text(stringResource(R.string.ui_your_friends_can_t_reach_you_anymore))},leadingContent={Icon(Icons.Default.GroupOff,null)})
                            ListItem(colors=ListItemDefaults.colors(containerColor=Color.Transparent),headlineContent={Text(stringResource(R.string.ui_this_can_t_be_undone))},leadingContent={Icon(Icons.Default.Warning,null)})
                        } }
                        item { SettingsSection(footer=uiResources.getString(R.string.type_to_confirm ,s.username.orEmpty())) {
                            OutlinedTextField(deleting,{deleting=it},label={Text(stringResource(R.string.ui_your_name))},modifier=Modifier.fillMaxWidth().padding(16.dp))
                        } }
                        item { Button(onClick=::authenticate,enabled=usernameSkeleton(deleting)==usernameSkeleton(s.username.orEmpty()),colors=ButtonDefaults.buttonColors(containerColor=MaterialTheme.colorScheme.error,contentColor=MaterialTheme.colorScheme.onError),modifier=Modifier.fillMaxWidth()){Text(stringResource(R.string.ui_delete_account))} }
                    }
                    "Encryption" -> {
                        item { SettingsSection {
                            LaunchedEffect(Unit) { status=withContext(Dispatchers.IO){repo.security()} }
                            Text(stringResource(R.string.ui_every_yo_photo_and_video_is_end_to_end_encrypted_only_you_an),modifier=Modifier.padding(16.dp))
                            status?.let { st ->
                                ValueRow(uiResources.getString(R.string.ui_hub_connected),if(s.lastSync!=null && System.currentTimeMillis()-s.lastSync<60000) "✓" else uiResources.getString(R.string.ui_waiting))
                                ValueRow(uiResources.getString(R.string.ui_hub_signed),if(st.hubSigned) "✓" else uiResources.getString(R.string.ui_not_checked))
                                ValueRow(uiResources.getString(R.string.ui_post_quantum),uiResources.getString(R.string.part_of_total ,st.pqSessions,st.devices))
                                ValueRow(uiResources.getString(R.string.ui_keys_at_hub),st.prekeysAtHub.toString())
                                ValueRow(stringResource(R.string.key_age),st.signedPrekeyDays.toString())
                                ValueRow(stringResource(R.string.refill_pending),if(st.refillPending) stringResource(R.string.ui_yes) else stringResource(R.string.ui_no))
                                ValueRow(uiResources.getString(R.string.ui_verified_contacts),uiResources.getString(R.string.part_of_total,st.verified,st.contacts))
                            }
                        }
                        }
                    }
                    "Contacts" -> {
                        item { SettingsSection {
                            contacts.forEach { friend ->
                                val fact = facts[friend.key]
                                ListItem(
                                    modifier = Modifier.clickable { selected = friend },
                                    colors = ListItemDefaults.colors(containerColor = Color.Transparent),
                                    headlineContent = { Text(friend.name) },
                                    trailingContent = {
                                        if (fact?.unconfirmed == true || friend.gone || fact?.outdated == true) {
                                            Text("!", color = TileOrange, style = MaterialTheme.typography.titleLarge)
                                        } else if (fact?.verified == true) {
                                            Icon(Icons.Default.Verified, stringResource(R.string.ui_verified), tint = TileGreen)
                                        }
                                    },
                                )
                            }
                        } }
                    }
                    "Licenses" -> {
                        item { LicenseIntro() }
                        itemsIndexed(licenseChunks,key={index,_ -> "license-$index"}) { _,text ->
                            Text(text,fontFamily=FontFamily.Monospace,fontSize=11.sp)
                        }
                    }
                }
            }
        }
    }
    selected?.let { friend ->
        ModalBottomSheet(onDismissRequest = { selected = null }, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
            Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp)) {
                Text("${friend.name} 🔒", style = MaterialTheme.typography.headlineSmall, modifier = Modifier.padding(16.dp))
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
    if(renew) AlertDialog(onDismissRequest={renew=false},title={Text(stringResource(R.string.ui_create_new_recovery_key_ccd8c2))},text={Text(stringResource(R.string.ui_your_current_recovery_key_will_stop_working_after_the_new_on))},confirmButton={TextButton(onClick={renew=false;repo.command { prepareRecovery(true) };secure=true}){Text(stringResource(R.string.ui_create_new_recovery_key))}},dismissButton={TextButton(onClick={renew=false}){Text(stringResource(R.string.ui_cancel))}})
}
private val TileOrange=Color(0xFFFF9500)
private val TileGreen=Color(0xFF34C759)
private val TileRed=Color(0xFFFF3B30)
private val TileBlue=Color(0xFF007AFF)
private val TileGray=Color(0xFF8E8E93)

@Composable private fun SettingsSection(title: String?=null,footer: String?=null,content: @Composable ColumnScope.()->Unit) {
    Column {
        if(title!=null) Text(title,style=MaterialTheme.typography.labelLarge,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.padding(start=16.dp,bottom=8.dp))
        Card(shape=RoundedCornerShape(20.dp),colors=CardDefaults.cardColors(containerColor=MaterialTheme.colorScheme.surfaceContainerHigh)) {
            CompositionLocalProvider(LocalContentColor provides MaterialTheme.colorScheme.onSurface) { Column(content=content) }
        }
        if(footer!=null) Text(footer,style=MaterialTheme.typography.bodySmall,color=MaterialTheme.colorScheme.onSurfaceVariant,modifier=Modifier.padding(start=16.dp,end=16.dp,top=8.dp))
    }
}
@Composable fun SettingRow(title: String,icon: androidx.compose.ui.graphics.vector.ImageVector,tileColor: Color=Amethyst,detail: String?=null,onClick: ()->Unit) {
    ListItem(colors=ListItemDefaults.colors(containerColor=Color.Transparent),headlineContent={Text(title)},leadingContent={Surface(shape=RoundedCornerShape(7.dp),color=tileColor,contentColor=Color.White){Icon(icon,null,modifier=Modifier.padding(6.dp).size(18.dp))}},trailingContent={
        Row(verticalAlignment = Alignment.CenterVertically) {
            if (detail != null) Text(detail, style = MaterialTheme.typography.labelMedium)
            Icon(Icons.Default.ChevronRight, null)
        }
    },modifier=Modifier.clickable(onClick=onClick))
}
@Composable fun SettingToggle(title: String,on: Boolean,changed: (Boolean)->Unit) { ListItem(colors=ListItemDefaults.colors(containerColor=Color.Transparent),headlineContent={Text(title)},trailingContent={Switch(on,changed)}) }
@Composable fun ValueRow(title: String,value: String) { ListItem(colors=ListItemDefaults.colors(containerColor=Color.Transparent),headlineContent={Text(title)},supportingContent={Text(value)}) }
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
    Column(Modifier.padding(16.dp)) {
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
            TextButton(onClick = { technical = !technical }) { Text(stringResource(R.string.technical_details)) }
            if (technical) {
                ValueRow(stringResource(R.string.ui_account), stringResource(if (d.contact?.gone == true) R.string.ui_deleted else R.string.ui_on_yo))
                ValueRow(stringResource(R.string.ui_identity_key), stringResource(if (d.contact?.unconfirmed == true) R.string.ui_changed_confirm_before_sending else R.string.ui_unchanged))
                ValueRow(stringResource(R.string.ui_sessions), d.sessions.joinToString("\n") {
                    (if (it.usable) "✓" else uiResources.getString(R.string.ui_waiting)) + "  " + it.device
                }.ifEmpty { uiResources.getString(R.string.ui_no_chat_yet) })
                d.accountKey?.let {
                    Text(stringResource(R.string.ui_account_key), modifier = Modifier.padding(top = 16.dp))
                    SelectionContainerCompat(it)
                }
            }
        }
        SettingToggle(uiResources.getString(R.string.ui_undo_before_sending),current.undo){model.repository.command { undoSettings(current,it,current.undoSeconds) }}
        if(current.undo) { Text(uiResources.getQuantityString(R.plurals.undo_seconds,current.undoSeconds,current.undoSeconds));Slider(current.undoSeconds.toFloat(),{model.repository.command { undoSettings(current,true,it.toInt()) }},valueRange=2f..10f,steps=7) }
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
    var showPasskeyProblem by remember { mutableStateOf(false) }
    val passkeyUnavailable = stringResource(R.string.ui_passkeys_need_domain_association_before_they_can_work_on_android)
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
                    TextButton(onClick = { showPasskeyProblem = true }, enabled = !busy) {
                        Text(stringResource(R.string.recovery_set_up), color = Color.White)
                    }
                }
            }
            if (showPasskeyProblem) item { Text(passkeyUnavailable) }
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
        TextButton(onClick={context.startActivity(Intent(Intent.ACTION_VIEW,"https://github.com/yoagain/yo".toUri()))}) { Text(stringResource(R.string.ui_source)) }
        Text(stringResource(R.string.ui_montserrat_sil_open_font_license_androidx_kotlin_jna_okhttp_),modifier=Modifier.padding(vertical=16.dp))
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

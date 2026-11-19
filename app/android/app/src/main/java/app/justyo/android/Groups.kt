@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
package app.justyo.android

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.Search
import androidx.compose.ui.Alignment
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextAlign
import kotlinx.coroutines.CancellationException
import uniffi.yo.YoException
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.yo.ReportReason

private val GroupNameStyle = TextStyle(fontFamily = YoFont, fontSize = 20.sp, textAlign = TextAlign.Center)

@Composable
fun GroupSheet(model: YoViewModel, state: AppState, onClose: () -> Unit) = YoSystemTheme {
    var name by remember { mutableStateOf("") }
    var search by remember { mutableStateOf("") }
    var members by remember { mutableStateOf(setOf<String>()) }
    var busy by remember { mutableStateOf(false) }
    var failed by remember { mutableStateOf(false) }
    val canCreate = name.isNotBlank() && members.isNotEmpty() && !busy
    val friends = state.friends.filter { it.group == null && it.user != null && !it.blocked && !it.gone }
    val visibleFriends = friends.filter { it.name.contains(search, ignoreCase = true) }
    val background = settingsGrouped()
    ModalBottomSheet(
        onDismissRequest = { if (!busy) onClose() },
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = background,
        contentColor = if (isSystemInDarkTheme()) Color.White else Color.Black,
        dragHandle = null,
    ) {
        val focus = LocalFocusManager.current
        val create = {
            if (canCreate) {
                focus.clearFocus()
                val groupName = name.trim()
                val selected = members.toList()
                busy = true
                failed = false
                model.repository.command {
                    val success = try {
                        createGroup(groupName, selected)
                        true
                    } catch (exception: CancellationException) {
                        throw exception
                    } catch (exception: Exception) {
                        if (exception is YoException.Replaced) throw exception
                        false
                    }
                    withContext(Dispatchers.Main.immediate) {
                        busy = false
                        failed = !success
                        if (success) onClose()
                    }
                }
            }
        }
        Column(Modifier.fillMaxWidth().imePadding()) {
            CenterAlignedTopAppBar(
                title = { Text(stringResource(R.string.ui_add_group), fontSize = 17.sp, fontWeight = FontWeight.SemiBold) },
                navigationIcon = {
                    TextButton(onClick = onClose, enabled = !busy) {
                        Text(stringResource(R.string.ui_cancel), fontSize = 17.sp)
                    }
                },
                actions = {
                    TextButton(
                        onClick = create,
                        enabled = canCreate,
                        colors = ButtonDefaults.textButtonColors(
                            contentColor = Amethyst, disabledContentColor = LocalContentColor.current.copy(alpha = 0.4f),
                        ),
                    ) {
                        Text(stringResource(R.string.ui_create), fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                    }
                },
                expandedHeight = 48.dp,
                windowInsets = WindowInsets(0, 0, 0, 0),
                colors = TopAppBarDefaults.topAppBarColors(containerColor = background),
            )
            SettingsTextField(
                search, { search = it }, stringResource(R.string.find_group_friends),
                modifier = Modifier.padding(start = 20.dp, end = 20.dp, top = 8.dp)
                    .background(settingsInputFill(), CircleShape),
                height = 38, enabled = !busy,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                keyboardActions = KeyboardActions(onSearch = { focus.clearFocus() }),
                leading = { Icon(Icons.Default.Search, null, Modifier.size(18.dp), tint = settingsSecondary()) },
            )
            LazyColumn(
                Modifier.fillMaxWidth().weight(1f, fill = false),
                verticalArrangement = Arrangement.spacedBy(28.dp),
                contentPadding = PaddingValues(start = 20.dp, end = 20.dp, top = 20.dp, bottom = 20.dp),
            ) {
                item {
                    SettingsSection {
                        SettingsTextField(
                            name, { name = utf8Limit(it, 32) }, stringResource(R.string.ui_group_name),
                            enabled = !busy, keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                            keyboardActions = KeyboardActions(onDone = { focus.clearFocus() }),
                        )
                    }
                }
                item {
                    SettingsSection(
                        title = stringResource(R.string.group_member_limit, members.size + 1),
                        footer = stringResource(R.string.group_member_footer),
                    ) {
                        GroupMemberRow(state.username.orEmpty(), selected = true)
                        for (friend in visibleFriends) {
                            val user = checkNotNull(friend.user)
                            val selected = user in members
                            val enabled = !busy && (selected || members.size < 19)
                            HorizontalDivider(
                                Modifier.padding(start = 64.dp, end = 16.dp),
                                color = LocalContentColor.current.copy(alpha = 0.12f),
                            )
                            GroupMemberRow(
                                friend.name, selected,
                                Modifier.alpha(if (enabled) 1f else 0.4f).toggleable(
                                    value = selected, enabled = enabled, role = Role.Checkbox,
                                    onValueChange = { checked ->
                                        focus.clearFocus()
                                        members = if (checked) members + user else members - user
                                    },
                                ),
                            )
                        }
                    }
                }
                if (failed) item {
                    SettingsSection {
                        Text(
                            stringResource(R.string.group_create_failure), Modifier.padding(16.dp),
                            color = MaterialTheme.colorScheme.error, fontSize = 17.sp,
                        )
                    }
                }
            }
        }
    }
}

@Composable
private fun GroupMemberRow(name: String, selected: Boolean, modifier: Modifier = Modifier) {
    Row(
        modifier.fillMaxWidth().heightIn(min = 54.dp).padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        SettingsAvatar(name, 36)
        Text(name, Modifier.weight(1f), fontSize = 17.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        if (selected) {
            Box(Modifier.size(26.dp).background(Amethyst, CircleShape), contentAlignment = Alignment.Center) {
                Icon(Icons.Default.Check, null, Modifier.size(17.dp), tint = Color.White)
            }
        } else {
            Box(Modifier.size(26.dp).border(1.5.dp, LocalContentColor.current.copy(alpha = 0.3f), CircleShape))
        }
    }
}
private enum class FriendAction { BLOCK, RENAME, LEAVE, REMOVE_MEMBER }
@Composable fun FriendMenu(model: YoViewModel,s: AppState,friend: Friend,onClose: ()->Unit) {
    val repo=model.repository
    var action by remember { mutableStateOf<FriendAction?>(null) }
    var selectedUser by remember { mutableStateOf<String?>(null) }
    var rename by remember { mutableStateOf(friend.name) }
    var members by remember { mutableStateOf<List<String>>(emptyList()) }
    var showMembers by remember { mutableStateOf(false) }
    var showBlock by remember { mutableStateOf(false) }
    var busy by remember { mutableStateOf(false) }
    LaunchedEffect(showMembers) { if(showMembers) members=withContext(Dispatchers.IO) { repo.groupMembers(friend) } }
    ModalBottomSheet(onDismissRequest=onClose) {
        LazyColumn(Modifier.fillMaxWidth().navigationBarsPadding().padding(bottom=24.dp)) {
            item { Text(friend.name,style=MaterialTheme.typography.titleLarge,modifier=Modifier.padding(24.dp)) }
            item {
                val format=remember { java.text.DateFormat.getDateTimeInstance(java.text.DateFormat.SHORT,java.text.DateFormat.SHORT) }
                val sent = s.lastSent[friend.key]
                val received = s.lastReceived[friend.key]
                val latest = maxOf(sent ?: 0L, received ?: 0L)
                if (latest > 0) {
                    val direction = if (latest == sent) R.string.last_sent else R.string.last_received
                    ValueRow(stringResource(direction), format.format(java.util.Date(latest)))
                }
            }
            if(friend.group==null) {
                item { TextButton(onClick={showBlock=!showBlock},modifier=Modifier.fillMaxWidth()){Text(stringResource(R.string.ui_block))} }
                if (showBlock) {
                    item { TextButton(onClick={action=FriendAction.BLOCK},modifier=Modifier.fillMaxWidth()){Text(stringResource(R.string.ui_block))} }
                    items(ReportReason.entries,key={it.name}) { reason ->
                        val label=stringResource(when(reason) { ReportReason.SPAM -> R.string.report_spam;ReportReason.ABUSE -> R.string.report_abuse;ReportReason.OTHER -> R.string.report_other })
                        TextButton(onClick={repo.command { report(friend,reason);onClose() }},modifier=Modifier.fillMaxWidth()){Text(label)}
                    }
                }
                item { TextButton(onClick={repo.requestRemove(friend);onClose()},modifier=Modifier.fillMaxWidth()){Text(stringResource(R.string.ui_remove))} }
            } else {
                item { TextButton(onClick={showMembers=!showMembers},modifier=Modifier.fillMaxWidth()){Text(stringResource(R.string.ui_members))} }
                if(showMembers) {
                    item { Text(stringResource(R.string.member_count,members.size),modifier=Modifier.padding(horizontal=24.dp)) }
                    items(members,key={it}) { user ->
                        val person=s.friends.firstOrNull { it.user==user }
                        ListItem(headlineContent={Text(person?.name ?: stringResource(R.string.group_unknown_member))},trailingContent={
                            if(person!=null) TextButton(onClick={selectedUser=user;action=FriendAction.REMOVE_MEMBER}){Text(stringResource(R.string.ui_remove))}
                        })
                    }
                    items(s.friends.filter { it.user!=null && it.user !in members && it.group==null && !it.blocked },key={it.key}) { candidate ->
                        TextButton(onClick={busy=true;repo.command { try { groupChange(friend,add=listOf(candidate.user!!));members=groupMembers(friend) } finally { busy=false } }},enabled=members.size<20 && !busy,modifier=Modifier.fillMaxWidth()) {
                            Text(stringResource(R.string.add_member,candidate.name))
                        }
                    }
                }
                item { TextButton(onClick={action=FriendAction.RENAME},modifier=Modifier.fillMaxWidth()){Text(stringResource(R.string.ui_rename))} }
                item { TextButton(onClick={action=FriendAction.LEAVE},modifier=Modifier.fillMaxWidth()){Text(stringResource(R.string.leave_group,friend.name))} }
            }
        }
    }
    action?.let { pending ->
        val label=stringResource(when(pending) { FriendAction.BLOCK -> R.string.ui_block;FriendAction.RENAME -> R.string.ui_rename;FriendAction.LEAVE -> R.string.leave_action;FriendAction.REMOVE_MEMBER -> R.string.ui_remove })
        AlertDialog(onDismissRequest={if(!busy) action=null},title={Text(label)},text={
            if (pending == FriendAction.RENAME) {
                OutlinedTextField(
                    rename, { rename = utf8Limit(it, 32) },
                    label = { Text(stringResource(R.string.ui_group_name)) },
                    singleLine = true,
                    textStyle = GroupNameStyle,
                )
            }
            else Text(stringResource(when(pending) { FriendAction.LEAVE -> R.string.ui_you_won_t_get_its_yos_any_more_a_member_can_add_you_again;FriendAction.REMOVE_MEMBER -> R.string.ui_they_won_t_get_the_group_s_yos_any_more;else -> R.string.ui_you_won_t_get_their_yos_unblock_them_in_your_profile }))
        },confirmButton={TextButton(onClick={busy=true;repo.command {
            try {
                when(pending) {
                    FriendAction.BLOCK -> block(friend,true)
                    FriendAction.RENAME -> groupChange(friend,rename=rename)
                    FriendAction.LEAVE -> groupChange(friend,leave=true)
                    FriendAction.REMOVE_MEMBER -> selectedUser?.let { groupChange(friend,remove=listOf(it)) }
                }
                action=null;onClose()
            } finally { busy=false }
        }},enabled=!busy && (pending!=FriendAction.RENAME || rename.isNotBlank())){Text(label)}},dismissButton={TextButton(onClick={action=null},enabled=!busy){Text(stringResource(R.string.ui_cancel))}})
    }
}

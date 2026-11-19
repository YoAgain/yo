@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
package app.justyo.android

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ExitToApp
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.yo.ReportReason
import uniffi.yo.YoException

/** A new group (iOS NewGroup): a name, then up to 19 friends. Standard sheet, fields and list items. */
@Composable
fun GroupSheet(model: YoViewModel, state: AppState, onClose: () -> Unit) = YoSystemTheme {
    var name by remember { mutableStateOf("") }
    var search by remember { mutableStateOf("") }
    var members by remember { mutableStateOf(setOf<String>()) }
    var busy by remember { mutableStateOf(false) }
    var failed by remember { mutableStateOf(false) }
    val canCreate = name.isNotBlank() && members.isNotEmpty() && !busy
    val friends = state.friends.filter { it.group == null && it.user != null && !it.blocked && !it.gone }
        .filter { it.name.contains(search, ignoreCase = true) }
    val focus = LocalFocusManager.current
    val create = {
        if (canCreate) {
            focus.clearFocus()
            val groupName = name.trim()
            val selected = members.toList()
            busy = true
            failed = false
            model.repository.command {
                val success = try { createGroup(groupName, selected); true }
                catch (e: CancellationException) { throw e }
                catch (e: Exception) { if (e is YoException.Replaced) throw e; false }
                withContext(Dispatchers.Main.immediate) { busy = false; failed = !success; if (success) onClose() }
            }
        }
    }
    ModalBottomSheet(onDismissRequest = { if (!busy) onClose() }, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true)) {
        Column(Modifier.fillMaxWidth().imePadding()) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                IconButton(onClick = onClose, enabled = !busy) { Icon(Icons.Default.Close, stringResource(R.string.ui_cancel)) }
                Text(stringResource(R.string.ui_add_group), Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
                Button(onClick = create, enabled = canCreate) { Text(stringResource(R.string.ui_create)) }
            }
            Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(16.dp)) {
                OutlinedTextField(name, { name = utf8Limit(it, 32) }, Modifier.fillMaxWidth(), enabled = !busy, singleLine = true,
                    label = { Text(stringResource(R.string.ui_group_name)) },
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done), keyboardActions = KeyboardActions(onDone = { focus.clearFocus() }))
                OutlinedTextField(search, { search = it }, Modifier.fillMaxWidth().padding(vertical = 12.dp), enabled = !busy, singleLine = true,
                    placeholder = { Text(stringResource(R.string.find_group_friends)) }, leadingIcon = { Icon(Icons.Default.Search, null) },
                    shape = MaterialTheme.shapes.extraLarge,
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search), keyboardActions = KeyboardActions(onSearch = { focus.clearFocus() }))
                if (failed) Text(stringResource(R.string.group_create_failure), Modifier.padding(bottom = 12.dp), color = MaterialTheme.colorScheme.error)
                Section(stringResource(R.string.group_member_limit, members.size + 1), stringResource(R.string.group_member_footer),
                    Setting(state.username.orEmpty(), leading = { SettingsAvatar(state.username.orEmpty(), 40) }, checked = true, checkbox = true, enabled = false),
                    *friends.map { friend ->
                        val user = checkNotNull(friend.user)
                        val selected = user in members
                        Setting(friend.name, leading = { SettingsAvatar(friend.name, 40) }, checked = selected, checkbox = true,
                            enabled = !busy && (selected || members.size < 19),
                            onChecked = { on -> focus.clearFocus(); members = if (on) members + user else members - user })
                    }.toTypedArray(),
                )
            }
        }
    }
}

private enum class FriendAction { BLOCK, RENAME, LEAVE, REMOVE_MEMBER }

/** A long press on a bar: the last Yo, block or report, remove; for a group its members, rename and leave. */
@Composable fun FriendMenu(model: YoViewModel, s: AppState, friend: Friend, onClose: () -> Unit) = YoSystemTheme {
    val repo = model.repository
    val res = LocalResources.current
    var action by remember { mutableStateOf<FriendAction?>(null) }
    var selectedUser by remember { mutableStateOf<String?>(null) }
    var rename by remember { mutableStateOf(friend.name) }
    var members by remember { mutableStateOf<List<String>?>(null) }
    var busy by remember { mutableStateOf(false) }
    LaunchedEffect(friend.group) { if (friend.group != null) members = withContext(Dispatchers.IO) { repo.groupMembers(friend) } }
    ModalBottomSheet(onDismissRequest = onClose) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp)) {
            Text(if (friend.group == null) friend.name else "#" + friend.name.removePrefix("#"), Modifier.padding(start = 8.dp, bottom = 16.dp),
                style = MaterialTheme.typography.headlineSmall)
            val format = remember { java.text.DateFormat.getDateTimeInstance(java.text.DateFormat.SHORT, java.text.DateFormat.SHORT) }
            val sent = s.lastSent[friend.key]
            val received = s.lastReceived[friend.key]
            val latest = maxOf(sent ?: 0L, received ?: 0L)
            if (latest > 0) Section(null, null,
                Setting(stringResource(if (latest == sent) R.string.last_sent else R.string.last_received), Icons.Default.Schedule, format.format(java.util.Date(latest))))
            if (friend.group == null) {
                Section(null, null,
                    Setting(stringResource(R.string.ui_block), Icons.Default.Block, onClick = { action = FriendAction.BLOCK }),
                    *ReportReason.entries.map { reason ->
                        Setting(stringResource(when (reason) { ReportReason.SPAM -> R.string.report_spam; ReportReason.ABUSE -> R.string.report_abuse; ReportReason.OTHER -> R.string.report_other }),
                            Icons.Default.Flag, onClick = { repo.command { report(friend, reason); onClose() } })
                    }.toTypedArray(),
                )
                Section(null, null,
                    Setting(stringResource(R.string.ui_remove), Icons.Default.PersonRemove, danger = true, onClick = { repo.requestRemove(friend); onClose() }))
            } else {
                val current = members
                if (current != null) {
                    Section(stringResource(R.string.ui_members) + " · " + stringResource(R.string.member_count, current.size), null,
                        *current.map { user ->
                            val person = s.friends.firstOrNull { it.user == user }
                            val name = person?.name ?: s.username.takeIf { user == repo.widgetAccount() } ?: res.getString(R.string.group_unknown_member)
                            Setting(name, leading = { SettingsAvatar(name, 40) }, value = person?.let { res.getString(R.string.ui_remove) },
                                onClick = person?.let { { selectedUser = user; action = FriendAction.REMOVE_MEMBER } })
                        }.toTypedArray())
                    val candidates = s.friends.filter { it.user != null && it.user !in current && it.group == null && !it.blocked && !it.gone }
                    if (candidates.isNotEmpty()) Section(null, null, *candidates.map { candidate ->
                        Setting(res.getString(R.string.add_member, candidate.name), Icons.Default.PersonAdd, enabled = current.size < 20 && !busy, onClick = {
                            busy = true
                            repo.command { try { groupChange(friend, add = listOf(candidate.user!!)); members = groupMembers(friend) } finally { busy = false } }
                        })
                    }.toTypedArray())
                }
                Section(null, null,
                    Setting(stringResource(R.string.ui_rename), Icons.Default.Edit, onClick = { action = FriendAction.RENAME }),
                    Setting(res.getString(R.string.leave_group, friend.name), Icons.AutoMirrored.Filled.ExitToApp, danger = true, onClick = { action = FriendAction.LEAVE }),
                )
            }
            Spacer(Modifier.height(16.dp))
        }
    }
    action?.let { pending ->
        val label = stringResource(when (pending) {
            FriendAction.BLOCK -> R.string.ui_block; FriendAction.RENAME -> R.string.ui_rename
            FriendAction.LEAVE -> R.string.leave_action; FriendAction.REMOVE_MEMBER -> R.string.ui_remove
        })
        AlertDialog(
            onDismissRequest = { if (!busy) action = null },
            title = { Text(label) },
            text = {
                if (pending == FriendAction.RENAME) OutlinedTextField(rename, { rename = utf8Limit(it, 32) }, singleLine = true,
                    label = { Text(stringResource(R.string.ui_group_name)) })
                else Text(stringResource(when (pending) {
                    FriendAction.LEAVE -> R.string.ui_you_won_t_get_its_yos_any_more_a_member_can_add_you_again
                    FriendAction.REMOVE_MEMBER -> R.string.ui_they_won_t_get_the_group_s_yos_any_more
                    else -> R.string.ui_you_won_t_get_their_yos_unblock_them_in_your_profile
                }))
            },
            confirmButton = {
                TextButton(enabled = !busy && (pending != FriendAction.RENAME || rename.isNotBlank()), onClick = {
                    busy = true
                    repo.command {
                        try {
                            when (pending) {
                                FriendAction.BLOCK -> block(friend, true)
                                FriendAction.RENAME -> groupChange(friend, rename = rename)
                                FriendAction.LEAVE -> groupChange(friend, leave = true)
                                FriendAction.REMOVE_MEMBER -> selectedUser?.let { groupChange(friend, remove = listOf(it)) }
                            }
                            action = null; onClose()
                        } finally { busy = false }
                    }
                }) { Text(label) }
            },
            dismissButton = { TextButton(onClick = { action = null }, enabled = !busy) { Text(stringResource(R.string.ui_cancel)) } },
        )
    }
}

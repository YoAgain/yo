@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
package app.justyo.android

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.ui.graphics.Color
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ExitToApp
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.lifecycle.compose.collectAsStateWithLifecycle
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
                catch (e: Exception) { if (e is YoException.Replaced) throw e; Problems.report(e); false }
                withContext(Dispatchers.Main.immediate) { busy = false; failed = !success; if (success) onClose() }
            }
        }
    }
    // As iOS NewGroup: Cancel, the title, Create in the bar; the name, the search, then the members with a tick.
    FittedSheet(onDismissRequest = { if (!busy) onClose() }, dragHandle = null) {
        CenterAlignedTopAppBar(
            title = { Text(stringResource(R.string.ui_add_group), maxLines = 1) },
            navigationIcon = { IconButton(onClick = onClose, enabled = !busy) { Icon(Icons.Default.Close, stringResource(R.string.ui_cancel)) } },
            actions = { TextButton(onClick = create, enabled = canCreate) { Text(stringResource(R.string.ui_create)) } },
            colors = TopAppBarDefaults.topAppBarColors(containerColor = Color.Transparent),
            windowInsets = WindowInsets(0),
        )
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).padding(bottom = 8.dp)) {
            val field = TextFieldDefaults.colors(
                focusedContainerColor = MaterialTheme.colorScheme.surfaceBright, unfocusedContainerColor = MaterialTheme.colorScheme.surfaceBright,
                disabledContainerColor = MaterialTheme.colorScheme.surfaceBright,
                focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent, disabledIndicatorColor = Color.Transparent,
            )
            TextField(name, { name = utf8Limit(it, 32) }, Modifier.fillMaxWidth(), enabled = !busy, singleLine = true,
                placeholder = { Text(stringResource(R.string.ui_group_name)) }, shape = MaterialTheme.shapes.large, colors = field,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done), keyboardActions = KeyboardActions(onDone = { focus.clearFocus() }))
            TextField(search, { search = it }, Modifier.fillMaxWidth().padding(top = 12.dp, bottom = 20.dp), enabled = !busy, singleLine = true,
                placeholder = { Text(stringResource(R.string.find_group_friends)) }, leadingIcon = { Icon(Icons.Default.Search, null) },
                shape = CircleShape, colors = field,
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search), keyboardActions = KeyboardActions(onSearch = { focus.clearFocus() }))
            if (failed) Text(stringResource(R.string.group_create_failure), Modifier.padding(start = 16.dp, bottom = 12.dp), color = MaterialTheme.colorScheme.error)
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

private enum class FriendAction { BLOCK, RENAME, LEAVE }

/**
 * A long press on a bar, as iOS's menu: what waits to be sent and the last Yo (greyed), then a friend's Block, Report
 * (Spam, Abuse, Something Else) and Remove; a group's Members, Rename and Leave. A report asks to block too.
 */
@Composable fun FriendMenu(model: YoViewModel, friend: Friend, onClose: () -> Unit) = YoSystemTheme {
    val s by model.repository.state.collectAsStateWithLifecycle()
    val repo = model.repository
    val res = LocalResources.current
    var action by remember { mutableStateOf<FriendAction?>(null) }
    var rename by remember { mutableStateOf(friend.name) }
    var busy by remember { mutableStateOf(false) }
    var members by remember { mutableStateOf(false) }
    var reported by remember { mutableStateOf(false) }
    var problem by remember { mutableStateOf<Pair<String, String>?>(null) }
    val stacks by repo.media.undo.collectAsStateWithLifecycle()
    if (members) { GroupMembersSheet(model, friend, onClose = { members = false; onClose() }); return@YoSystemTheme }
    FittedSheet(onDismissRequest = onClose) {
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp)) {
            Text(if (friend.group == null) friend.name else "#" + friend.name.removePrefix("#"), Modifier.padding(start = 8.dp, bottom = 16.dp),
                style = MaterialTheme.typography.headlineSmall)
            val activity = remember(s.lastSent[friend.key], s.lastReceived[friend.key]) {
                uniffi.yo.productLastActivity(s.lastSent[friend.key], s.lastReceived[friend.key], System.currentTimeMillis(), Words)
            }
            val waiting = stacks[friend.target]?.takeIf { it.kind != uniffi.yo.ListKind.YO }?.count?.toInt() ?: 0
            Section(null, null,
                if (waiting > 0) Setting(res.getQuantityString(R.plurals.photos_waiting, waiting, waiting), Icons.Default.PhotoLibrary, enabled = false) else null,
                Setting(activity.text, when (activity.direction) {
                    uniffi.yo.ProductDirection.SENT -> Icons.Default.ArrowUpward
                    uniffi.yo.ProductDirection.RECEIVED -> Icons.Default.ArrowDownward
                    uniffi.yo.ProductDirection.NEVER -> Icons.Default.Schedule
                }, enabled = false),
            )
            if (friend.group == null) {
                Section(null, null, Setting(stringResource(R.string.ui_block), Icons.Default.Block, onClick = { action = FriendAction.BLOCK }))
                Section(stringResource(R.string.report), null,
                    *ReportReason.entries.map { reason ->
                        Setting(stringResource(when (reason) { ReportReason.SPAM -> R.string.report_spam; ReportReason.ABUSE -> R.string.report_abuse; ReportReason.OTHER -> R.string.report_other }),
                            Icons.Default.Flag, enabled = !busy, onClick = {
                                busy = true
                                repo.command {
                                    try { report(friend, reason); reported = true; action = FriendAction.BLOCK }
                                    catch (e: CancellationException) { throw e }
                                    catch (e: Exception) {
                                        if (e is YoException.Replaced) throw e
                                        Problems.report(e)
                                        problem = res.getString(R.string.unable_to_report) to friendlyError(model.getApplication(), e)
                                    } finally { busy = false }
                                }
                            })
                    }.toTypedArray(),
                )
                Section(null, null,
                    Setting(stringResource(R.string.ui_remove), Icons.Default.Delete, danger = true, onClick = { repo.requestRemove(friend); onClose() }))
            } else {
                Section(null, null,
                    Setting(stringResource(R.string.ui_members), Icons.Default.Groups, onClick = { members = true }),
                    Setting(stringResource(R.string.ui_rename), Icons.Default.Edit, onClick = { rename = friend.name; action = FriendAction.RENAME }),
                    Setting(res.getString(R.string.leave_group, friend.name), Icons.AutoMirrored.Filled.ExitToApp, danger = true, onClick = { action = FriendAction.LEAVE }),
                )
            }
            Spacer(Modifier.height(16.dp))
        }
    }
    problem?.let { (title, message) ->
        AlertDialog(onDismissRequest = { problem = null }, title = { Text(title) }, text = { Text(message) },
            confirmButton = { TextButton(onClick = { problem = null }) { Text(stringResource(android.R.string.ok)) } })
    }
    action?.let { pending ->
        val title = when (pending) {
            FriendAction.BLOCK -> if (reported) stringResource(R.string.thanks_for_reporting) else res.getString(R.string.block_name, friend.name)
            FriendAction.RENAME -> res.getString(R.string.rename_name, friend.name)
            FriendAction.LEAVE -> res.getString(R.string.leave_name, friend.name)
        }
        val confirm = when (pending) {
            FriendAction.BLOCK -> res.getString(R.string.block_action_name, friend.name)
            FriendAction.RENAME -> stringResource(R.string.ui_rename)
            FriendAction.LEAVE -> stringResource(R.string.leave_action)
        }
        val dismissAll = { action = null; if (reported) onClose() }
        AlertDialog(
            onDismissRequest = { if (!busy) dismissAll() },
            title = { Text(title) },
            text = {
                if (pending == FriendAction.RENAME) OutlinedTextField(rename, { rename = utf8Limit(it, 32) }, singleLine = true,
                    label = { Text(stringResource(R.string.ui_group_name)) })
                else Text(when (pending) {
                    FriendAction.LEAVE -> stringResource(R.string.ui_you_won_t_get_its_yos_any_more_a_member_can_add_you_again)
                    else -> if (reported) res.getString(R.string.block_too, friend.name) else stringResource(R.string.ui_you_won_t_get_their_yos_unblock_them_in_your_profile)
                })
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
                            }
                            action = null; onClose()
                        } finally { busy = false }
                    }
                }) { Text(confirm) }
            },
            dismissButton = { TextButton(onClick = dismissAll, enabled = !busy) {
                Text(stringResource(if (reported && pending == FriendAction.BLOCK) R.string.not_now else R.string.ui_cancel)) } },
        )
    }
}

/** A group's members, as iOS's sheet: this account ticked, each member removable (asked first), friends not in it to
 *  add (up to 20 in all), a search, Done. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable private fun GroupMembersSheet(model: YoViewModel, group: Friend, onClose: () -> Unit) {
    val s by model.repository.state.collectAsStateWithLifecycle()
    val repo = model.repository
    val res = LocalResources.current
    var members by remember { mutableStateOf<List<String>?>(null) }
    var busy by remember { mutableStateOf(false) }
    var failed by remember { mutableStateOf(false) }
    var search by remember { mutableStateOf("") }
    var removing by remember { mutableStateOf<Pair<String, String>?>(null) }
    LaunchedEffect(group.group) { members = withContext(Dispatchers.IO) { repo.groupMembers(group) } }
    val me = repo.widgetAccount()
    fun nameOf(user: String) = s.friends.firstOrNull { it.user == user && it.group == null }?.name
        ?: s.username.takeIf { user == me } ?: res.getString(R.string.group_unknown_member)
    fun change(add: List<String> = emptyList(), remove: List<String> = emptyList()) {
        busy = true; failed = false
        repo.command {
            try { groupChange(group, add = add, remove = remove); members = groupMembers(group) }
            catch (e: CancellationException) { throw e }
            catch (e: Exception) { if (e is YoException.Replaced) throw e; Problems.report(e); failed = true }
            finally { busy = false }
        }
    }
    val current = members.orEmpty()
    val choices = (current + s.friends.filter { it.group == null && !it.blocked && !it.gone }.mapNotNull { it.user })
        .distinct().filter { it != me && (search.isBlank() || nameOf(it).contains(search, ignoreCase = true)) }
        .sortedWith(compareBy(String.CASE_INSENSITIVE_ORDER) { nameOf(it) })
    val focus = LocalFocusManager.current
    FittedSheet(onDismissRequest = onClose, dragHandle = null) {
        CenterAlignedTopAppBar(
            title = { Text(group.name, maxLines = 1) },
            actions = { TextButton(onClick = onClose) { Text(stringResource(R.string.ui_done)) } },
            colors = TopAppBarDefaults.topAppBarColors(containerColor = Color.Transparent),
            windowInsets = WindowInsets(0),
        )
        Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).padding(bottom = 8.dp)) {
            TextField(search, { search = it }, Modifier.fillMaxWidth().padding(bottom = 20.dp), singleLine = true,
                placeholder = { Text(stringResource(R.string.find_group_friends)) }, leadingIcon = { Icon(Icons.Default.Search, null) },
                shape = CircleShape, colors = TextFieldDefaults.colors(
                    focusedContainerColor = MaterialTheme.colorScheme.surfaceBright, unfocusedContainerColor = MaterialTheme.colorScheme.surfaceBright,
                    focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search), keyboardActions = KeyboardActions(onSearch = { focus.clearFocus() }))
            if (members == null) Box(Modifier.fillMaxWidth().padding(24.dp), contentAlignment = Alignment.Center) { LoadingIndicator() }
            else {
                fun row(user: String): Setting {
                    val selected = user in current
                    val name = nameOf(user)
                    return Setting(name, leading = { SettingsAvatar(name, 40) }, checked = selected, checkbox = true,
                        enabled = !busy && (selected || current.size < 20),
                        onChecked = { on -> if (on) change(add = listOf(user)) else removing = user to name })
                }
                Section(res.getString(R.string.ui_members) + " · " + res.getString(R.string.member_count, current.size), null,
                    me?.let { Setting(s.username.orEmpty(), leading = { SettingsAvatar(s.username.orEmpty(), 40) }, checked = true, checkbox = true, enabled = false) },
                    *choices.filter { it in current }.map(::row).toTypedArray())
                val others = choices.filter { it !in current }
                if (others.isNotEmpty()) Section(stringResource(R.string.ui_add_friends), null, *others.map(::row).toTypedArray())
                if (failed) Text(stringResource(R.string.group_change_failed), Modifier.padding(8.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
    removing?.let { (user, name) ->
        AlertDialog(onDismissRequest = { removing = null }, title = { Text(res.getString(R.string.remove_name, name)) },
            text = { Text(stringResource(R.string.ui_they_won_t_get_the_group_s_yos_any_more)) },
            confirmButton = { TextButton(onClick = { removing = null; change(remove = listOf(user)) }) { Text(res.getString(R.string.remove_action_name, name)) } },
            dismissButton = { TextButton(onClick = { removing = null }) { Text(stringResource(R.string.ui_cancel)) } })
    }
}

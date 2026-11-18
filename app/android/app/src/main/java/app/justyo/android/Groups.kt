@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
package app.justyo.android

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.yo.ReportReason

@Composable fun GroupSheet(model: YoViewModel,s: AppState,onClose: ()->Unit) {
    val uiResources=androidx.compose.ui.platform.LocalResources.current

    var name by remember { mutableStateOf("") }
    var members by remember { mutableStateOf(setOf<String>()) }
    var busy by remember { mutableStateOf(false) }
    ModalBottomSheet(onDismissRequest=onClose,sheetState=rememberModalBottomSheetState(skipPartiallyExpanded=true)) {
        Column(Modifier.fillMaxWidth().padding(16.dp)) {
            Text(stringResource(R.string.ui_add_group),style=MaterialTheme.typography.headlineSmall)
            OutlinedTextField(name,{name=utf8Limit(it,32)},label={Text(stringResource(R.string.ui_group_name))},modifier=Modifier.fillMaxWidth().padding(vertical=16.dp))
            Text(stringResource(R.string.ui_members_up_to_19_friends))
            LazyColumn(Modifier.heightIn(max=350.dp)) {
                items(s.friends.filter { it.group==null && it.user!=null && !it.blocked && !it.gone },key={it.key}) { friend ->
                    val user=friend.user!!
                    ListItem(headlineContent={Text(friend.name)},trailingContent={Checkbox(user in members,{chosen -> if(chosen && members.size<19) members=members+user else if(!chosen) members=members-user})},modifier=Modifier.clickable { if(user in members) members=members-user else if(members.size<19) members=members+user })
                }
            }
            Button(onClick={busy=true;model.repository.command { try { createGroup(name,members.toList());onClose() } finally {busy=false} }},enabled=name.isNotBlank() && members.isNotEmpty() && !busy,modifier=Modifier.fillMaxWidth().padding(top=16.dp)){Text(if(busy) uiResources.getString(R.string.ui_creating) else uiResources.getString(R.string.ui_create))}
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
            if(pending==FriendAction.RENAME) OutlinedTextField(rename,{rename=utf8Limit(it,32)},label={Text(stringResource(R.string.ui_group_name))})
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

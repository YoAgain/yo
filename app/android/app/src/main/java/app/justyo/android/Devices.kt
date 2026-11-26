@file:OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class, androidx.compose.material3.ExperimentalMaterial3ExpressiveApi::class)
package app.justyo.android

import android.text.format.DateUtils
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.core.net.toUri
import kotlinx.coroutines.*
import uniffi.yo.*

private fun YoDevice.platform() = when (app.substringBefore('/').lowercase()) {
    "android" -> R.string.md_android
    "ios" -> R.string.md_iphone
    "linux" -> R.string.md_linux
    else -> R.string.md_computer
}

@Composable private fun DeviceIcon(device: YoDevice) {
    when (device.platform()) {
        R.string.md_android -> Icon(Icons.Default.Smartphone, null, tint = Color(0xff2ecc71))
        R.string.md_iphone -> Icon(Icons.Default.Smartphone, null, tint = Color(0xff3498db))
        else -> Icon(Icons.Default.Laptop, null)
    }
}

/** The hub keeps last seen to the hour and writes it once a day (spec 1): only the day is told, as Signal's "Last active". */
@Composable private fun lastSeen(device: YoDevice): String {
    val at = device.lastSeenMs.toLong()
    return when {
        DateUtils.isToday(at) -> stringResource(R.string.md_active_today)
        DateUtils.isToday(at + DateUtils.DAY_IN_MILLIS) -> stringResource(R.string.md_active_yesterday)
        else -> stringResource(R.string.md_active_on, DateUtils.formatDateTime(LocalContext.current, at, DateUtils.FORMAT_SHOW_DATE or DateUtils.FORMAT_ABBREV_MONTH))
    }
}

@Composable internal fun DevicesPage(repo: Repository, ownNumber: UByte?, devices: List<YoDevice>?, loadError: String?, refresh: () -> Unit, scan: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var busy by remember { mutableStateOf(false) }
    var removeError by remember { mutableStateOf<String?>(null) }
    var removing by remember { mutableStateOf<YoDevice?>(null) }
    Section(null, null, Setting(stringResource(R.string.md_add_device), Icons.Default.QrCodeScanner, onClick = scan))
    if (loadError != null) {
        Text(loadError)
        TextButton(onClick = refresh) { Text(stringResource(R.string.retry)) }
    }
    if (devices == null) {
        if (loadError == null) CircularProgressIndicator()
        return
    }
    Section(stringResource(R.string.md_devices_count, devices.size), null,
        *devices.map { device ->
            val own = ownNumber == device.number
            Setting(stringResource(device.platform()), leading = { DeviceIcon(device) },
                supporting = if (own) stringResource(R.string.md_this_device) else lastSeen(device),
                trailing = if (own || ownNumber == null) null else { {
                    TextButton(onClick = { removeError = null; removing = device }) { Text(stringResource(R.string.ui_remove), color = MaterialTheme.colorScheme.error) }
                } })
        }.toTypedArray())
    removing?.let { device ->
        AlertDialog(onDismissRequest = { if (!busy) removing = null },
            title = { Text(stringResource(R.string.md_remove_title, stringResource(device.platform()))) },
            text = {
                Column {
                    Text(stringResource(R.string.md_remove_message))
                    removeError?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                }
            },
            confirmButton = { TextButton(enabled = !busy, onClick = {
                busy = true
                scope.launch {
                    try {
                        withContext(Dispatchers.IO) { repo.removeDevice(device.number) }
                        removing = null
                        refresh()
                    } catch (e: CancellationException) { throw e }
                    catch (e: Exception) {
                        if (e is YoException.Replaced) repo.command { throw e }
                        if (e is YoException.Refused && e.status.toInt() == 404) {
                            removing = null
                            refresh()
                        } else removeError = friendlyError(context, e)
                    } finally { busy = false }
                }
            }) { Text(stringResource(R.string.ui_remove), color = MaterialTheme.colorScheme.error) } },
            dismissButton = { TextButton(enabled = !busy, onClick = { removing = null }) { Text(stringResource(R.string.ui_cancel)) } })
    }
}

@Composable internal fun DeviceScanner(repo: Repository, capable: Boolean, showingCode: Boolean,
    onLinked: () -> Unit, onFriendAdded: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var offer by remember { mutableStateOf<YoLinkOffer?>(null) }
    var friend by remember { mutableStateOf<String?>(null) }
    var paste by remember { mutableStateOf(false) }
    var input by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    val invalidCode = stringResource(R.string.md_invalid_code)
    fun read(raw: String) {
        val text = raw.trim()
        error = null
        if (text.startsWith("yo-link:")) {
            offer = if (capable) runCatching { linkOffer(text) }.getOrNull() else null
        } else {
            val url = if (text.startsWith("justyo.co/", ignoreCase = true)) "https://$text" else text
            friend = inviteName(url.toUri())?.takeIf(::usernameValid)
        }
        if (offer == null && friend == null) error = invalidCode
    }
    Column(Modifier.fillMaxSize().imePadding().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (!paste && !showingCode)
            QrCamera(Modifier.fillMaxWidth().weight(1f), continuous = true) {
                if (offer == null && friend == null && !busy) read(it.text)
            }
        else Spacer(Modifier.weight(1f))
        error?.let {
            Text(it, color = MaterialTheme.colorScheme.error)
        }
        // The camera alone; a code is pasted only in a debug build (testing without a second camera).
        if (!BuildConfig.DEBUG) Unit
        else if (!paste) TextButton(onClick = { paste = true; error = null }) { Text(stringResource(R.string.md_paste_code)) }
        else {
            OutlinedTextField(input, { input = it }, singleLine = true, label = { Text(stringResource(R.string.md_paste_code)) })
            Button(onClick = { read(input) }, enabled = input.isNotBlank()) { Text(stringResource(R.string.code_link)) }
        }
    }
    offer?.let { pending ->
        FittedSheet(onDismissRequest = { if (!busy) offer = null }) {
            Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Icon(Icons.Default.Smartphone, null)
                Text(stringResource(R.string.md_confirm_title), style = MaterialTheme.typography.headlineSmall)
                Text(stringResource(R.string.md_confirm_message))
                error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                Row(horizontalArrangement = Arrangement.End, modifier = Modifier.fillMaxWidth()) {
                    TextButton(onClick = { offer = null; error = null }, enabled = !busy) { Text(stringResource(R.string.ui_cancel)) }
                    Button(onClick = {
                        busy = true
                        scope.launch {
                            try {
                                withContext(Dispatchers.IO) { repo.approveDevice(pending) }
                                offer = null
                                onLinked()
                            } catch (e: CancellationException) { throw e }
                            catch (e: Exception) {
                                if (e is YoException.Replaced) repo.command { throw e }
                                error = if (e is YoException.Refused && e.status.toInt() in setOf(400, 404, 409, 410))
                                    invalidCode else friendlyError(context, e)
                            } finally { busy = false }
                        }
                    }, enabled = !busy) { Text(stringResource(R.string.code_link)) }
                }
            }
        }
    }
    friend?.let { name ->
        AlertDialog(onDismissRequest = { friend = null },
            title = { Text(stringResource(R.string.md_add_title, name.uppercase())) },
            text = { Text(stringResource(R.string.md_add_message, name)) },
            confirmButton = { TextButton(onClick = {
                friend = null
                onFriendAdded()
                repo.command { add(name) }
            }) { Text(stringResource(R.string.md_add)) } },
            dismissButton = { TextButton(onClick = { friend = null }) { Text(stringResource(R.string.ui_cancel)) } })
    }
}

@Composable internal fun ReplaceDevice(request: Repository.Replacement) {
    var picked by remember(request) { mutableStateOf<UByte?>(null) }
    androidx.compose.ui.window.Dialog(onDismissRequest = { request.choice.complete(null) },
        properties = androidx.compose.ui.window.DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize(), color = Amethyst, contentColor = Color.White) {
            Column(Modifier.fillMaxSize().safeDrawingPadding().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(R.string.md_limit), style = MaterialTheme.typography.headlineMedium)
                Text(stringResource(R.string.md_limit_message))
                Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).selectableGroup()) {
                    request.devices.forEach { device ->
                        val selected = picked == device.number
                        ListItem(selected = selected, onClick = { picked = device.number },
                            supportingContent = { Text(lastSeen(device)) }, leadingContent = { DeviceIcon(device) },
                            trailingContent = { RadioButton(selected, onClick = null) }) { Text(stringResource(device.platform())) }
                    }
                }
                Button(onClick = { picked?.let { request.choice.complete(it) } }, enabled = picked != null,
                    colors = ButtonDefaults.buttonColors(containerColor = Color.White, contentColor = Color.Black)) {
                    Text(stringResource(R.string.md_replace))
                }
                TextButton(onClick = { request.choice.complete(null) }) { Text(stringResource(R.string.ui_cancel), color = Color.White) }
            }
        }
    }
}

package app.justyo.android

// Connecting a service (skills/ifttt, spec 3.5): its page opens yo://connect?origin=…&ticket=… (a phone's "Open Yo") or
// https://justyo.co/connect/approve?t=… (a computer's QR code). Asked first, with the pairing code the page shows; on
// Allow the account key signs the ticket bound to the page's origin, the page gets the signature and goes back to the
// service itself. This app only says yes or no; it never opens the service's return link.

import android.net.Uri
import android.util.Base64
import androidx.compose.animation.AnimatedContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Check
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject
import uniffi.yo.YoClient
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest

data class ConnectRequest(val origin: String, val ticket: String) {
    /** The service's bot on Yo, added as a friend on Allow. */
    val service get() = "IFTTT"
    /** The four digits the page shows: SHA-256("yo pairing " || ticket), its first 4 bytes, mod 10000. */
    val code: String get() {
        val digest = MessageDigest.getInstance("SHA-256").digest(("yo pairing $ticket").toByteArray())
        val number = digest.take(4).fold(0L) { n, b -> (n shl 8) or (b.toLong() and 0xFF) }
        return "%04d".format(number % 10000)
    }
}

enum class ConnectOutcome { CONNECTED, REFUSED, EXPIRED, FAILED }

private val Ink = Color(0xFF2B0F38)

object Connect {
    /** The pages that may ask: Yo's own site; a debug build also a local one (-PyoConnectOrigin). */
    val origins: Set<String> get() = buildSet {
        add("https://justyo.co")
        if (BuildConfig.DEBUG && BuildConfig.CONNECT_ORIGIN.isNotEmpty()) add(BuildConfig.CONNECT_ORIGIN)
    }

    fun isConnect(uri: Uri): Boolean = when (uri.scheme?.lowercase()) {
        "yo" -> uri.host?.lowercase() == "connect"
        "https", "http" -> uri.host?.lowercase() in setOf("justyo.co", "www.justyo.co") && uri.pathSegments.firstOrNull() == "connect"
        else -> false
    }

    fun request(uri: Uri): ConnectRequest? {
        val (origin, ticket) = if (uri.scheme?.lowercase() == "yo") uri.getQueryParameter("origin") to uri.getQueryParameter("ticket")
        else (if (uri.scheme?.lowercase() == "https") "https://justyo.co" else null) to
            uri.getQueryParameter("t")?.takeIf { uri.pathSegments.getOrNull(1) == "approve" }
        if (origin == null || origin !in origins || ticket == null) return null
        if (ticket.length !in 1..255 || !ticket.all { it in 'a'..'z' || it in 'A'..'Z' || it in '0'..'9' || it == '-' || it == '_' }) return null
        return ConnectRequest(origin, ticket)
    }

    /** Says yes (signed) or no to the page. Blocking: off the main thread. */
    fun answer(request: ConnectRequest, allow: Boolean, client: YoClient): ConnectOutcome {
        val body = JSONObject().put("ticket", request.ticket).put("allow", allow)
        if (allow) {
            val signature = runCatching { client.signConnect(request.origin, request.ticket) }.getOrNull() ?: return ConnectOutcome.FAILED
            body.put("user", client.userId())
            body.put("signature", Base64.encodeToString(signature, Base64.URL_SAFE or Base64.NO_PADDING or Base64.NO_WRAP))
        }
        return try {
            val connection = URL(request.origin + "/connect/approve").openConnection() as HttpURLConnection
            try {
                connection.requestMethod = "POST"
                connection.connectTimeout = 20_000
                connection.readTimeout = 20_000
                connection.doOutput = true
                connection.useCaches = false
                connection.setRequestProperty("Content-Type", "application/json")
                connection.outputStream.use { it.write(body.toString().toByteArray()) }
                when (connection.responseCode) {
                    200 -> if (allow) ConnectOutcome.CONNECTED else ConnectOutcome.REFUSED
                    410 -> ConnectOutcome.EXPIRED
                    else -> ConnectOutcome.FAILED
                }
            } finally { connection.disconnect() }
        } catch (_: Exception) { ConnectOutcome.FAILED }
    }
}

/** Asks before a service is connected: who, what it may do, and the page's pairing code to check. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable fun ConnectSheet(model: YoViewModel, request: ConnectRequest) = YoSystemTheme {
    val scope = rememberCoroutineScope()
    var working by remember(request) { mutableStateOf(false) }
    var outcome by remember(request) { mutableStateOf<ConnectOutcome?>(null) }
    fun close() { model.connect.value = null }
    fun answer(allow: Boolean) {
        working = true
        scope.launch {
            val result = withContext(Dispatchers.IO) { model.repository.answerConnect(request, allow) }
            working = false
            if (allow) outcome = result else close()
        }
    }
    ModalBottomSheet(
        // Swiped away while it asks: a no, sent on the way out; the sheet goes at once whatever the answer.
        onDismissRequest = {
            if (outcome == null && !working) scope.launch(Dispatchers.IO) { model.repository.answerConnect(request, false) }
            close()
        },
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
    ) {
        Column(Modifier.fillMaxWidth().navigationBarsPadding().padding(start = 24.dp, end = 24.dp, top = 8.dp, bottom = 16.dp),
            horizontalAlignment = Alignment.CenterHorizontally) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp),
                modifier = Modifier.clearAndSetSemantics { contentDescription = "Yo and ${request.service}" }) {
                Tile("Yo", Amethyst, if (outcome == ConnectOutcome.CONNECTED) 44 else 38)
                AnimatedContent(outcome == ConnectOutcome.CONNECTED, label = "joined") { joined ->
                    Icon(if (joined) Icons.Default.Check else Icons.Default.Add, contentDescription = null,
                        tint = if (joined) Amethyst else MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.size(30.dp))
                }
                Tile(request.service, Ink, 21)
            }
            Spacer(Modifier.height(28.dp))
            val (title, message) = when (outcome) {
                ConnectOutcome.CONNECTED -> stringResource(R.string.connect_connected) to stringResource(R.string.connect_connected_message, request.service)
                ConnectOutcome.REFUSED -> stringResource(R.string.connect_refused) to stringResource(R.string.connect_refused_message, request.service)
                ConnectOutcome.EXPIRED -> stringResource(R.string.connect_expired) to stringResource(R.string.connect_expired_message, request.service)
                ConnectOutcome.FAILED -> stringResource(R.string.connect_failed) to stringResource(R.string.connect_failed_message)
                null -> stringResource(R.string.connect_title, request.service) to stringResource(R.string.connect_message, request.service)
            }
            Text(title, style = TextStyle(fontFamily = YoFont, fontSize = 28.sp), textAlign = TextAlign.Center)
            Spacer(Modifier.height(10.dp))
            Text(message, style = MaterialTheme.typography.bodyLarge, textAlign = TextAlign.Center,
                color = MaterialTheme.colorScheme.onSurfaceVariant)
            if (outcome == null) {
                Spacer(Modifier.height(28.dp))
                Text(stringResource(R.string.connect_code_label).uppercase(), style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
                Spacer(Modifier.height(10.dp))
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp),
                    modifier = Modifier.clearAndSetSemantics { contentDescription = request.code.toList().joinToString(" ") }) {
                    request.code.forEach { digit ->
                        Box(Modifier.size(width = 48.dp, height = 60.dp).background(Amethyst.copy(alpha = 0.12f), RoundedCornerShape(14.dp)),
                            contentAlignment = Alignment.Center) {
                            Text(digit.toString(), style = TextStyle(fontFamily = YoFont, fontSize = 28.sp, color = Amethyst))
                        }
                    }
                }
                Spacer(Modifier.height(10.dp))
                Text(stringResource(R.string.connect_code_check, request.service), style = MaterialTheme.typography.bodySmall,
                    textAlign = TextAlign.Center, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Spacer(Modifier.height(32.dp))
            if (outcome == null) {
                Button(onClick = { answer(true) }, enabled = !working, shape = CircleShape,
                    modifier = Modifier.fillMaxWidth().heightIn(min = 56.dp),
                    colors = ButtonDefaults.buttonColors(containerColor = Amethyst, contentColor = Color.White)) {
                    if (working) CircularProgressIndicator(Modifier.size(22.dp), color = Color.White, strokeWidth = 2.dp)
                    else Text(stringResource(R.string.connect_allow), style = TextStyle(fontFamily = YoFont, fontSize = 20.sp))
                }
                TextButton(onClick = { answer(false) }, enabled = !working) { Text(stringResource(R.string.connect_cancel)) }
            } else {
                Button(onClick = ::close, shape = CircleShape, modifier = Modifier.fillMaxWidth().heightIn(min = 56.dp),
                    colors = ButtonDefaults.buttonColors(containerColor = Amethyst, contentColor = Color.White)) {
                    Text(stringResource(R.string.connect_done), style = TextStyle(fontFamily = YoFont, fontSize = 20.sp))
                }
            }
        }
    }
}

@Composable private fun Tile(text: String, color: Color, size: Int) {
    Box(Modifier.size(104.dp).shadow(16.dp, RoundedCornerShape(28.dp), ambientColor = color, spotColor = color)
        .background(color, RoundedCornerShape(28.dp)), contentAlignment = Alignment.Center) {
        Text(text, style = TextStyle(fontFamily = YoFont, fontSize = size.sp, color = Color.White))
    }
}

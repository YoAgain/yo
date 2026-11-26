package app.justyo.android

import android.net.Uri
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID

fun utf8Limit(text: String, bytes: Int): String {
    val out = StringBuilder()
    var used = 0
    text.codePoints().forEachOrdered { point ->
        val chars = String(Character.toChars(point))
        val size = chars.toByteArray().size
        if (used + size <= bytes) { out.append(chars); used += size } else used = bytes + 1
    }
    return out.toString()
}
/** yo://services/NAME/connect?code=CODE, from a service's connect page (justyo.co/services/NAME/connect). */
data class ConnectRequest(val service: String, val code: String)
fun isServiceLink(uri: Uri) = uri.scheme?.lowercase() == "yo" && uri.host?.lowercase() == "services"
/** Off until services launch: a connect link is swallowed and nothing is asked or sent. */
const val SERVICES_CONNECT = false
fun connectRequest(uri: Uri): ConnectRequest? {
    if (!SERVICES_CONNECT) return null
    if (!isServiceLink(uri) || uri.pathSegments.size != 2 || uri.pathSegments[1] != "connect") return null
    val service = uri.pathSegments[0].takeIf { Regex("[A-Za-z0-9._]{1,32}").matches(it) } ?: return null
    val code = uri.getQueryParameter("code")?.takeIf { Regex("[A-Za-z0-9_-]{43}").matches(it) } ?: return null
    return ConnectRequest(service.uppercase(), code)
}
fun inviteName(uri: Uri): String? {
    if (isServiceLink(uri)) return null
    val raw = when (uri.scheme?.lowercase()) {
        "yo" -> uri.encodedAuthority?.let(Uri::decode)  // yo://NAME, the website's "Open in Yo"
        "https", "http" -> uri.pathSegments.firstOrNull()?.takeIf { uri.host?.lowercase() in setOf("justyo.co", "www.justyo.co") }
        else -> null
    }
    // yo://camera and yo://open are Yo's own (iOS's intents), not names.
    if (uri.scheme?.lowercase() == "yo" && raw?.lowercase() in setOf("camera", "open")) return null
    return raw?.takeIf { it.isNotBlank() && it.lowercase() !in setOf("privacy", "security") }
}
/** Yo codes the camera sees: each asks once, as an opened invite link does (iOS CodeSightings). A code stays answered
 *  while it is in view, also after its dialog was closed; out of view for [AwayMs] (or the camera closed), it may ask
 *  again. [sighted] with `canAsk` false (a dialog up, a video recording) asks nothing; a new code waits for the next. */
class CodeSightings {
    private val asked = HashMap<String, Long>()
    fun sighted(names: List<String>, now: Long, canAsk: Boolean): String? {
        asked.entries.removeAll { now - it.value >= AwayMs }
        var ask: String? = null
        for (name in names) {
            val key = name.lowercase()
            if (key in asked) asked[key] = now
            else if (canAsk && ask == null) { asked[key] = now; ask = name }
        }
        return ask
    }
    fun reset() = asked.clear()
    companion object { const val AwayMs = 3000L }
}
data class Friend(
    val key: String = UUID.randomUUID().toString(), val name: String, val color: Int,
    val user: String? = null, val group: String? = null, val blocked: Boolean = false,
    val gone: Boolean = false, val request: Boolean = false, val keyChanged: Boolean = false,
    val undo: Boolean = false, val undoSeconds: Int = 2, val bot: Boolean = false,
) {
    /** The name as shown: a group's with its "#", a bot's with its "/" (the core's rule). */
    val title get() = uniffi.yo.listBarTitle(name, group != null, bot)
    val target get() = group?.let { "group:$it" } ?: user ?: name
    fun json() = JSONObject().put("key", key).put("name", name).put("color", color)
        .put("user", user).put("group", group).put("blocked", blocked).put("gone", gone)
        .put("request", request).put("keyChanged", keyChanged).put("undo", undo).put("undoSeconds", undoSeconds).put("bot", bot)
    companion object {
        fun from(j: JSONObject) = Friend(j.getString("key"), j.getString("name"), j.getInt("color"),
            j.stringOrNull("user"), j.stringOrNull("group"), j.optBoolean("blocked"), j.optBoolean("gone"),
            j.optBoolean("request"), j.optBoolean("keyChanged"), j.optBoolean("undo"), j.optInt("undoSeconds", 2), j.optBoolean("bot"))
    }
}
fun JSONObject.stringOrNull(key: String) = if (isNull(key)) null else optString(key).takeIf { it.isNotEmpty() }
fun JSONArray.objects() = (0 until length()).map { getJSONObject(it) }
data class Received(val id: String, val target: String, val sender: String, val media: uniffi.yo.YoMedia,
    val video: Boolean, val duration: Int, val at: Long, val ready: Boolean = false, val consumed: Boolean = false)
data class AppState(
    val byFriend: Map<String,Long> = emptyMap(), val lastSent: Map<String,Long> = emptyMap(),val lastReceived: Map<String,Long> = emptyMap(),
    val deletion: uniffi.yo.ListDeletion? = null,
    val listRows: Map<String, uniffi.yo.ListRow> = emptyMap(), val lockedRow: String? = null,
    val loading: Boolean = true, val username: String? = null, val friends: List<Friend> = emptyList(),
    val busy: Boolean = false, val error: String? = null, val update: Boolean = false,
    val broken: Boolean = false, val secured: Boolean = false, val recovery: String? = null,
    /** iOS `wordsSaved`: false after "Turn Off Recovery Key" until the new key is saved; `recoveryPasskey`: the passkey
     *  still opens the backup then (no red badge, as iOS's `recoveryNeedsAttention`). */
    val recoverySaved: Boolean = true, val recoveryPasskey: Boolean = false,
    /** `updating`: an old install signs in again ("Yo is updating…"). `googleBackup`: wanted in Google's cloud;
     *  `backedUp`: it is there, end-to-end encrypted (else the red dot on the menu). */
    val updating: Boolean = false, val googleBackup: Boolean = !BuildConfig.FLOSS, val backedUp: Boolean = !BuildConfig.FLOSS,
    /** Why it is not there: no screen lock, or Google's device backup is off (Block Store's cloud copy rides on it). */
    val backupMissingReason: CloudMissing? = null,
    val media: List<Received> = emptyList(),
    val leftHanded: Boolean = false, val requestsOpen: Boolean = true,
    val sent: Long = 0, val received: Long = 0, val pendingInvite: String? = null,
    val keepConnection: Boolean = false,
    val lastSync: Long? = null, val firstYo: Boolean = false, val notice: String? = null, val appIcon: String? = null,
    /** A scanned or tapped invite, verified by the hub, waiting for "Add & Yo" (not saved). */
    val invitation: Invitation? = null,
    /** A service to connect, waiting for "Allow" (not saved). */
    val connecting: ConnectRequest? = null,
    /** A contact whose account key changed (spec 2.1), said once in a dialog; `confirm`: their Yos wait for it (was verified). */
    val keyChanged: KeyChange? = null,
    /** A group with nobody else in it any more, after a Yo to it: asked whether to delete it (iOS). */
    val emptyGroup: Friend? = null,
    /** The list's top line from the core: "Offline", a failed send's words (tap: try again), else none. */
    val banner: String? = null,
    /** An unread reply from the Yo team in the support chat (`/v1/help`, by the install id): the red dot on the menu,
     *  Send Feedback until the chat is opened. */
    val supportUnread: Boolean = false,
) {
    /** What the list and the screens over it draw: without the counters and clocks every Yo and every sync change
     *  (stats, last sync). A Yo then recomposes the list once, not three or four times (each 4-16 ms on the A34). */
    fun forScreen() = copy(sent = 0, received = 0, byFriend = emptyMap(), lastSent = emptyMap(), lastReceived = emptyMap(), lastSync = null)
    /** Wanted in Google's cloud but not there (no screen lock, or Google's backup off): the red dot. Turned off by choice is no warning. */
    val backupMissing: Boolean get() = googleBackup && !backedUp
    /** The red 1 (iOS `recoveryNeedsAttention`): not secured, or the recovery key turned off and the new one not
     *  saved while no passkey opens the backup either. */
    val recoveryNeedsAttention: Boolean get() = !secured || !recoverySaved && !recoveryPasskey
}

data class KeyChange(val friend: Friend, val confirm: Boolean)

/** An invite's friend, as the hub spells it: `known` when already on the list. */
data class Invitation(val name: String, val user: String?, val known: Boolean,
    /** What is wrong instead ("No Such User", "That’s You"): the dialog says so, with only OK. */
    val problem: String? = null)

fun JSONObject.longMap(key: String): Map<String,Long> = optJSONObject(key)?.let { values -> values.keys().asSequence().associateWith { values.getLong(it) } }.orEmpty()

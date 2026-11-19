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
fun inviteName(uri: Uri): String? {
    val raw = when (uri.scheme?.lowercase()) {
        "yo" -> uri.encodedAuthority?.let(Uri::decode)  // yo://NAME, the website's "Open in Yo"
        "https", "http" -> uri.pathSegments.firstOrNull()?.takeIf { uri.host?.lowercase() in setOf("justyo.co", "www.justyo.co") }
        else -> null
    }
    return raw?.takeIf { it.isNotBlank() && it.lowercase() !in setOf("privacy", "security") }
}
data class Friend(
    val key: String = UUID.randomUUID().toString(), val name: String, val color: Int,
    val user: String? = null, val group: String? = null, val blocked: Boolean = false,
    val gone: Boolean = false, val request: Boolean = false, val keyChanged: Boolean = false,
    val undo: Boolean = false, val undoSeconds: Int = 2,
) {
    val target get() = group?.let { "group:$it" } ?: user ?: name
    fun json() = JSONObject().put("key", key).put("name", name).put("color", color)
        .put("user", user).put("group", group).put("blocked", blocked).put("gone", gone)
        .put("request", request).put("keyChanged", keyChanged).put("undo", undo).put("undoSeconds", undoSeconds)
    companion object {
        fun from(j: JSONObject) = Friend(j.getString("key"), j.getString("name"), j.getInt("color"),
            j.stringOrNull("user"), j.stringOrNull("group"), j.optBoolean("blocked"), j.optBoolean("gone"),
            j.optBoolean("request"), j.optBoolean("keyChanged"), j.optBoolean("undo"), j.optInt("undoSeconds", 2))
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
    /** `updating`: an old install signs in again ("Yo is updating…"). `googleBackup`: wanted in Google's cloud;
     *  `backedUp`: it is there, end-to-end encrypted (else the red dot on the menu). */
    val updating: Boolean = false, val googleBackup: Boolean = true, val backedUp: Boolean = true,
    /** Why it is not there: no screen lock, or Google's device backup is off (Block Store's cloud copy rides on it). */
    val backupMissingReason: CloudMissing? = null,
    val media: List<Received> = emptyList(),
    val leftHanded: Boolean = false, val requestsOpen: Boolean = true,
    val sent: Long = 0, val received: Long = 0, val pendingInvite: String? = null,
    val lastSync: Long? = null, val firstYo: Boolean = false, val notice: String? = null, val appIcon: String? = null,
    /** A scanned or tapped invite, verified by the hub, waiting for "Add & Yo" (not saved). */
    val invitation: Invitation? = null,
) {
    /** Wanted in Google's cloud but not there (no screen lock, or Google's backup off): the red dot. Turned off by choice is no warning. */
    val backupMissing: Boolean get() = googleBackup && !backedUp
}

/** An invite's friend, as the hub spells it: `known` when already on the list. */
data class Invitation(val name: String, val user: String, val known: Boolean)

fun JSONObject.longMap(key: String): Map<String,Long> = optJSONObject(key)?.let { values -> values.keys().asSequence().associateWith { values.getLong(it) } }.orEmpty()

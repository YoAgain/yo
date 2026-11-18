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
    val valid = when (uri.scheme?.lowercase()) {
        "yo" -> uri.host.equals("add", true)
        "https", "http" -> uri.host?.lowercase() in setOf("justyo.co", "www.justyo.co")
        else -> false
    }
    return uri.pathSegments.firstOrNull()?.takeIf { valid && it.isNotBlank() && !it.equals("privacy", true) }
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
    val media: List<Received> = emptyList(),
    val leftHanded: Boolean = false, val saveSent: Boolean = false, val requestsOpen: Boolean = true,
    val sent: Long = 0, val received: Long = 0, val pendingInvite: String? = null,
    val lastSync: Long? = null, val firstYo: Boolean = false, val notice: String? = null, val appIcon: String? = null,
)

fun JSONObject.longMap(key: String): Map<String,Long> = optJSONObject(key)?.let { values -> values.keys().asSequence().associateWith { values.getLong(it) } }.orEmpty()

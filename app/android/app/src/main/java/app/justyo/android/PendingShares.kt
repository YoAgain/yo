package app.justyo.android

import android.content.Context
import android.net.Uri
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID

/** Target-independent, encrypted staging. A provider's temporary grant is needed only during import. */
internal class PendingShares(private val context: Context,private val vault: Vault,private val accountGuard: Mutex,private val session: ()->Long) {
    private val gate=Mutex()
    private fun manifest()=vault.read("share-inbox")?.let { JSONArray(it.toString(Charsets.UTF_8)).objects() }.orEmpty()
    private fun save(rows: List<JSONObject>)=vault.write("share-inbox",JSONArray(rows).toString().toByteArray())
    suspend fun items(): List<Uri> = gate.withLock { manifest().map { uri(it.getString("id")) } }
    suspend fun import(source: Uri, video: Boolean): Uri {
        val version=session()
        val prepared=if(video) MediaFiles.video(context,source) else MediaFiles.photo(context,source)
        return gate.withLock { accountGuard.withLock {
            currentCoroutineContext().ensureActive()
            check(version==session())
            val rows=manifest()
            check(rows.size<10)
            val id=UUID.randomUUID().toString()
            vault.write("share-$id",prepared.bytes)
            try { save(rows+JSONObject().put("id",id).put("width",prepared.width).put("height",prepared.height).put("video",prepared.video).put("duration",prepared.duration)) }
            catch(e: Exception) { vault.delete("share-$id");throw e }
            uri(id)
        } }
    }
    suspend fun open(uri: Uri): PreparedMedia = gate.withLock {
        val id=uri.schemeSpecificPart
        val row=manifest().first { it.getString("id")==id }
        PreparedMedia(checkNotNull(vault.read("share-$id")),row.getInt("width"),row.getInt("height"),row.getBoolean("video"),row.getInt("duration"))
    }
    suspend fun remove(uri: Uri) = gate.withLock {
        val id=uri.schemeSpecificPart
        save(manifest().filter { it.getString("id")!=id });vault.delete("share-$id")
    }
    suspend fun clear() = gate.withLock {
        val rows=manifest();save(emptyList());rows.forEach { vault.delete("share-${it.getString("id")}") }
    }
    private fun uri(id: String)=Uri.fromParts("yo-share",id,null)
}

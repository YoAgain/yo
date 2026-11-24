package app.justyo.android

import android.content.Context
import android.util.Base64
import com.google.android.gms.tasks.Tasks
import com.google.android.play.core.integrity.IntegrityManagerFactory
import com.google.android.play.core.integrity.IntegrityTokenRequest
import com.google.firebase.FirebaseOptions
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import uniffi.yo.YoClient

internal object Attestation {
    private val started = AtomicBoolean(false)
    private fun now() = System.currentTimeMillis().toULong()

    fun check(context: Context, client: YoClient) {
        if (!started.compareAndSet(false, true)) return
        val project = FirebaseOptions.fromResource(context)?.gcmSenderId?.toLongOrNull() ?: return
        if (!client.attestDue(now())) return
        val token = try {
            val challenge = client.attestChallenge() ?: return
            // A classic request: only its token is encrypted with the app's own keys, which the hub holds (no call to
            // Google to read it). Its nonce: the challenge's hash, base64url.
            val hash = java.security.MessageDigest.getInstance("SHA-256").digest(challenge)
            val nonce = Base64.encodeToString(hash, Base64.URL_SAFE or Base64.NO_WRAP)
            val request = IntegrityTokenRequest.builder().setNonce(nonce).setCloudProjectNumber(project).build()
            Tasks.await(IntegrityManagerFactory.create(context).requestIntegrityToken(request), 60, TimeUnit.SECONDS).token()
        } catch (_: Exception) {
            runCatching { client.attestFailed(now()) }
            return
        }
        runCatching { client.attestSend(2u, token.toByteArray(Charsets.UTF_8)) }
    }
}

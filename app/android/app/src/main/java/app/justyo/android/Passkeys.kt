package app.justyo.android

import android.content.Context
import androidx.credentials.CredentialManager
import androidx.credentials.GetCredentialRequest
import androidx.credentials.GetPublicKeyCredentialOption
import org.json.JSONObject
import org.json.JSONArray
import java.security.MessageDigest

/** TODO: publish assetlinks.json for the package/signing certificate on justyo.app, then validate PRF
 * outputs across providers and connect assertion.userHandle + PRF to openBackup(YoDoor.Passkey).
 * Never mark an account secured merely because Credential Manager returned a credential. */
object Passkeys {
    const val AVAILABLE = false
    suspend fun signIn(context: Context) {
        check(AVAILABLE) { context.getString(R.string.passkeys_unavailable) }
        val request=JSONObject().put("challenge",Repository.encode(randomBytes(32)).trimEnd('=').replace('+','-').replace('/','_'))
            .put("rpId","justyo.app").put("allowCredentials",JSONArray()).put("userVerification","required")
            .put("extensions",JSONObject().put("prf",JSONObject().put("eval",JSONObject().put("first",
                android.util.Base64.encodeToString(MessageDigest.getInstance("SHA-256").digest("yo backup v1".toByteArray()),android.util.Base64.URL_SAFE or android.util.Base64.NO_WRAP or android.util.Base64.NO_PADDING)))))
        try {
            CredentialManager.create(context).getCredential(context,GetCredentialRequest(listOf(GetPublicKeyCredentialOption(request.toString()))))
        } catch (_: androidx.credentials.exceptions.NoCredentialException) {
            error(context.getString(R.string.no_passkey))
        }
        error(context.getString(R.string.passkey_pending))
    }
}

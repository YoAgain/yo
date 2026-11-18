package app.justyo.android

import android.content.Context
import android.util.Base64
import androidx.credentials.CreatePublicKeyCredentialRequest
import androidx.credentials.CreatePublicKeyCredentialResponse
import androidx.credentials.CredentialManager
import androidx.credentials.GetCredentialRequest
import androidx.credentials.GetPublicKeyCredentialOption
import androidx.credentials.PublicKeyCredential
import androidx.credentials.exceptions.NoCredentialException
import org.json.JSONArray
import org.json.JSONObject
import java.security.MessageDigest

internal object Passkeys {
    private const val RP = "justyo.app"
    private const val FLAGS = Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING
    private fun encode(bytes: ByteArray) = Base64.encodeToString(bytes, FLAGS)
    private fun decode(encoded: String) = Base64.decode(encoded, FLAGS)
    private fun challenge() = encode(randomBytes(32))

    data class Answer(val user: ByteArray, val prf: ByteArray)

    suspend fun create(context: Context, name: String, user: ByteArray): Answer {
        val request = JSONObject()
            .put("challenge", challenge())
            .put("rp", JSONObject().put("id", RP).put("name", "Yo"))
            .put("user", JSONObject().put("id", encode(user)).put("name", name).put("displayName", name))
            .put("pubKeyCredParams", JSONArray(listOf(-7, -257).map {
                JSONObject().put("type", "public-key").put("alg", it)
            }))
            .put("authenticatorSelection", JSONObject()
                .put("residentKey", "required").put("userVerification", "required"))
            .put("extensions", JSONObject().put("prf", JSONObject()))
        val response = CredentialManager.create(context).createCredential(
            context, CreatePublicKeyCredentialRequest(request.toString()),
        ) as CreatePublicKeyCredentialResponse
        val id = JSONObject(response.registrationResponseJson).getString("id")
        val answer = assert(context, id, user)
        check(answer.user.contentEquals(user))
        return answer
    }

    suspend fun assert(context: Context): Answer = assert(context, null, null)

    private suspend fun assert(context: Context, credentialId: String?, expectedUser: ByteArray?): Answer {
        val salt = MessageDigest.getInstance("SHA-256").digest("yo backup v1".toByteArray())
        val allowed = JSONArray()
        if (credentialId != null) allowed.put(JSONObject().put("type", "public-key").put("id", credentialId))
        val request = JSONObject().put("challenge", challenge()).put("rpId", RP)
            .put("allowCredentials", allowed).put("userVerification", "required")
            .put("extensions", JSONObject().put("prf", JSONObject().put("eval",
                JSONObject().put("first", encode(salt)))))
        val credential = try {
            CredentialManager.create(context).getCredential(
                context, GetCredentialRequest(listOf(GetPublicKeyCredentialOption(request.toString()))),
            ).credential as PublicKeyCredential
        } catch (_: NoCredentialException) {
            error(context.getString(R.string.no_passkey))
        }
        val response = JSONObject(credential.authenticationResponseJson)
        if (credentialId != null) check(response.getString("id") == credentialId)
        val value = response.optJSONObject("clientExtensionResults")?.optJSONObject("prf")
            ?.optJSONObject("results")?.optString("first").orEmpty()
        val prf = decode(value)
        require(prf.size == 32) { context.getString(R.string.passkey_prf_required) }
        val handle = response.getJSONObject("response").optString("userHandle").takeUnless { it.isBlank() || it == "null" }
        val user = handle?.let(::decode) ?: checkNotNull(expectedUser)
        require(user.size == 16)
        return Answer(user, prf)
    }
}

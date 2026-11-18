package app.justyo.android

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import java.io.File
import java.security.KeyStore
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec
import uniffi.yo.StateStore
import uniffi.yo.YoException

/** Every caller runs on Dispatchers.IO. A failed read is never treated as an empty vault. */
class Vault(private val context: Context) {
    private val directory by lazy { File(context.noBackupFilesDir, "vault").apply { mkdirs() } }
    private val keys by lazy { KeyStore.getInstance("AndroidKeyStore").apply { load(null) } }
    private val alias = "yo.wrap.v1"
    private fun wrappingKey(): SecretKey {
        (keys.getKey(alias, null) as? SecretKey)?.let { return it }
        // A missing key with existing ciphertext requires recovery, never replacement.
        check(directory.listFiles().orEmpty().isEmpty()) { context.getString(R.string.keystore_missing) }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
            init(KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).setKeySize(256).build())
            generateKey()
        }
    }
    @Synchronized fun read(name: String): ByteArray? {
        val file = File(directory, name)
        if (!file.exists() && !File(directory, "$name.bak").exists()) return null
        val data = AtomicFile(file).readFully()
        require(data.size >= 29) { context.getString(R.string.vault_damaged) }
        return Cipher.getInstance("AES/GCM/NoPadding").run {
            init(Cipher.DECRYPT_MODE, wrappingKey(), GCMParameterSpec(128, data.copyOfRange(0, 12)))
            updateAAD(name.toByteArray())
            doFinal(data.copyOfRange(12, data.size))
        }
    }
    @Synchronized fun write(name: String, value: ByteArray) {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, wrappingKey())
        cipher.updateAAD(name.toByteArray())
        atomicWrite(File(directory, name), cipher.iv + cipher.doFinal(value))
    }
    @Synchronized fun secret(name: String): ByteArray = read(name) ?: randomBytes(32).also { write(name, it) }
    @Synchronized fun delete(name: String) = AtomicFile(File(directory, name)).delete()
    @Synchronized fun clear() { directory.listFiles().orEmpty().forEach { it.delete() } }
}

fun randomBytes(size: Int) = ByteArray(size).also { SecureRandom().nextBytes(it) }
fun atomicWrite(file: File, data: ByteArray) {
    file.parentFile?.mkdirs()
    val atomic = AtomicFile(file)
    val stream = atomic.startWrite()
    try { stream.write(data); atomic.finishWrite(stream) } catch (error: Exception) { atomic.failWrite(stream); throw error }
}

class CoreState(private val context: Context) : StateStore {
    private val file by lazy { File(context.noBackupFilesDir, "yo-state") }
    override fun save(blob: ByteArray) {
        try { atomicWrite(file, blob) } catch (_: Exception) { throw YoException.Save(context.getString(R.string.save_state_failed)) }
    }
    fun load(): ByteArray? = if (file.exists() || File(file.path + ".bak").exists()) AtomicFile(file).readFully() else null
    fun delete() = AtomicFile(file).delete()
}

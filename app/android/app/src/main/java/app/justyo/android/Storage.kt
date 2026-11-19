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
import uniffi.yo.StateDb
import uniffi.yo.StateDiagnostics

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
    @Synchronized fun clear() { directory.listFiles().orEmpty().filter { it.name != "state-key" }.forEach { it.delete() } }
}

fun randomBytes(size: Int) = ByteArray(size).also { SecureRandom().nextBytes(it) }
fun atomicWrite(file: File, data: ByteArray) {
    file.parentFile?.mkdirs()
    val atomic = AtomicFile(file)
    val stream = atomic.startWrite()
    try { stream.write(data); atomic.finishWrite(stream) } catch (error: Exception) { atomic.failWrite(stream); throw error }
}

/**
 * The account's state: one SQLite database in the core's current schema. Files of older schemas are removed once at
 * launch (`migrate`): that install is signed out, and its saved recovery key signs it in again.
 */
class CoreState(context: Context, private val diagnostics: StateDiagnostics) {
    private val dir = context.noBackupFilesDir
    private val path = File(dir, "yo-state-v3.sqlite")
    private var db: StateDb? = null

    fun exists(): Boolean = path.exists()

    /** Removes an older install's state files; true when that install had no current state (it is signed out). */
    fun migrate(): Boolean {
        val old = dir.listFiles().orEmpty().filter { it.name.startsWith("yo-state") && !it.name.startsWith("yo-state-v3") }
        old.forEach { it.delete() }
        return old.isNotEmpty() && !exists()
    }

    @Synchronized fun database(key: ByteArray): StateDb =
        db ?: StateDb.openFresh(path.path, key, diagnostics).also { db = it }

    fun delete(key: ByteArray?) {
        if (key != null && exists()) runCatching { database(key).clear() }
        synchronized(this) { db = null }
        dir.listFiles().orEmpty().filter { it.name.startsWith("yo-state-v3") }.forEach { it.delete() }
    }
}

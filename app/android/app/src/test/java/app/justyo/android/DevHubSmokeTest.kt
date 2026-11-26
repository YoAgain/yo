package app.justyo.android

import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import uniffi.yo.*
import java.io.File
import java.util.UUID

/** Explicit opt-in (YO_DEV_SMOKE=1): one synthetic dev-hub account, reopened from its state and its backup, deleted. */
class DevHubSmokeTest {
    @Test(timeout = 120000) fun registrationStateAndRecoveryBackup() {
        assumeTrue(System.getenv("YO_DEV_SMOKE") == "1")
        val transport = HubTransport("unreachable") { error("Dev hub requires a newer build") }
        val diagnostics = object : StateDiagnostics { override fun failure(kind: String) = Unit }
        val path = File.createTempFile("yo-smoke", ".sqlite").apply { delete() }
        val key = randomBytes(32)
        val name = "Android" + UUID.randomUUID().toString().replace("-", "").take(9)
        val db = StateDb.openFresh(path.path, key, diagnostics)
        var account: YoClient? = null
        try {
            val made = YoClient.registerUsernameDb(transport, db, key, null, name, null, null, null, null, 0u, "", null)
            account = made
            assertEquals(name, made.username())
            val recovery = newRecoveryKey()
            val plain = "{\"synthetic\":true}".toByteArray()
            made.putBackup(plain, recovery, YoPasskeyDoor.None)
            assertArrayEquals(plain, openBackup(transport, null, YoDoor.RecoveryKey(recovery), Hub.hubKeys.map { Repository.decodeUrl(it) })!!.sealed)
            made.shutdown()
            account = YoClient.openDb(transport, db, key, false, null)
            assertEquals(name, account.username())
        } finally {
            account?.let { runCatching { it.deleteAccount() }; it.shutdown() }
            path.parentFile?.listFiles { file -> file.name.startsWith(path.name) }?.forEach { it.delete() }
        }
    }
}

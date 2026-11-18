package app.justyo.android

import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import uniffi.yo.*
import java.util.UUID

/** Explicit opt-in: creates one synthetic dev-hub account, then deletes it. Never contacts production. */
class DevHubSmokeTest {
    @Test(timeout=120000) fun registrationSealedStateAndRecoveryBackup() {
        assumeTrue(System.getenv("YO_DEV_SMOKE") == "1")
        val transport=HubTransport { error("Dev hub requires a newer build") }
        var savedBlob: ByteArray?=null
        val store=object: StateStore { override fun save(blob: ByteArray) { savedBlob=blob.copyOf() } }
        val key=randomBytes(32)
        val name="Android"+UUID.randomUUID().toString().replace("-","").take(9)
        var account: YoClient?=null
        try {
            val made=YoClient.registerUsername(transport,store,key,name,null,null,null,0u,"")
            account=made
            made.setHubKeys(listOf("LPiW1TOvv_ricbn7bXmS_fNdVbgtEw_3o7OdelvEdVo", "nFQuDMd-2Ueoec6oYG-Ty7lHQELq1OHs4TVODmPmryg").map { java.util.Base64.getUrlDecoder().decode(it) })
            assertEquals(name,made.username())
            assertNotNull(savedBlob)
            val recovery=newRecoveryKey()
            val plain="{\"synthetic\":true}".toByteArray()
            made.putBackup(randomBytes(32),plain,recovery,YoPasskeyDoor.None)
            val opened=openBackup(transport,null,YoDoor.RecoveryKey(recovery))
            assertNotNull(opened)
            assertArrayEquals(plain,opened!!.sealed)
            val saved=savedBlob!!.copyOf()
            made.shutdown()
            val restored=YoClient.restore(transport,store,key,saved)
            account=restored
            assertEquals(name,restored.username())
        } finally {
            // A lost registration answer can still leave a sealed registration to finish and clean up.
            if(account==null) savedBlob?.let { saved -> account=runCatching { YoClient.restore(transport,store,key,saved) }.getOrNull() }
            account?.let { try { it.deleteAccount() } finally { it.shutdown() } }
        }
    }
}

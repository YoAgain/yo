package app.justyo.android

import org.junit.Assert.*
import org.junit.Test
import uniffi.yo.*

class CoreContractTest {
    @Test fun utf8LimitsNeverSplitAnEmoji() {
        assertEquals("Yo",utf8Limit("Yo🌈",5))
        assertEquals("Yo🌈",utf8Limit("Yo🌈!",6))
        assertEquals("",utf8Limit("🌈",3))
        assertEquals("é",utf8Limit("éé",3))
    }
    @Test fun anonymousTrafficHasSeparatePoolOnlyForAnonymousRoutes() {
        assertTrue(HubTransport.anonymousRoute("POST","/v1/inbox/recipient"))
        assertFalse(HubTransport.anonymousRoute("POST","/v1/inbox/ack"))
        assertFalse(HubTransport.anonymousRoute("GET","/v1/inbox/recipient"))
        assertTrue(HubTransport.anonymousRoute("GET","/v1/blobs/blob?token=opaque"))
        assertFalse(HubTransport.anonymousRoute("POST","/v1/register"))
        assertFalse(HubTransport.anonymousRoute("GET","/v1/backup"))
    }
    @Test fun recoveryKeysUseTheActualSignalCore() {
        val key=newRecoveryKey()
        assertEquals(64,key.length)
        assertEquals(key,recoveryKeyParse(key.uppercase().chunked(4).joinToString(" - ")))
        assertNull(recoveryKeyParse("invalid"))
    }
    @Test fun usernamesUseSharedCoreRules() {
        assertEquals(usernameSkeleton("YoGPT"),usernameSkeleton("Y0GPT"))
        assertEquals("YoGPT",usernameCanonical("YoGPT"))
        assertFalse(usernameValid("123"))
        assertFalse(usernameValid("#️⃣Mia"))
        assertFalse(usernameValid("M@ia"))
        assertEquals("Mia", usernameCanonical("#️⃣M@ia"))
    }
    @Test fun rustCallsKotlinTransportAndReturnsTypedError() {
        val transport=object: HttpTransport {
            override fun request(method: String,path: String,headers: List<Header>,body: ByteArray): HttpResult {
                assertTrue(path.startsWith("/v1/"))
                return HttpResult(429u,byteArrayOf(),null)
            }
        }
        try { usernameFree(transport,"ExampleName");fail("Expected 429") }
        catch(error: YoException.Refused) { assertEquals(429,error.status.toInt()) }
    }
    @Test fun failedSavePreventsRegistrationLeavingTheDevice() {
        var requests=0
        val transport=object: HttpTransport {
            override fun request(method: String,path: String,headers: List<Header>,body: ByteArray): HttpResult {
                requests++
                return HttpResult(503u,byteArrayOf(),null)
            }
        }
        val disk=object: StateStore {
            override fun save(blob: ByteArray) { throw YoException.Save("Synthetic full disk") }
        }
        try {
            YoClient.registerUsername(transport,disk,ByteArray(32){1},"ExampleName",null,null,null,0u,"")
            fail("Expected save error")
        } catch (_: YoException.Save) { assertEquals(0,requests) }
    }

}

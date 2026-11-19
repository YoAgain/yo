package app.justyo.android

import org.junit.Assert.*
import org.junit.Test
import uniffi.yo.*

/** The app's own rules and the binding itself; the core's rules are tested in Rust. */
class CoreContractTest {
    @Test fun utf8LimitsNeverSplitAnEmoji() {
        assertEquals("Yo", utf8Limit("Yo🌈", 5))
        assertEquals("Yo🌈", utf8Limit("Yo🌈!", 6))
        assertEquals("", utf8Limit("🌈", 3))
        assertEquals("é", utf8Limit("éé", 3))
    }

    @Test fun rustCallsKotlinTransportAndReturnsTypedError() {
        val transport = object : HttpTransport {
            override fun request(method: String, path: String, headers: List<Header>, body: ByteArray): HttpResult {
                assertTrue(path.startsWith("/v1/"))
                return HttpResult(429u, byteArrayOf(), null)
            }
        }
        try { usernameFree(transport, "ExampleName"); fail("Expected 429") }
        catch (error: YoException.Refused) { assertEquals(429, error.status.toInt()) }
    }

    @Test fun barTitlesComeFromTheCore() {
        val mia = Friend(name = "Mia", color = 0, user = "u")
        assertEquals("Yo Request from Mia", rowTitle(null, mia.copy(request = true)))
        assertEquals("#Hey", rowTitle(null, Friend(name = "Hey", color = 0, group = "g")))
        assertEquals("Mia", rowTitle(null, mia))
    }

    /** A Japanese keyboard's full-width letters keep the name (on the device kana too, through ICU from API 29). */
    @Test fun fullWidthLettersReadAsLatin() {
        assertEquals("ping", uniffi.yo.usernameCanonical(latinName("ｐｉｎｇ")))
        assertEquals("Mia", latinName("Mia"))
    }
}

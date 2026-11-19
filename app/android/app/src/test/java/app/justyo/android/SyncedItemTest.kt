package app.justyo.android

import org.junit.Assert.*
import org.junit.Test
import uniffi.yo.*

/** The Block Store item is the iOS iCloud Keychain item: the same JSON goes both ways through the core. */
class SyncedItemTest {
    @Test fun iosItemDecodesAndRoundTrips() {
        val ios = """{"username":"Mia","userID":"AQID","accountKey":"${"A".repeat(43)}=","byPasskey":false,
            "byCode":true,"recoveryKey":"key","version":3}""".toByteArray()
        val item = syncedItemUpgraded(syncedItemDecode(ios)!!)!!
        assertEquals("Mia", item.username)
        assertArrayEquals(byteArrayOf(1, 2, 3), item.userId)
        assertEquals(syncedItemVersion(), item.version)
        val marked = syncedItemDecode(syncedItemEncode(item.copy(signedOut = true)))!!
        assertEquals(true, marked.signedOut)
        assertArrayEquals(syncedItemEncode(item), syncedItemEncode(marked.copy(signedOut = null)))
        assertNull(syncedItemDecode("not an item".toByteArray()))
    }
}

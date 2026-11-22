package app.justyo.android

import org.junit.Assert.*
import org.junit.Test
import uniffi.yo.YoException

class FallbackCodeTest {
    @Test fun aFallbackCountsWhatTheInboxPassFailedWith() {
        assertEquals("network", YoWork.fallbackCode(YoException.Network("Could not reach the hub")))
        assertEquals("hubStatus/503", YoWork.fallbackCode(YoException.Refused(503u)))
        assertEquals("protocol", YoWork.fallbackCode(YoException.Protocol("bad envelope")))
        assertEquals("state", YoWork.fallbackCode(YoException.State()))
        assertEquals("save", YoWork.fallbackCode(YoException.Save("disk full")))
        assertEquals("other/IllegalStateException", YoWork.fallbackCode(IllegalStateException("private text")))
    }
    @Test fun onlyTheNetworkIsExpected() {
        assertEquals(setOf("network"), CrashReports.EXPECTED_FALLBACKS)
    }
}

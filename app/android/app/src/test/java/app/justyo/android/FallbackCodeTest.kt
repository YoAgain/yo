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
    @Test fun skipsOnlyTheExpected() {
        for (code in listOf("network","hubStatus/426","hubStatus/429"))
            assertTrue(code, CrashReports.noise(code))
        for (code in listOf("hubStatus/500","hubStatus/502","hubStatus/503","hubStatus/401","protocol","save"))
            assertFalse(code, CrashReports.noise(code))
    }
}

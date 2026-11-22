package app.justyo.android

import org.junit.Assert.assertEquals
import org.junit.Test

/** A see-through photo goes on white, never black (HEIC and JPEG as sent have no alpha), as iOS sends it. */
class PhotoFlattenTest {
    @Test fun transparentIsWhite() {
        assertEquals(0xFFFFFFFF.toInt(), onWhite(0x00000000))
        assertEquals(0xFFFFFFFF.toInt(), onWhite(0x009B59B6))
    }

    @Test fun opaqueStays() {
        assertEquals(0xFF9B59B6.toInt(), onWhite(0xFF9B59B6.toInt()))
        assertEquals(0xFF000000.toInt(), onWhite(0xFF000000.toInt()))
    }

    @Test fun halfTransparentBlendsTowardWhite() {
        assertEquals(0xFF7F7F7F.toInt(), onWhite(0x80000000.toInt()))
        assertEquals(0xFFFF7F7F.toInt(), onWhite(0x80FF0000.toInt()))
    }
}

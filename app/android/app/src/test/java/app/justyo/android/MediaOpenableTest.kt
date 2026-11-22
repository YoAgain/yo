package app.justyo.android

import org.junit.Assert.assertEquals
import org.junit.Test
import uniffi.yo.YoMedia

/** A bar's sign opens only media downloaded and written whole: one still loading is never handed to the viewer. */
class MediaOpenableTest {
    private val blob = YoMedia(ByteArray(16), ByteArray(32), ByteArray(32), 1u, 1u.toUShort(), 1u.toUShort())
    private fun item(id: String, target: String = "a", at: Long, ready: Boolean, consumed: Boolean = false) =
        Received(id, target, "s", blob, video = true, duration = 3000, at = at, ready = ready, consumed = consumed)

    @Test fun onlyReadyMediaOpensOldestFirst() {
        val media = listOf(item("late", at = 3, ready = true), item("loading", at = 1, ready = false),
            item("early", at = 2, ready = true), item("other", target = "b", at = 0, ready = true),
            item("seen", at = 0, ready = true, consumed = true))
        assertEquals(listOf("early", "late"), MediaQueue.openable(media, "a").map { it.id })
    }

    @Test fun aGroupBarNamesTheNewestWaitingSender() {
        val media = listOf(item("old", at = 1, ready = true).copy(sender = "mia"),
            item("new", at = 5, ready = false).copy(sender = "ben"),
            item("seen", at = 9, ready = true, consumed = true).copy(sender = "ida"),
            item("elsewhere", target = "b", at = 7, ready = true).copy(sender = "max"))
        assertEquals("ben", MediaQueue.newestSender(media, "a"))
        assertEquals(null, MediaQueue.newestSender(media, "c"))
    }

    @Test fun nothingOpensWhileAllStillLoad() {
        assertEquals(emptyList<Received>(), MediaQueue.openable(listOf(item("v", at = 1, ready = false)), "a"))
    }
}

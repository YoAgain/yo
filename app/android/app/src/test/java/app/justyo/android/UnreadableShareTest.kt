package app.justyo.android

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class UnreadableShareTest {
    @Test fun aMissingGrantOrFileIsAnUnreadableShare() {
        assertTrue(unreadableShare(SecurityException("app.justyo.android.dev has no access to content://media/external/images/media/39")))
        assertTrue(unreadableShare(java.io.FileNotFoundException("content://com.android.chrome.FileProvider/images/x.png")))
    }

    @Test fun otherFailuresKeepTheirOwnWords() {
        assertFalse(unreadableShare(IllegalStateException("too long")))
        assertFalse(unreadableShare(java.io.IOException("disk full")))
    }
}

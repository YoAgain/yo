package app.justyo.android

import org.junit.Assert.assertEquals
import org.junit.Test

/** A request's tap is its yes whatever media wait to go (iOS FriendList+Sending `send(to:)`). */
class BarTapTest {
    private fun tap(request: Boolean = false, blocked: Boolean = false, gone: Boolean = false, shares: Boolean = false,
                    picked: Boolean = false, camera: Boolean = false, unsent: Boolean = false) =
        barTap(request, blocked, gone, shares, picked, camera, unsent)

    @Test fun requestAcceptsWithMediaWaiting() {
        assertEquals(BarTap.Accept, tap(request = true))
        assertEquals(BarTap.Accept, tap(request = true, camera = true, picked = true))
        assertEquals(BarTap.Accept, tap(request = true, shares = true))
        assertEquals(BarTap.Accept, tap(request = true, shares = true, camera = true))
    }

    @Test fun mediaGoToFriends() {
        assertEquals(BarTap.StageShares, tap(shares = true, picked = true, camera = true))
        assertEquals(BarTap.StagePicked, tap(picked = true, camera = true, unsent = true))
        assertEquals(BarTap.TakePhoto, tap(camera = true, unsent = true))
        assertEquals(BarTap.Retry, tap(unsent = true))
        assertEquals(BarTap.Yo, tap())
    }

    @Test fun sharedMediaGoToNobodyBlockedOrGone() {
        assertEquals(BarTap.Nothing, tap(shares = true, blocked = true))
        assertEquals(BarTap.Nothing, tap(shares = true, gone = true))
        assertEquals(BarTap.Yo, tap(gone = true))
    }
}

package app.justyo.android

import org.junit.Assert.*
import org.junit.Test
import uniffi.yo.*

/** Exercise the rebuilt native library, including identity and viewport behavior across the binding. */
class ListModelTest {
    private fun YoList.seed() = friends(0u,listOf(ListFriend("a","A",false,false),ListFriend("g","Group",false,true)))
    @Test fun glimpsesLeaveYosFreshButSeenYosExpireWhileAway() {
        val model = YoList()
        model.seed()
        model.received(0u, "a", ListKind.YO, "event", null)
        model.tick(300u)
        assertEquals(1u, model.tick(10000u).rows.first { it.id == "a" }.yoCount)
        model.rowsVisible(10000u, listOf("a"))
        model.camera(10999u, true)
        assertEquals(1u, model.tick(20000u).rows.first { it.id == "a" }.yoCount)
        model.camera(20000u, false)
        model.foreground(21000u, false)
        assertEquals(0u, model.foreground(60000u, true).rows.first { it.id == "a" }.yoCount)
    }
    @Test fun groupSendersRetainUserIdentityWhenNamesCollideAndHeal() {
        val model=YoList();model.seed()
        model.received(0u,"g",ListKind.YO,"one","user-one")
        model.received(1u,"g",ListKind.YO,"two","user-two")
        model.senderNamed(2u,"user-one","Same")
        val row=model.senderNamed(3u,"user-two","Same").rows.first { it.id=="g" }
        assertEquals(listOf("user-one","user-two"),row.senders.map { it.user })
        assertEquals(listOf("Same","Same"),row.senders.map { it.name })
    }
    @Test fun pressFreezesPromotionUntilReleaseAndCoreOwnsSentLifetime() {
        val model=YoList();model.seed()
        model.press(0u,"a",true)
        model.received(0u,"g",ListKind.YO,"event",null)
        assertEquals("a",model.tick(400u).rows.first().id)
        assertEquals("g",model.press(401u,"a",false).rows.first().id)
        assertEquals("Sent Yo",model.sendFinished(500u,"a",ListKind.PHOTO).rows.first { it.id=="a" }.title)
        assertEquals("A",model.tick(2001u).rows.first { it.id=="a" }.title)
    }
}

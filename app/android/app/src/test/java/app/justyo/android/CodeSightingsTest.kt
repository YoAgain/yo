package app.justyo.android

import org.junit.Assert.*
import org.junit.Test

/** A Yo code in the camera's view asks once while it stays there (iOS tests/code-scan.sh). */
class CodeSightingsTest {
    @Test fun oneAskWhileInView() {
        val seen = CodeSightings()
        assertEquals("Anna", seen.sighted(listOf("Anna"), 0, true))
        for (t in 1..50) assertNull(seen.sighted(listOf("anna"), t * 200L, true))
        assertNull(seen.sighted(listOf("Anna"), 10_000 + 2_900, true))
        assertEquals("Anna", seen.sighted(listOf("Anna"), 12_900 + CodeSightings.AwayMs, true))
        seen.reset()
        assertEquals("Anna", seen.sighted(listOf("Anna"), 16_000, true))
    }

    @Test fun blockedWaitsAndCodesTakeTurns() {
        val seen = CodeSightings()
        assertNull(seen.sighted(listOf("Ben"), 0, false))
        assertEquals("Ben", seen.sighted(listOf("Ben"), 200, true))
        assertEquals("Anna", seen.sighted(listOf("Anna", "Ben"), 400, true))
        assertNull(seen.sighted(listOf("Anna", "Cleo"), 600, false))
        assertEquals("Cleo", seen.sighted(listOf("Anna", "Cleo"), 800, true))
    }
}

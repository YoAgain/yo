package app.justyo.android

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class TestLabTest {
    @Test fun onlyTrueMeansTestLab() {
        assertTrue(TestLab.isRunning("true"))
        assertFalse(TestLab.isRunning(null))
        assertFalse(TestLab.isRunning("false"))
        assertFalse(TestLab.isRunning(""))
    }
}

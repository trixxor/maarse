package dk.maarse.app

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ResumeTest {
    /**
     * Seen on a Galaxy S25: at a cold start the app began a Bluetooth connection, and coming to the front closed it
     * and began another 8 ms later. Android never answered the second one, so the app showed "Connecting" forever.
     */
    @Test
    fun `an attempt that has just begun is left alone`() {
        assertTrue(attemptStillFresh(connecting = true, sinceMs = 0))
        assertTrue(attemptStillFresh(connecting = true, sinceMs = 5_000))
    }

    @Test
    fun `an old attempt or a dead link is replaced`() {
        assertFalse(attemptStillFresh(connecting = true, sinceMs = 10_000))
        assertFalse(attemptStillFresh(connecting = false, sinceMs = 0))
    }
}

class RadioAddressTest {
    /**
     * Seen on a Galaxy S25: after the person had forgotten the radio, Android took its address to be a public one and
     * every attempt timed out (status 147). A Må:rse radio uses a random static address (its two highest bits set).
     */
    @Test
    fun `a radio address is told apart from a public one`() {
        assertTrue(isRandomStatic("E9:EF:F6:9B:02:B0"))
        assertTrue(isRandomStatic("C0:00:00:00:00:01"))
        assertFalse(isRandomStatic("A4:CB:8F:A5:0A:94"))
        assertFalse(isRandomStatic("not an address"))
    }
}

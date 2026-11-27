package dk.maarse.app

import org.junit.Assert.assertEquals
import org.junit.Test

class BleLostTest {
    private val forget = "The radio no longer accepts this phone's pairing (was its Bluetooth code changed?). " +
        "Forget Maarse-b002 in the phone's Bluetooth settings, then connect again"

    /** Seen on a Galaxy S25 after the code was changed over USB: status 5, again and again. */
    @Test
    fun `a paired radio that refuses the pairing says what to do`() {
        for (status in listOf(5, 137)) assertEquals(forget, bleLostText("Maarse-b002", status, paired = true))
    }

    @Test
    fun `any other lost connection only gives its status`() {
        assertEquals("Bluetooth connection lost (status 5)", bleLostText("Maarse-b002", 5, paired = false))
        assertEquals("Bluetooth connection lost (status 8)", bleLostText("Maarse-b002", 8, paired = true))
        assertEquals("Bluetooth connection lost (status 19)", bleLostText("Maarse-b002", 19, paired = true))
    }
}

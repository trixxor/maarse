package dk.maarse.app

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Test

/** The commands and the event for a recording made by a radio must match maarse-core (`companion.rs`) and the web client. */
class RadioRecProtoTest {
    private fun bytes(vararg v: Int) = ByteArray(v.size) { v[it].toByte() }

    @Test
    fun startCommandMatchesTheOtherSides() {
        // 863000 kHz = 0x000D2B18, step 110, 600 s, every 2 s, threshold 6 dB; length 11 = type + 10 bytes.
        assertArrayEquals(bytes(11, 0, 0x3e, 0x18, 0x2b, 0x0d, 0x00, 110, 0, 0x58, 0x02, 20, 6), Cmd.sweepRecStart(863_000, 110, 600, 20, 6))
        assertArrayEquals(bytes(1, 0, 0x3f), Cmd.sweepRecStop())
        assertArrayEquals(bytes(1, 0, 0x43), Cmd.sweepRecStatus())
    }

    @Test
    fun stateEventIsRead() {
        val ev = decodeEvent(bytes(0xb9, 1, 42, 0, 0x58, 0x02, 21, 0, 0, 0))
        assertEquals(Ev.SweepRecState(1, 42, 600, 21, 0), ev)
    }
}

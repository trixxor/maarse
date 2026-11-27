package dk.maarse.app

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** The administrator list: what a person may type, and the frames to and from the node (0x0d, 0x94). */
class AdminListTest {
    private val a = "0123456789abcdef0123456789abcdef"
    private val b = "ffeeddccbbaa99887766554433221100"

    @Test fun readsOneAddressPerLine() {
        assertEquals(listOf(a, b), parseAdminList("  ${a.uppercase()}\n\n$b \n"))
        assertEquals(emptyList<String>(), parseAdminList(" \n "))
        assertEquals(listOf(a), parseAdminList("$a\n$a"))
    }

    @Test fun refusesALineThatIsNotAnAddress() {
        assertNull(parseAdminList("$a\n${b.dropLast(1)}"))
        assertNull(parseAdminList("maarse:link?addr=$a"))
    }

    @Test fun refusesMoreThanFour() {
        val five = (1..5).joinToString("\n") { "$it".repeat(32) }
        assertNull(parseAdminList(five))
    }

    @Test fun framesMatchTheProtocol() {
        // length (u16) | type | 16 bytes per administrator
        assertArrayEquals(byteArrayOf(33, 0, 0x0d) + a.unhex() + b.unhex(), Cmd.setAdmins(listOf(a, b)))
        assertArrayEquals(byteArrayOf(1, 0, 0x0d), Cmd.setAdmins(emptyList()))
        assertEquals(Ev.Admins(listOf(a, b)), decodeEvent(byteArrayOf(0x94.toByte()) + a.unhex() + b.unhex()))
        assertEquals(Ev.Admins(emptyList()), decodeEvent(byteArrayOf(0x94.toByte())))
    }
}

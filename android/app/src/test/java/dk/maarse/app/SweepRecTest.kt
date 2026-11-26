package dk.maarse.app

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/** The Kotlin port of the spectrum recording must give the bytes and groups the Rust code gives (maarse-core/tests/data). */
class SweepRecTest {
    private val dir = listOf("../../maarse-core/tests/data", "../maarse-core/tests/data", "maarse-core/tests/data").map(::File).first { it.isDirectory }

    @Test
    fun matchesTheRustFile() {
        val input = File(dir, "sweeprec-golden-sweeps.bin").readBytes()
        val want = File(dir, "sweeprec-golden.msr").readBytes()
        val rec = SweepRec.Recorder(SweepRec.Config(863_000, 110, 300, 1000, 6))
        var o = 0
        while (o + 68 <= input.size) {
            val t = (input[o].toLong() and 255) or ((input[o + 1].toLong() and 255) shl 8) or ((input[o + 2].toLong() and 255) shl 16) or ((input[o + 3].toLong() and 255) shl 24)
            rec.push(t, IntArray(64) { input[o + 4 + it].toInt() and 255 }, false)
            o += 68
        }
        assertArrayEquals(want, rec.finish())
    }

    @Test
    fun groupsMatchTheRustText() {
        val want = File(dir, "sweeprec-golden-groups.txt").readText()
        val r = SweepRec.parse(File(dir, "sweeprec-golden.msr").readBytes())
        val got = r.groups.joinToString("") { g -> "${g.kind} ${g.severity} ${g.b0} ${g.b1} ${g.count} ${g.airMs} ${g.firstMs} ${g.peakDb} ${g.intervalMs}\n" }
        assertEquals(want, got)
    }

    @Test
    fun rejectsOtherFiles() {
        assertTrue(runCatching { SweepRec.parse(ByteArray(40)) }.exceptionOrNull() is SweepRec.BadFile)
    }
}

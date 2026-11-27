package dk.maarse.app

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** The plain status: what a missing battery means, and the few facts shown in everyday words. */
class PlainStatusTest {
    @Test fun aMeasuredBattery() {
        assertEquals(Battery.Read(90, 4028, true), batteryOf(mapOf(49 to 4028, 85 to 90, 82 to 1), StatusSource.REMOTE))
    }

    @Test fun missingBatterySaysWhy() {
        // The node reports its power source but no voltage: it measures nothing.
        assertEquals(Battery.NoReading, batteryOf(mapOf(82 to 0), StatusSource.REMOTE))
        // Neither of them: that part of the reply has not arrived.
        assertEquals(Battery.NotReceived, batteryOf(mapOf(1 to 60), StatusSource.REMOTE))
        // A computer or phone without a battery radio says nothing.
        assertNull(batteryOf(mapOf(82 to 1), StatusSource.HOST))
    }

    @Test fun factsInEverydayWords() {
        val rows = plainRows(mapOf(7 to 1, 1 to 3720, 35 to 11, 41 to 25, 13 to 42, 86 to 285, 90 to 0))
        assertEquals(
            listOf(
                "Works as" to "Repeater: passes messages on", "Running for" to "1 h 2 min", "Last signal received" to "Good (11 dB)",
                "Airtime used, last hour" to formatStat(41, 25), "Packets passed on" to "42", "Chip temperature" to formatStat(86, 285),
            ),
            rows,
        )
    }

    @Test fun aClientPassesNothingOnAndWeakSignalsSaySo() {
        val rows = plainRows(mapOf(7 to 0, 13 to 0, 35 to -12, 90 to 2), airtime = false)
        assertEquals(
            listOf("Works as" to "Client: passes nothing on", "Last signal received" to "Weak (-12 dB)", "Restarts after a crash" to "2"),
            rows,
        )
    }
}

package dk.maarse.app

// The plain status: the battery and a few facts in everyday words. Same wording as the PC web client.

/** Where a status comes from: it decides what a missing battery means. HOST: a phone or computer with a radio. */
enum class StatusSource { HOST, RADIO, REMOTE }

sealed class Battery {
    data class Read(val percent: Int?, val mv: Int, val external: Boolean) : Battery()
    /** The node reports its power source but no voltage: it measures nothing. */
    object NoReading : Battery()
    /** That part of the status has not arrived. */
    object NotReceived : Battery()
}

/** Null: nothing to say (a phone or computer without a battery radio). */
fun batteryOf(s: Map<Int, Int>, source: StatusSource): Battery? {
    val mv = s[49] ?: 0
    return when {
        mv > 0 -> Battery.Read(s[85], mv, s[82] == 1)
        source == StatusSource.HOST -> null
        82 in s -> Battery.NoReading
        else -> Battery.NotReceived
    }
}

private val ROLE_PLAIN = listOf(
    "Client: passes nothing on", "Repeater: passes messages on", "Relay: passes on and carries messages",
    "Anchor: repeater that also splits long routes", "Auto: relay when no repeater is near",
)

private fun signalWord(snr: Int) = if (snr >= 0) "Good" else if (snr >= -7) "Fair" else "Weak"

/** Label and value of every fact the status holds. `airtime` false where the screen shows airtime already. */
fun plainRows(s: Map<Int, Int>, airtime: Boolean = true): List<Pair<String, String>> = buildList {
    s[7]?.let { add("Works as" to (ROLE_PLAIN.getOrNull(it) ?: "$it")) }
    s[1]?.let { add("Running for" to formatStat(1, it)) }
    s[35]?.let { add("Last signal received" to "${signalWord(it)} ($it dB)") }
    if (airtime) s[41]?.let { add("Airtime used, last hour" to formatStat(41, it)) }
    s[83]?.let { add("Radios heard directly" to "$it") }
    s[13]?.takeIf { s[7] != 0 }?.let { add("Packets passed on" to "$it") }
    s[86]?.let { add("Chip temperature" to formatStat(86, it)) }
    s[90]?.takeIf { it > 0 }?.let { add("Restarts after a crash" to "$it") }
    s[118]?.takeIf { it > 0 }?.let { add("Radio change on trial" to "${it / 60} min ${it % 60} s left to confirm") }
}

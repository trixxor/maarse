package dk.maarse.app

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.location.LocationManager
import kotlinx.coroutines.flow.MutableStateFlow
import java.io.File

/**
 * History for the Experimental pages: battery and temperature, signal per neighbour and where the
 * radio heard others (the map's coverage layer). Everything stays on the phone, in plain text files
 * in the app's folder, and is cut to [KEEP_MS] (7 days) when the app starts and once an hour.
 * Samples come once a minute while the app runs and a radio answers.
 */
object History {
    private const val KEEP_MS = 7L * 24 * 3600 * 1000
    private const val EVERY_MS = 60_000L

    /** One reading of the radio: battery percent and mV, chip temperature (1/10 °C), noise floor (dBm), external power. */
    data class Power(val t: Long, val pct: Int?, val mv: Int?, val tempDc: Int?, val noise: Int?, val ext: Boolean)

    /** What one neighbour's signal was at a time. */
    data class Link(val t: Long, val prefix: String, val snr: Int, val rssi: Int)

    /** A place where a neighbour was heard: degrees x 10^6 and SNR. */
    data class Spot(val t: Long, val latE6: Int, val lonE6: Int, val snr: Int)

    val power = MutableStateFlow<List<Power>>(emptyList())
    val links = MutableStateFlow<List<Link>>(emptyList())
    val spots = MutableStateFlow<List<Spot>>(emptyList())

    private lateinit var app: Context
    private lateinit var prefs: android.content.SharedPreferences
    private var lastPower = 0L
    private var lastLinks = 0L
    private var lastTrim = 0L

    /** Keep history at all (Experimental > Keep history). On by default. */
    var keep: Boolean
        get() = prefs.getBoolean("keep", true)
        set(v) = prefs.edit().putBoolean("keep", v).apply()

    /** Draw the coverage layer on the map (Experimental > Show coverage on the map). Off by default. */
    var showCoverage: Boolean
        get() = prefs.getBoolean("coverage", false)
        set(v) { prefs.edit().putBoolean("coverage", v).apply(); coverageOn.value = v }

    val coverageOn = MutableStateFlow(false)

    private fun file(name: String) = File(app.filesDir, name)

    fun init(ctx: Context) {
        app = ctx.applicationContext
        prefs = app.getSharedPreferences("experimental", Context.MODE_PRIVATE)
        coverageOn.value = showCoverage
        val cut = System.currentTimeMillis() - KEEP_MS
        power.value = read("history-power.txt") { f ->
            if (f.size < 6) null else Power(f[0].toLong(), f[1].toIntOrNull(), f[2].toIntOrNull(), f[3].toIntOrNull(), f[4].toIntOrNull(), f[5] == "1")
        }.filter { it.t >= cut }
        links.value = read("history-links.txt") { f -> if (f.size < 4) null else Link(f[0].toLong(), f[1], f[2].toInt(), f[3].toInt()) }.filter { it.t >= cut }
        spots.value = read("history-spots.txt") { f -> if (f.size < 4) null else Spot(f[0].toLong(), f[1].toInt(), f[2].toInt(), f[3].toInt()) }.filter { it.t >= cut }
        rewrite()
    }

    private fun <T> read(name: String, parse: (List<String>) -> T?): List<T> =
        runCatching { file(name).readLines().mapNotNull { l -> runCatching { parse(l.split(',')) }.getOrNull() } }.getOrDefault(emptyList())

    private fun rewrite() {
        runCatching {
            file("history-power.txt").writeText(power.value.joinToString("") { "${it.t},${it.pct ?: ""},${it.mv ?: ""},${it.tempDc ?: ""},${it.noise ?: ""},${if (it.ext) 1 else 0}\n" })
            file("history-links.txt").writeText(links.value.joinToString("") { "${it.t},${it.prefix},${it.snr},${it.rssi}\n" })
            file("history-spots.txt").writeText(spots.value.joinToString("") { "${it.t},${it.latE6},${it.lonE6},${it.snr}\n" })
        }
    }

    private fun trimIfDue(now: Long) {
        if (now - lastTrim < 3600_000) return
        lastTrim = now
        val cut = now - KEEP_MS
        power.value = power.value.filter { it.t >= cut }
        links.value = links.value.filter { it.t >= cut }
        spots.value = spots.value.filter { it.t >= cut }
        rewrite()
    }

    /** Called with every status frame; saves a sample at most once a minute. Only radio values (key 85 or 49 present). */
    fun onStats(s: Map<Int, Int>) {
        if (!::app.isInitialized || !keep) return
        val now = System.currentTimeMillis()
        if (now - lastPower < EVERY_MS) return
        val pct = s[85]; val mv = s[49]?.takeIf { it > 0 }
        if (pct == null && mv == null) return
        lastPower = now
        val p = Power(now, pct, mv, s[86], s[80], s[82] == 1)
        power.value = power.value + p
        runCatching { file("history-power.txt").appendText("${p.t},${p.pct ?: ""},${p.mv ?: ""},${p.tempDc ?: ""},${p.noise ?: ""},${if (p.ext) 1 else 0}\n") }
        trimIfDue(now)
    }

    /** Should the app ask the radio for its neighbours now (once a minute) even without a status screen open? */
    fun wantsNeighbours(): Boolean = ::app.isInitialized && keep && System.currentTimeMillis() - lastLinks >= EVERY_MS

    /** Called when a full neighbour list has arrived. */
    fun onNeighbours(list: List<Neighbor>) {
        if (!::app.isInitialized || !keep) return
        val now = System.currentTimeMillis()
        if (now - lastLinks < EVERY_MS - 5000) return
        lastLinks = now
        if (list.isEmpty()) return
        val add = list.map { Link(now, it.prefix, it.snr, it.rssi) }
        links.value = links.value + add
        runCatching { file("history-links.txt").appendText(add.joinToString("") { "${it.t},${it.prefix},${it.snr},${it.rssi}\n" }) }
        // The coverage layer: where we are, and the best signal heard here.
        location()?.let { (lat, lon) ->
            val best = list.maxOf { it.snr }
            val s = Spot(now, (lat * 1e6).toInt(), (lon * 1e6).toInt(), best)
            val last = spots.value.lastOrNull()
            // One spot per ~25 m at most, or when nothing was saved for 10 minutes.
            if (last == null || now - last.t > 600_000 || distanceM(last.latE6 / 1e6, last.lonE6 / 1e6, lat, lon) > 25) {
                spots.value = spots.value + s
                runCatching { file("history-spots.txt").appendText("${s.t},${s.latE6},${s.lonE6},${s.snr}\n") }
            }
        }
    }

    /** The phone's position if a fix is known and recent (2 minutes), else null. Needs the location permission. */
    private fun location(): Pair<Double, Double>? {
        if (app.checkSelfPermission(Manifest.permission.ACCESS_FINE_LOCATION) != PackageManager.PERMISSION_GRANTED &&
            app.checkSelfPermission(Manifest.permission.ACCESS_COARSE_LOCATION) != PackageManager.PERMISSION_GRANTED) return null
        val lm = app.getSystemService(Context.LOCATION_SERVICE) as LocationManager
        val l = lm.getProviders(true).mapNotNull { runCatching { lm.getLastKnownLocation(it) }.getOrNull() }.maxByOrNull { it.time } ?: return null
        if (System.currentTimeMillis() - l.time > 120_000) return null
        return l.latitude to l.longitude
    }

    fun distanceM(lat1: Double, lon1: Double, lat2: Double, lon2: Double): Double {
        val r = FloatArray(1)
        android.location.Location.distanceBetween(lat1, lon1, lat2, lon2, r)
        return r[0].toDouble()
    }

    /** Estimated battery use in percent per hour over the last [hours] hours, or null without enough data. */
    fun drainPerHour(hours: Int = 6): Double? {
        val now = System.currentTimeMillis()
        val v = power.value.filter { it.t >= now - hours * 3600_000L && it.pct != null && !it.ext }
        if (v.size < 5) return null
        val first = v.first(); val last = v.last()
        val h = (last.t - first.t) / 3600_000.0
        if (h < 0.25) return null
        val d = (first.pct!! - last.pct!!) / h
        return if (d > 0.05) d else null
    }
}

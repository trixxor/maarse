package dk.maarse.app

import android.Manifest
import android.content.Context
import android.content.pm.PackageManager
import android.location.Location
import android.location.LocationListener
import android.location.LocationManager
import android.os.Looper
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/**
 * Ping test: repeated trace routes with statistics, for range tests on a walk or a drive and to
 * check a route over time. Each ping is one trace route (command 0x20): the answer lists the
 * repeaters and the SNR for every hop both ways, and the time to the answer is the round trip.
 * Nothing new goes over the air. Same behaviour as the web client's Ping test (Diagnostics).
 */
object PingTest {
    enum class State { WAITING, OK, LOST, ERROR }

    data class Ping(
        val seq: Int,
        val at: Long,
        val state: State = State.WAITING,
        val rttMs: Long? = null,
        val hops: Int? = null,
        val outMin: Int? = null,
        val backMin: Int? = null,
        val path: String = "",
        val lat: Double? = null,
        val lon: Double? = null,
        val accM: Int? = null,
        val text: String = "",
    )

    data class Run(
        val dest: String,
        val count: Int,
        val intervalS: Int,
        val withPosition: Boolean,
        val started: Long,
        val pings: List<Ping> = emptyList(),
        val finished: Boolean = false,
    )

    data class Summary(
        val sent: Int,
        val answered: Int,
        val lost: Int,
        val lossPct: Int,
        val minMs: Long?,
        val avgMs: Long?,
        val maxMs: Long?,
        val farthestM: Int?,
        val baseIsTarget: Boolean,
    )

    /** A ping with no answer after this long counts as lost. */
    const val TIMEOUT_MS = 30_000L
    const val MIN_INTERVAL_S = 5

    val run = MutableStateFlow<Run?>(null)

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private var job: Job? = null
    private val lock = Any()
    /** Trace request id -> ping number. */
    private val reqs = java.util.concurrent.ConcurrentHashMap<Int, Int>()
    private var position: Triple<Double, Double, Int>? = null
    private var listener: LocationListener? = null
    private var manager: LocationManager? = null

    fun start(ctx: Context, dest: String, count: Int, intervalS: Int, withPosition: Boolean) {
        stop()
        val r = Run(dest, count.coerceIn(1, 100), intervalS.coerceIn(MIN_INTERVAL_S, 600), withPosition, System.currentTimeMillis())
        run.value = r
        position = null
        if (withPosition) startLocation(ctx.applicationContext)
        Node.note("Ping test to ${Node.label(dest)}: ${r.count} pings, one every ${r.intervalS} s")
        job = scope.launch {
            for (seq in 1..r.count) {
                sendPing(r, seq)
                if (seq < r.count) delay(r.intervalS * 1000L)
            }
            // All sent: the run ends when the last answer or time-out is in.
            while (run.value?.pings?.any { it.state == State.WAITING } == true) delay(500)
            finish()
        }
    }

    fun stop() {
        job?.cancel()
        synchronized(lock) {
            run.update { r -> r?.copy(pings = r.pings.map { if (it.state == State.WAITING) it.copy(state = State.LOST) else it }) }
            reqs.clear()
        }
        finish()
    }

    private fun finish() {
        stopLocation()
        run.update { it?.copy(finished = true) }
    }

    private fun sendPing(r: Run, seq: Int) {
        val req = Node.req()
        val pos = position ?: lastKnown()
        synchronized(lock) {
            reqs[req] = seq
            run.update { cur ->
                cur?.copy(pings = cur.pings + Ping(seq, System.currentTimeMillis(), lat = pos?.first, lon = pos?.second, accM = pos?.third))
            }
        }
        Node.send(Cmd.traceRoute(req, r.dest))
        scope.launch {
            delay(TIMEOUT_MS)
            synchronized(lock) {
                if (reqs.remove(req) != null) update(seq) { it.copy(state = State.LOST) }
            }
        }
    }

    private fun update(seq: Int, f: (Ping) -> Ping) {
        run.update { cur -> cur?.copy(pings = cur.pings.map { if (it.seq == seq && it.state == State.WAITING) f(it) else it }) }
    }

    /** Called for every trace answer; true if it belonged to a ping (then it is not listed as an ordinary trace). */
    fun onTrace(ev: Ev.Trace): Boolean {
        val seq = reqs.remove(ev.req) ?: return false
        synchronized(lock) {
            update(seq) {
                it.copy(
                    state = State.OK,
                    rttMs = System.currentTimeMillis() - it.at,
                    hops = ev.path.size,
                    outMin = ev.out.minOrNull(),
                    backMin = ev.back.minOrNull(),
                    path = ev.path.joinToString(" > ") { id -> idName(id) },
                )
            }
        }
        return true
    }

    /** Called for every error; true if it belonged to a ping. */
    fun onError(req: Int, text: String): Boolean {
        val seq = reqs.remove(req) ?: return false
        synchronized(lock) { update(seq) { it.copy(state = State.ERROR, text = text) } }
        return true
    }

    private fun idName(id: Int): String {
        val p = id.toString(16).padStart(4, '0')
        return Node.nodes.value.keys.firstOrNull { it.startsWith(p) }?.let { Node.label(it) } ?: "$p…"
    }

    // ---------- the phone's position ----------

    private fun hasPermission(ctx: Context) =
        ctx.checkSelfPermission(Manifest.permission.ACCESS_FINE_LOCATION) == PackageManager.PERMISSION_GRANTED

    private fun startLocation(ctx: Context) {
        if (!hasPermission(ctx)) return
        val lm = ctx.getSystemService(Context.LOCATION_SERVICE) as LocationManager
        manager = lm
        val l = LocationListener { loc: Location -> position = Triple(loc.latitude, loc.longitude, loc.accuracy.toInt()) }
        listener = l
        for (p in listOf(LocationManager.GPS_PROVIDER, LocationManager.NETWORK_PROVIDER)) {
            runCatching { if (lm.isProviderEnabled(p)) lm.requestLocationUpdates(p, 2000L, 0f, l, Looper.getMainLooper()) }
        }
        lastContext = ctx
    }

    private var lastContext: Context? = null

    private fun lastKnown(): Triple<Double, Double, Int>? {
        val ctx = lastContext ?: return null
        val lm = manager ?: return null
        if (!hasPermission(ctx)) return null
        val l = lm.getProviders(true).mapNotNull { runCatching { lm.getLastKnownLocation(it) }.getOrNull() }.maxByOrNull { it.time } ?: return null
        if (System.currentTimeMillis() - l.time > 30_000) return null
        return Triple(l.latitude, l.longitude, l.accuracy.toInt())
    }

    private fun stopLocation() {
        val lm = manager
        val l = listener
        if (lm != null && l != null) runCatching { lm.removeUpdates(l) }
        listener = null
        manager = null
        lastContext = null
    }

    // ---------- results ----------

    fun distanceM(lat1: Double, lon1: Double, lat2: Double, lon2: Double): Double {
        val r = 6371000.0
        val rad = Math.PI / 180
        val dLat = (lat2 - lat1) * rad
        val dLon = (lon2 - lon1) * rad
        val h = Math.sin(dLat / 2).let { it * it } + Math.cos(lat1 * rad) * Math.cos(lat2 * rad) * Math.sin(dLon / 2).let { it * it }
        return 2 * r * Math.asin(Math.sqrt(h))
    }

    /** Statistics for a run. The farthest answered ping is measured from the target if it shares its position, else from the first position. */
    fun summary(r: Run, target: Pair<Double, Double>?): Summary {
        val done = r.pings.filter { it.state != State.WAITING }
        val ok = r.pings.filter { it.state == State.OK }
        val rtts = ok.mapNotNull { it.rttMs }
        val base = target ?: r.pings.firstOrNull { it.lat != null && it.lon != null }?.let { it.lat!! to it.lon!! }
        val far = if (base == null) null else ok.filter { it.lat != null && it.lon != null }
            .maxOfOrNull { distanceM(base.first, base.second, it.lat!!, it.lon!!) }?.toInt()
        return Summary(
            sent = r.pings.size,
            answered = ok.size,
            lost = done.size - ok.size,
            lossPct = if (done.isEmpty()) 0 else Math.round(100.0 * (done.size - ok.size) / done.size).toInt(),
            minMs = rtts.minOrNull(),
            avgMs = if (rtts.isEmpty()) null else Math.round(rtts.average()),
            maxMs = rtts.maxOrNull(),
            farthestM = far,
            baseIsTarget = target != null,
        )
    }

    private fun cell(v: Any?): String {
        val s = v?.toString() ?: ""
        return if (s.any { it == ',' || it == '"' || it == '\n' }) "\"" + s.replace("\"", "\"\"") + "\"" else s
    }

    /** The run as CSV, the same columns as the web client. */
    fun csv(r: Run): String {
        val iso = java.text.SimpleDateFormat("yyyy-MM-dd'T'HH:mm:ss'Z'", java.util.Locale.ROOT).apply { timeZone = java.util.TimeZone.getTimeZone("UTC") }
        val head = listOf("time_iso", "target", "seq", "result", "rtt_ms", "hops", "worst_snr_out_db", "worst_snr_back_db", "lat", "lon", "accuracy_m", "route")
        val rows = r.pings.map {
            listOf(
                iso.format(java.util.Date(it.at)), r.dest, it.seq, it.state.name.lowercase(), it.rttMs, it.hops, it.outMin, it.backMin,
                it.lat, it.lon, it.accM, it.path,
            ).joinToString(",") { v -> cell(v) }
        }
        return (listOf(head.joinToString(",")) + rows).joinToString("\n") + "\n"
    }
}

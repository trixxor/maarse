package dk.maarse.app

import java.io.ByteArrayOutputStream

/**
 * Spectrum recordings ("MSR1"): a port of `maarse-core/src/sweeprec.rs`. `SweepRecTest` checks it byte for byte against
 * `maarse-core/tests/data/sweeprec-golden.*`. Change both together. The file layout is in `docs/protocol-spec.md`.
 */
object SweepRec {
    const val BINS = 64
    const val MAX_BUCKETS = 120
    private const val MAX_EVENTS = 200
    private const val MAX_GROUPS = 48
    private const val HEAD = 24
    private const val CELLS = BINS * MAX_BUCKETS
    const val STEP_DB = 3

    /** What to record. `intervalMs` is the time between the start of two sweeps. */
    class Config(val startKhz: Long, val stepKhz: Int, val durationS: Int, val intervalMs: Int, val thrDb: Int)

    /** A sweep where most steps read stronger than -60 dBm: a transmitter very close overloads the receiver. */
    fun isOverload(r: IntArray): Boolean {
        val valid = r.count { it < 248 }
        val strong = r.count { it < 120 }
        return valid >= 8 && strong * 10 >= valid * 6
    }

    /** Collects sweeps and makes the file. */
    class Recorder(cfg: Config) {
        val config = Config(cfg.startKhz, cfg.stepKhz, cfg.durationS, cfg.intervalMs.coerceIn(100, 25_500), cfg.thrDb)
        val bucketMs: Int
        val buckets: Int
        private var used = 0
        var sweeps = 0
            private set
        private val best = IntArray(CELLS) { 255 }
        private val sum = IntArray(CELLS)
        private val grid = IntArray(CELLS)
        private val cnt = IntArray(MAX_BUCKETS)

        init {
            require(cfg.durationS in 1..7800 && cfg.stepKhz > 0) { "bad config" }
            val durMs = cfg.durationS * 1000
            bucketMs = maxOf(durMs / MAX_BUCKETS, config.intervalMs)
            buckets = minOf((durMs + bucketMs - 1) / bucketMs, MAX_BUCKETS)
        }

        /** Takes one sweep; `tMs` is the time since the recording began. A masked sweep counts as time, not as a measurement. */
        fun push(tMs: Long, rssi: IntArray, masked: Boolean) {
            if (rssi.size != BINS) return
            val k = minOf((tMs / bucketMs).toInt(), buckets - 1).coerceAtLeast(0)
            used = maxOf(used, k + 1)
            sweeps = minOf(65535, sweeps + 1)
            if (masked) return
            var any = false
            for (b in 0 until BINS) {
                val v = rssi[b]
                if (v >= 248) continue
                any = true
                val i = b * MAX_BUCKETS + k
                if (v < best[i]) best[i] = v
                sum[i] = minOf(65535, sum[i] + v)
            }
            if (any) cnt[k] = minOf(65535, cnt[k] + 1)
        }

        fun finish(): ByteArray {
            val nb = maxOf(used, 1)
            val floor = IntArray(BINS) { 236 }
            for (b in 0 until BINS) {
                val tmp = ArrayList<Int>()
                for (k in 0 until nb) {
                    val i = b * MAX_BUCKETS + k
                    if (cnt[k] > 0 && best[i] != 255) tmp.add(minOf(247, sum[i] / cnt[k]))
                }
                if (tmp.isNotEmpty()) { tmp.sort(); floor[b] = tmp[tmp.size shr 1] }
            }
            val typical = floor.sorted()[BINS shr 1]
            for (b in 0 until BINS) floor[b] = maxOf(floor[b], maxOf(typical - 4, 0))
            val valid = (0 until nb).count { cnt[it] > 0 }
            val maxRaw = IntArray(BINS) { 255 }
            val duty = IntArray(BINS)
            for (b in 0 until BINS) {
                var hot = 0
                for (k in 0 until nb) {
                    val i = b * MAX_BUCKETS + k
                    if (cnt[k] == 0 || best[i] == 255) { grid[i] = 0; continue }
                    maxRaw[b] = minOf(maxRaw[b], best[i])
                    val d2 = floor[b] - best[i]
                    val v = if (d2 >= 2 * config.thrDb) ((d2 + 3) / 6).coerceIn(1, 15) else 0
                    grid[i] = v
                    if (v > 0) hot++
                }
                duty[b] = if (valid > 0) (hot * 100 + valid / 2) / valid else 0
                if (maxRaw[b] == 255) maxRaw[b] = floor[b]
                var k = 0
                while (k < nb) {
                    if (cnt[k] > 0) { k++; continue }
                    var k1 = k
                    while (k1 + 1 < nb && cnt[k1 + 1] == 0) k1++
                    val v = if (k > 0 && k1 + 1 < nb) minOf(grid[b * MAX_BUCKETS + k - 1], grid[b * MAX_BUCKETS + k1 + 1]) else 0
                    for (q in k..k1) grid[b * MAX_BUCKETS + q] = v
                    k = k1 + 1
                }
            }
            // Events: connected areas of the profile.
            val events = ArrayList<IntArray>() // k0, k1, b0, b1, peak
            val stack = IntArray(CELLS)
            for (start in 0 until CELLS) {
                val b0 = start / MAX_BUCKETS
                val k0 = start % MAX_BUCKETS
                if (k0 >= nb || grid[start] == 0 || grid[start] and 0x80 != 0) continue
                val ev = intArrayOf(k0, k0, b0, b0, 0)
                var sp = 0
                grid[start] = grid[start] or 0x80; stack[sp++] = start
                while (sp > 0) {
                    val x = stack[--sp]
                    val b = x / MAX_BUCKETS
                    val k = x % MAX_BUCKETS
                    val v = grid[x] and 0x7f
                    ev[2] = minOf(ev[2], b); ev[3] = maxOf(ev[3], b); ev[0] = minOf(ev[0], k); ev[1] = maxOf(ev[1], k)
                    ev[4] = maxOf(ev[4], v * STEP_DB)
                    fun visit(nbin: Int, nk: Int) {
                        val y = nbin * MAX_BUCKETS + nk
                        if (grid[y] != 0 && grid[y] and 0x80 == 0) { grid[y] = grid[y] or 0x80; stack[sp++] = y }
                    }
                    if (b > 0) visit(b - 1, k)
                    if (b + 1 < BINS) visit(b + 1, k)
                    if (k > 0) visit(b, k - 1)
                    if (k + 1 < nb) visit(b, k + 1)
                }
                if (events.size < MAX_EVENTS) events.add(ev)
            }
            for (i in 0 until CELLS) grid[i] = grid[i] and 0x7f
            // The file.
            val out = ByteArrayOutputStream()
            fun u8(v: Int) = out.write(v and 255)
            fun u16(v: Int) { u8(v); u8(v shr 8) }
            val durationS = minOf(65535, ((nb.toLong() * bucketMs + 999) / 1000).toInt())
            u8(77); u8(83); u8(82); u8(49); u8(BINS); u8(config.thrDb); u8(config.intervalMs / 100); u8(0)
            u16(durationS); u16(bucketMs and 0xffff); u16(nb); u16(sweeps)
            val sk = config.startKhz
            u8(sk.toInt()); u8((sk shr 8).toInt()); u8((sk shr 16).toInt()); u8((sk shr 24).toInt())
            u16(config.stepKhz); u8(0); u8(0)
            for (b in 0 until BINS) u8(-(floor[b] shr 1))
            for (b in 0 until BINS) u8(-(maxRaw[b] shr 1))
            for (b in 0 until BINS) u8(duty[b])
            val bm = IntArray((nb + 7) / 8)
            for (k in 0 until nb) if (cnt[k] == 0) bm[k shr 3] = bm[k shr 3] or (1 shl (k and 7))
            bm.forEach { u8(it) }
            val stream = ByteArrayOutputStream()
            for (b in 0 until BINS) {
                var k = 0
                while (k < nb) {
                    val v = grid[b * MAX_BUCKETS + k]
                    var run = 1
                    if (v == 0) {
                        while (k + run < nb && grid[b * MAX_BUCKETS + k + run] == 0 && run < 128) run++
                        stream.write(run - 1)
                    } else {
                        while (k + run < nb && grid[b * MAX_BUCKETS + k + run] == v && run < 8) run++
                        stream.write(0x80 or (v shl 3) or (run - 1))
                    }
                    k += run
                }
            }
            u16(stream.size()); out.write(stream.toByteArray())
            u16(events.size)
            for (e in events) { u8(e[0]); u8(e[1]); u8(e[2]); u8(e[3]); u8(e[4]); u8(0) }
            return out.toByteArray()
        }
    }

    /** A connected area of the profile: buckets `k0..k1`, steps `b0..b1`, strongest level over the floor in dB. */
    class Event(val k0: Int, val k1: Int, val b0: Int, val b1: Int, val peak: Int)

    enum class Kind { Constant, Periodic, Rising, Long, Pulses, Single }
    enum class Severity { Low, Medium, High }

    /** Events that look alike. */
    class Group(
        val kind: Kind, val severity: Severity, val b0: Int, val b1: Int, val count: Int,
        val airMs: Long, val firstMs: Long, val peakDb: Int, val intervalMs: Long, val members: List<Int>,
    )

    /** A recording that has been read. */
    class Recording(
        val thrDb: Int, val intervalMs: Int, val durationS: Int, val bucketMs: Int, val buckets: Int, val sweeps: Int,
        val startKhz: Long, val stepKhz: Int, val floor: IntArray, val max: IntArray, val duty: IntArray,
        val masked: BooleanArray, private val grid: IntArray, val events: List<Event>, val size: Int,
    ) {
        fun khz(bin: Int): Long = startKhz + stepKhz.toLong() * bin
        val lengthMs: Long get() = buckets.toLong() * bucketMs
        /** Level over the floor in 3 dB steps; 0 = under the threshold. */
        fun cell(bin: Int, bucket: Int): Int = grid[bin * MAX_BUCKETS + bucket]
        val groups: List<Group> by lazy { makeGroups(this) }
    }

    class BadFile(message: String) : Exception(message)

    fun parse(f: ByteArray): Recording {
        fun u8(o: Int) = f[o].toInt() and 255
        fun u16(o: Int) = u8(o) or (u8(o + 1) shl 8)
        if (f.size < HEAD || u8(0) != 77 || u8(1) != 83 || u8(2) != 82 || u8(3) != 49) throw BadFile("This is not a spectrum recording.")
        if (u8(4) != BINS) throw BadFile("The recording is damaged.")
        val buckets = u16(12)
        if (buckets == 0 || buckets > MAX_BUCKETS) throw BadFile("The recording is damaged.")
        var p = HEAD
        val nbm = (buckets + 7) / 8
        if (f.size < p + BINS * 3 + nbm + 2) throw BadFile("The recording is damaged.")
        val floor = IntArray(BINS) { f[p + it].toInt() }
        val max = IntArray(BINS) { f[p + BINS + it].toInt() }
        val duty = IntArray(BINS) { u8(p + 2 * BINS + it) }
        p += BINS * 3
        val masked = BooleanArray(MAX_BUCKETS)
        for (k in 0 until buckets) masked[k] = (u8(p + (k shr 3)) shr (k and 7)) and 1 == 1
        p += nbm
        val slen = u16(p); p += 2
        if (f.size < p + slen + 2) throw BadFile("The recording is damaged.")
        val grid = IntArray(CELLS)
        var b = 0
        var k = 0
        for (i in 0 until slen) {
            val t = u8(p + i)
            val v = if (t and 0x80 != 0) (t shr 3) and 15 else 0
            val run = if (t and 0x80 != 0) (t and 7) + 1 else (t and 127) + 1
            repeat(run) {
                if (b >= BINS) throw BadFile("The recording is damaged.")
                grid[b * MAX_BUCKETS + k] = v
                if (++k == buckets) { k = 0; b++ }
            }
        }
        if (b != BINS || k != 0) throw BadFile("The recording is damaged.")
        p += slen
        val n = u16(p); p += 2
        if (n > MAX_EVENTS || f.size < p + n * 6) throw BadFile("The recording is damaged.")
        val events = (0 until n).map { i ->
            val o = p + i * 6
            val e = Event(u8(o), u8(o + 1), u8(o + 2), u8(o + 3), u8(o + 4))
            if (e.k0 > e.k1 || e.k1 >= buckets || e.b0 > e.b1 || e.b1 >= BINS) throw BadFile("The recording is damaged.")
            e
        }
        return Recording(u8(5), u8(6) * 100, u16(8), u16(10), buckets, u16(14),
            (u8(16).toLong() or (u8(17).toLong() shl 8) or (u8(18).toLong() shl 16) or (u8(19).toLong() shl 24)), u16(20),
            floor, max, duty, masked, grid, events, f.size)
    }

    private fun makeGroups(r: Recording): List<Group> {
        val order = r.events.withIndex().sortedWith(compareBy({ it.value.b0 + it.value.b1 }, { it.value.k0 }, { it.value.b0 }))
        val sums = ArrayList<IntArray>() // sum of 2 x centre, count, width of the first event
        val gid = IntArray(order.size) { -1 }
        order.forEachIndexed { i, ie ->
            val e = ie.value
            val c2 = e.b0 + e.b1
            val w = e.b1 - e.b0 + 1
            var g = sums.indexOfFirst { Math.abs(it[0] - c2 * it[1]) <= 4 * it[1] && Math.abs(it[2] - w) <= 3 }
            if (g < 0) { if (sums.size >= MAX_GROUPS) return@forEachIndexed; sums.add(intArrayOf(0, 0, w)); g = sums.size - 1 }
            sums[g][0] += c2; sums[g][1]++; gid[i] = g
        }
        val total = r.lengthMs
        val bl = r.bucketMs.toLong()
        val out = ArrayList<Group>()
        for (g in sums.indices) {
            val m = order.filterIndexed { i, _ -> gid[i] == g }.sortedWith(compareBy({ it.value.k0 }, { it.value.b0 }))
            val count = m.size
            var air = 0L; var b0 = 255; var b1 = 0; var peak = 0
            for (ie in m) { val e = ie.value; air += (e.k1 - e.k0 + 1) * bl; b0 = minOf(b0, e.b0); b1 = maxOf(b1, e.b1); peak = maxOf(peak, e.peak) }
            val first = m[0].value.k0 * bl
            var sumD = 0L
            for (i in 1 until m.size) sumD += (m[i].value.k0 - m[i - 1].value.k0) * bl
            val nd = (m.size - 1).toLong()
            val mean = if (nd > 0) sumD / nd else 0L
            var v = 0L
            for (i in 1 until m.size) { val d = (m[i].value.k0 - m[i - 1].value.k0) * bl; v += (d - mean) * (d - mean) }
            val regular = nd >= 2 && mean > 0 && v * 16 < mean * mean * nd
            var growUp = false
            if (count == 1) {
                val e = m[0].value
                val len = e.k1 - e.k0 + 1
                if (len >= 6) {
                    fun level(k: Int): Int { var x = 0; for (b in e.b0..e.b1) x = maxOf(x, r.cell(b, k)); return x * STEP_DB }
                    val t = maxOf(1, len / 3)
                    var a = 0; var c = 0
                    for (q in 0 until t) { a += level(e.k0 + q); c += level(e.k1 - q) }
                    growUp = c - a > 3 * t
                }
            }
            val cov = air * 100
            val kind = when {
                cov > 80 * total -> Kind.Constant
                count >= 3 && regular -> Kind.Periodic
                count == 1 && cov > 15 * total -> if (growUp) Kind.Rising else Kind.Long
                count >= 2 -> Kind.Pulses
                else -> Kind.Single
            }
            val severity = if (kind == Kind.Single || kind == Kind.Pulses) {
                if (count >= 3 || peak >= 27) Severity.Medium else Severity.Low
            } else if (peak >= 18 || cov > 50 * total || kind == Kind.Rising) Severity.High else Severity.Medium
            out.add(Group(kind, severity, b0, b1, count, air, first, peak, mean, m.map { it.index }))
        }
        return out.sortedWith(compareBy({ -it.airMs }, { it.firstMs }, { it.b0 }))
    }
}

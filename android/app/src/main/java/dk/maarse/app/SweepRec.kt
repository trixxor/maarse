package dk.maarse.app

/**
 * Spectrum recordings ("MSR2", and "MSR1" for reading): a port of `maarse-core/src/sweeprec.rs`. `SweepRecTest` checks it
 * byte for byte against `maarse-core/tests/data/sweeprec-golden.*`. Change both together. The file layout is in
 * `docs/protocol-spec.md`.
 */
object SweepRec {
    const val BINS = 64
    const val MAX_BUCKETS = 256
    private const val MAX_BUCKETS_V1 = 120
    private const val MAX_EVENTS = 200
    private const val MAX_GROUPS = 48
    private const val HEAD1 = 24
    private const val HEAD2 = 21 + BINS / 2
    private const val CELLS = BINS * MAX_BUCKETS
    const val STEP_DB = 3
    private const val MIN_BUCKET_MS = 1000
    private const val WARM = 16
    private const val Z_CTX = 18
    private const val PROBS = Z_CTX + 16 * 16

    /** What to record. `intervalMs` is the time between the start of two sweeps (only written to the file). */
    class Config(val startKhz: Long, val stepKhz: Int, val durationS: Int, val intervalMs: Int, val thrDb: Int)

    /** A sweep where most steps read stronger than -60 dBm: a transmitter very close overloads the receiver. */
    fun isOverload(r: IntArray): Boolean {
        val valid = r.count { it < 248 }
        val strong = r.count { it < 120 }
        return valid >= 8 && strong * 10 >= valid * 6
    }

    // The context of a cell: the cell before it in time (a), the one before that (a2) and the one for the step below (u).
    private fun zCtx(a: Int, a2: Int, u: Int) = minOf(a, 2) * 3 + minOf(u, 2) + (if (a2 > 0) 9 else 0)
    private fun vCtx(a: Int, u: Int) = if (a > 0) a else u

    /** The range coder of LZMA (11-bit probabilities, shift 5). The first byte (always 0) is left out. */
    private class RcEnc {
        private var low = 0L
        private var range = 0xFFFFFFFFL
        private var cache = 0
        private var cacheSize = 1L
        private var first = true
        private val bytes = ArrayList<Int>()

        private fun byte(b: Int) { if (first) { first = false; return }; bytes.add(b and 255) }

        private fun shiftLow() {
            if ((low and 0xFFFFFFFFL) < 0xFF000000L || (low shr 32) != 0L) {
                val carry = (low shr 32).toInt()
                var temp = cache
                do { byte(temp + carry); temp = 0xFF; cacheSize-- } while (cacheSize != 0L)
                cache = ((low shr 24) and 0xFF).toInt()
            }
            cacheSize++
            low = (low and 0x00FFFFFFL) shl 8
        }

        fun bit(pr: IntArray, i: Int, bit: Boolean) {
            val p = pr[i]
            val bound = (range ushr 11) * p
            if (!bit) { range = bound; pr[i] = p + ((2048 - p) shr 5) } else { low += bound; range -= bound; pr[i] = p - (p shr 5) }
            while (range < (1L shl 24)) { range = range shl 8; shiftLow() }
        }

        /** The bytes, without a tail of zeros: the reader reads zeros past the end. */
        fun finish(): List<Int> {
            repeat(5) { shiftLow() }
            while (bytes.isNotEmpty() && bytes.last() == 0) bytes.removeAt(bytes.size - 1)
            return bytes
        }
    }

    private class RcDec(private val d: ByteArray, private var pos: Int) {
        private var range = 0xFFFFFFFFL
        private var code = 0L

        init { repeat(4) { code = (code shl 8) or next().toLong() } }

        private fun next(): Int = if (pos < d.size) (d[pos++].toInt() and 255) else { pos++; 0 }

        fun bit(pr: IntArray, i: Int): Boolean {
            val p = pr[i]
            val bound = (range ushr 11) * p
            val bit: Boolean
            if (code < bound) { range = bound; pr[i] = p + ((2048 - p) shr 5); bit = false }
            else { code -= bound; range -= bound; pr[i] = p - (p shr 5); bit = true }
            while (range < (1L shl 24)) { range = range shl 8; code = (code shl 8) or next().toLong() }
            return bit
        }
    }

    /** Collects sweeps and makes the file. */
    class Recorder(cfg: Config) {
        val config = Config(cfg.startKhz, cfg.stepKhz, cfg.durationS, cfg.intervalMs.coerceIn(100, 25_500), cfg.thrDb)
        val bucketMs: Int
        val buckets: Int
        private var used = 0
        var sweeps = 0
            private set
        private var curK = 0
        private var curCnt = 0
        private var warmDone = false
        private var peakRaw = 255
        private val cells = IntArray(CELLS)
        private val cur = IntArray(BINS) { 255 }
        private val floor = IntArray(BINS) { 236 }
        private val none = BooleanArray(MAX_BUCKETS)
        private val warm = IntArray(WARM * BINS)

        init {
            require(cfg.durationS in 1..7800 && cfg.stepKhz > 0) { "bad config" }
            val durMs = cfg.durationS * 1000
            bucketMs = maxOf((durMs + MAX_BUCKETS - 1) / MAX_BUCKETS, MIN_BUCKET_MS)
            buckets = minOf((durMs + bucketMs - 1) / bucketMs, MAX_BUCKETS)
        }

        /** Takes one sweep; `tMs` is the time since the recording began. A masked sweep counts as time, not as a measurement. */
        fun push(tMs: Long, rssi: IntArray, masked: Boolean) {
            if (rssi.size != BINS) return
            val k = maxOf(minOf((tMs / bucketMs).toInt(), buckets - 1), curK)
            while (curK < k) { closeBucket(); curK++ }
            used = maxOf(used, k + 1)
            sweeps = minOf(65535, sweeps + 1)
            if (masked) return
            var any = false
            for (b in 0 until BINS) {
                val v = rssi[b]
                if (v >= 248) continue
                any = true
                if (v < cur[b]) cur[b] = v
                if (v < peakRaw) peakRaw = v
            }
            if (any) curCnt = minOf(65535, curCnt + 1)
        }

        /** Seconds recorded so far (whole buckets touched). */
        val elapsedS: Int get() = (used * bucketMs + 999) / 1000

        // The usual peak of each step, with no step stronger than 2 dB over the middle of all steps.
        private fun floors(): IntArray {
            val lim = maxOf(0, floor.sortedArray()[BINS / 2] - 4)
            return IntArray(BINS) { maxOf(floor[it], lim) }
        }

        private fun levels(k: Int, row: IntArray, f: IntArray) {
            val thr2 = 2 * config.thrDb
            for (b in 0 until BINS) {
                val best = row[b]
                if (best == 255) continue
                val d2 = f[b] - best
                cells[b * MAX_BUCKETS + k] = if (d2 >= thr2) ((d2 + STEP_DB) / (STEP_DB * 2)).coerceIn(1, 15) else 0
            }
        }

        private fun finishWarm(n: Int) {
            for (b in 0 until BINS) {
                val tmp = ArrayList<Int>()
                for (k in 0 until n) { val v = warm[k * BINS + b]; if (v != 255) tmp.add(v) }
                if (tmp.isNotEmpty()) { tmp.sort(); floor[b] = tmp[tmp.size shr 1] }
            }
            val f = floors()
            for (k in 0 until n) levels(k, warm.copyOfRange(k * BINS, (k + 1) * BINS), f)
            warmDone = true
        }

        private fun closeBucket() {
            val k = curK
            val row = cur.copyOf()
            if (curCnt == 0) none[k] = true
            if (!warmDone) {
                System.arraycopy(row, 0, warm, k * BINS, BINS)
                if (k == WARM - 1) finishWarm(WARM)
            } else if (curCnt > 0) {
                levels(k, row, floors())
                // The usual peak follows the band slowly: half a dB a bucket towards each value.
                for (b in 0 until BINS) {
                    val best = row[b]
                    val fl = floor[b]
                    if (best != 255) { if (best > fl) floor[b] = fl + 1 else if (best < fl) floor[b] = fl - 1 }
                }
            }
            cur.fill(255)
            curCnt = 0
        }

        private fun end() {
            if (used > 0) { closeBucket(); if (!warmDone) finishWarm(minOf(used, WARM)) } else { used = 1; none[0] = true }
        }

        /** Makes the file. Call it once. */
        fun finish(): ByteArray {
            end()
            val nb = maxOf(used, 1)
            val f = if (warmDone) floors() else IntArray(BINS) { 236 }
            val dbm = IntArray(BINS) { -(f[it] shr 1) }
            val base = dbm.sortedArray()[BINS / 2]
            val anyNone = (0 until nb).any { none[it] }
            val sk = config.startKhz
            val peak = if (peakRaw == 255) base else -(peakRaw shr 1)
            val out = ArrayList<Int>()
            fun u8(v: Int) { out.add(v and 255) }
            u8(77); u8(83); u8(82); u8(50); u8(if (anyNone) 1 else 0); u8(config.thrDb)
            u8(bucketMs); u8(bucketMs shr 8); u8(nb); u8(nb shr 8); u8(sweeps); u8(sweeps shr 8)
            u8(sk.toInt()); u8((sk shr 8).toInt()); u8((sk shr 16).toInt()); u8((sk shr 24).toInt())
            u8(config.stepKhz); u8(config.stepKhz shr 8); u8(peak); u8(config.intervalMs / 100); u8(base)
            fun d(i: Int) = (dbm[i] - base).coerceIn(-8, 7) and 15
            for (b in 0 until BINS / 2) u8(d(2 * b) or (d(2 * b + 1) shl 4))
            if (anyNone) {
                val bm = IntArray((nb + 7) / 8)
                for (k in 0 until nb) if (none[k]) bm[k shr 3] = bm[k shr 3] or (1 shl (k and 7))
                bm.forEach { u8(it) }
            }
            val enc = RcEnc()
            val pr = IntArray(PROBS) { 1024 }
            for (b in 0 until BINS) for (k in 0 until nb) {
                val v = cells[b * MAX_BUCKETS + k]
                val a = if (k > 0) cells[b * MAX_BUCKETS + k - 1] else 0
                val a2 = if (k > 1) cells[b * MAX_BUCKETS + k - 2] else 0
                val u = if (b > 0) cells[(b - 1) * MAX_BUCKETS + k] else 0
                enc.bit(pr, zCtx(a, a2, u), v != 0)
                if (v != 0) {
                    val x = v - 1
                    val vc = vCtx(a, u)
                    var node = 1
                    for (i in 3 downTo 0) { val bit = (x shr i) and 1; enc.bit(pr, Z_CTX + vc * 16 + node, bit == 1); node = node * 2 + bit }
                }
            }
            enc.finish().forEach { u8(it) }
            return ByteArray(out.size) { out[it].toByte() }
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

    fun parse(f: ByteArray): Recording =
        if (f.size >= 4 && f[0].toInt() == 77 && f[1].toInt() == 83 && f[2].toInt() == 82 && f[3].toInt() == 50) parse2(f) else parse1(f)

    private fun parse2(f: ByteArray): Recording {
        fun u8(o: Int) = f[o].toInt() and 255
        fun u16(o: Int) = u8(o) or (u8(o + 1) shl 8)
        if (f.size < HEAD2) throw BadFile("The recording is damaged.")
        val buckets = u16(8)
        val bucketMs = u16(6)
        if (buckets == 0 || buckets > MAX_BUCKETS || bucketMs == 0) throw BadFile("The recording is damaged.")
        val peak = f[18].toInt()
        val base = f[20].toInt()
        val floor = IntArray(BINS) { b -> val nib = (u8(21 + (b shr 1)) shr ((b and 1) * 4)) and 15; (base + ((nib shl 28) shr 28)).coerceIn(-128, 127) }
        var p = HEAD2
        val masked = BooleanArray(MAX_BUCKETS)
        if (u8(4) and 1 == 1) {
            val m = (buckets + 7) / 8
            if (f.size < p + m) throw BadFile("The recording is damaged.")
            for (k in 0 until buckets) masked[k] = (u8(p + (k shr 3)) shr (k and 7)) and 1 == 1
            p += m
        }
        val dec = RcDec(f, p)
        val pr = IntArray(PROBS) { 1024 }
        val g = IntArray(CELLS)
        for (b in 0 until BINS) for (k in 0 until buckets) {
            val a = if (k > 0) g[b * MAX_BUCKETS + k - 1] else 0
            val a2 = if (k > 1) g[b * MAX_BUCKETS + k - 2] else 0
            val u = if (b > 0) g[(b - 1) * MAX_BUCKETS + k] else 0
            if (dec.bit(pr, zCtx(a, a2, u))) {
                val vc = vCtx(a, u)
                var node = 1
                repeat(4) { node = node * 2 + (if (dec.bit(pr, Z_CTX + vc * 16 + node)) 1 else 0) }
                g[b * MAX_BUCKETS + k] = node - 16 + 1
            }
        }
        val measured = maxOf(1, (0 until buckets).count { !masked[it] })
        val duty = IntArray(BINS)
        val max = IntArray(BINS)
        for (b in 0 until BINS) {
            var hot = 0
            var top = 0
            for (k in 0 until buckets) { val v = g[b * MAX_BUCKETS + k]; if (v > 0 && !masked[k]) hot++; top = maxOf(top, v) }
            duty[b] = (hot * 100 + measured / 2) / measured
            max[b] = maxOf(floor[b], minOf(peak, floor[b] + top * STEP_DB))
        }
        val events = findEvents(g, buckets)
        val sk = u8(12).toLong() or (u8(13).toLong() shl 8) or (u8(14).toLong() shl 16) or (u8(15).toLong() shl 24)
        val durationS = minOf(65535L, (buckets.toLong() * bucketMs + 999) / 1000).toInt()
        return Recording(u8(5), u8(19) * 100, durationS, bucketMs, buckets, u16(10), sk, u16(16), floor, max, duty, masked, g, events, f.size)
    }

    // The events: connected areas of cells above the threshold, in the order of their first cell. An area grows pass by pass
    // over its own bounding box.
    private fun findEvents(g: IntArray, nb: Int): List<Event> {
        val events = ArrayList<Event>()
        for (start in 0 until BINS * MAX_BUCKETS) {
            val b0 = start / MAX_BUCKETS
            val k0 = start % MAX_BUCKETS
            if (k0 >= nb || g[start] == 0 || g[start] and 0x80 != 0) continue
            var eb0 = b0; var eb1 = b0; var ek0 = k0; var ek1 = k0
            var peak = g[start] * STEP_DB
            g[start] = g[start] or 0x80
            var reverse = false
            while (true) {
                var changed = false
                val lb = maxOf(0, eb0 - 1); val hb = minOf(BINS - 1, eb1 + 1)
                val lk = maxOf(0, ek0 - 1); val hk = minOf(nb - 1, ek1 + 1)
                for (bi in 0..(hb - lb)) {
                    val b = if (reverse) hb - bi else lb + bi
                    for (ki in 0..(hk - lk)) {
                        val k = if (reverse) hk - ki else lk + ki
                        val i = b * MAX_BUCKETS + k
                        val v = g[i]
                        if (v == 0 || v and 0x80 != 0) continue
                        val touches = (b > 0 && g[i - MAX_BUCKETS] and 0x80 != 0) || (b + 1 < BINS && g[i + MAX_BUCKETS] and 0x80 != 0) ||
                            (k > 0 && g[i - 1] and 0x80 != 0) || (k + 1 < nb && g[i + 1] and 0x80 != 0)
                        if (touches) {
                            g[i] = v or 0x80
                            eb0 = minOf(eb0, b); eb1 = maxOf(eb1, b); ek0 = minOf(ek0, k); ek1 = maxOf(ek1, k)
                            peak = maxOf(peak, v * STEP_DB)
                            changed = true
                        }
                    }
                }
                if (!changed) break
                reverse = !reverse
            }
            if (events.size < MAX_EVENTS) events.add(Event(ek0, ek1, eb0, eb1, peak))
        }
        for (i in g.indices) g[i] = g[i] and 0x7f
        return events
    }

    private fun parse1(f: ByteArray): Recording {
        fun u8(o: Int) = f[o].toInt() and 255
        fun u16(o: Int) = u8(o) or (u8(o + 1) shl 8)
        if (f.size < HEAD1 || u8(0) != 77 || u8(1) != 83 || u8(2) != 82 || u8(3) != 49) throw BadFile("This is not a spectrum recording.")
        if (u8(4) != BINS) throw BadFile("The recording is damaged.")
        val buckets = u16(12)
        if (buckets == 0 || buckets > MAX_BUCKETS_V1) throw BadFile("The recording is damaged.")
        var p = HEAD1
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

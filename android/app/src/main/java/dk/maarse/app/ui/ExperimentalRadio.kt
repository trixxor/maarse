package dk.maarse.app.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Pause
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dk.maarse.app.Cmd
import dk.maarse.app.GpsInfo
import dk.maarse.app.GpsSat
import dk.maarse.app.GPS_STATES
import dk.maarse.app.Node
import kotlinx.coroutines.delay
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.sin

// ---------- GPS ----------

private val USED_COLOUR = Color(0xFF1F6F5C)
private val UNUSED_COLOUR = Color(0xFF8A928D)

private fun accuracyWord(hdopX10: Int) = when {
    hdopX10 <= 0 -> "no value"
    hdopX10 <= 10 -> "excellent"
    hdopX10 <= 20 -> "very good"
    hdopX10 <= 50 -> "good"
    hdopX10 <= 100 -> "fair"
    else -> "poor"
}

/** Experimental > GPS: satellites on a sky view, signal per satellite, height, speed and accuracy. */
@Composable
fun GpsPage() {
    val gps by Node.gps.collectAsState()
    val stats by Node.stats.collectAsState()
    // Ask for details every 2 seconds while the page is open (a radio with GPS answers; the others stay silent).
    LaunchedEffect(Unit) {
        while (true) { Node.send(Cmd.getGps()); Node.send(Cmd.getStats()); delay(2000) }
    }
    val g = gps
    val state = g?.state ?: stats[105]
    Card2(Modifier.padding(top = 4.dp)) {
        if (state == null) {
            Text("No GPS reported", fontWeight = FontWeight.SemiBold)
            Text("Either the connected radio has no GPS module (the RAK WisMesh Tag has one), or it has not answered yet.", style = MaterialTheme.typography.bodyMedium)
        } else {
            Text(GPS_STATES.getOrElse(state) { "$state" }, style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold,
                color = if (state == 3) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurface)
            Text(
                when (state) {
                    0 -> "The GPS is only switched on while you share your position. Turn it on under Name and position."
                    1 -> "The module is powered but sends nothing. If this stays, it is not talking to the radio."
                    2 -> "Searching. It needs at least 4 satellites: stay in the open, a first fix can take a few minutes."
                    else -> (g?.let { "${it.used} of ${it.view} satellites in use" } ?: "")
                },
                style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
    if (g == null || g.sats.isEmpty()) {
        if (state != null && state != 0) Hint("Satellites show up here as soon as the module has heard them.")
        return
    }
    GroupTitle("Sky view")
    Card2 {
        Text("Looking up, north at the top. The middle is straight above you.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        SkyPlot(g.sats)
        Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
            Text("● in use", color = USED_COLOUR, style = MaterialTheme.typography.bodySmall)
            Text("○ seen, not used", color = UNUSED_COLOUR, style = MaterialTheme.typography.bodySmall)
        }
    }
    Row(Modifier.padding(horizontal = 16.dp, vertical = 10.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        GpsTile("Height", g.altDm?.let { "%.0f m".format(it / 10.0) } ?: "–", "above sea level", Modifier.weight(1f))
        GpsTile("Speed", "%.1f km/h".format(g.speedCms * 0.036), if (g.speedCms > 30) "heading ${g.course}°" else "standing still", Modifier.weight(1f))
        GpsTile("Accuracy", if (g.hdopX10 > 0) "± %.0f m".format(g.hdopX10 / 10.0 * 2.5) else "–", accuracyWord(g.hdopX10), Modifier.weight(1f))
    }
    GroupTitle("Signal per satellite")
    Card2 {
        g.sats.forEach { s ->
            Row(Modifier.fillMaxWidth().height(24.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("${s.system} ${s.prn}", Modifier.width(62.dp), style = MaterialTheme.typography.labelLarge, fontWeight = FontWeight.SemiBold)
                Canvas(Modifier.weight(1f).height(10.dp)) {
                    drawRoundRect(Color(0x1F808080), size = size, cornerRadius = androidx.compose.ui.geometry.CornerRadius(5.dp.toPx()))
                    drawRoundRect(if (s.used) USED_COLOUR else Color(0xFFB9C1BC), size = Size(size.width * (s.snr / 50f).coerceIn(0f, 1f), size.height), cornerRadius = androidx.compose.ui.geometry.CornerRadius(5.dp.toPx()))
                }
                Text(if (s.snr > 0) "${s.snr} dB-Hz" else "–", Modifier.width(70.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
    Card2(Modifier.padding(top = 10.dp)) {
        Text("Position", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (g.latE7 != 0 || g.lonE7 != 0) Text("%.5f° %s, %.5f° %s".format(kotlin.math.abs(g.latE7 / 1e7), if (g.latE7 >= 0) "N" else "S", kotlin.math.abs(g.lonE7 / 1e7), if (g.lonE7 >= 0) "E" else "W"), fontWeight = FontWeight.SemiBold)
        else Text("No position yet", fontWeight = FontWeight.SemiBold)
        if (g.utc > 0) Text("Time from the satellites %02d:%02d:%02d UTC".format(g.utc / 10000, g.utc / 100 % 100, g.utc % 100), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
private fun GpsTile(title: String, value: String, sub: String, modifier: Modifier) {
    Column(
        modifier.clip(RoundedCornerShape(16.dp)).androidBg().padding(horizontal = 12.dp, vertical = 10.dp),
    ) {
        Text(title, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(value, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold, maxLines = 1)
        Text(sub, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
    }
}

@Composable
private fun Modifier.androidBg(): Modifier = this.then(Modifier.background(MaterialTheme.colorScheme.surfaceContainer))

@Composable
private fun SkyPlot(sats: List<GpsSat>) {
    val line = MaterialTheme.colorScheme.outlineVariant
    val ink = MaterialTheme.colorScheme.onSurfaceVariant
    val ring = MaterialTheme.colorScheme.outline
    Canvas(Modifier.fillMaxWidth().aspectRatio(1f).padding(8.dp)) {
        val c = Offset(size.width / 2, size.height / 2)
        val r = minOf(size.width, size.height) / 2 - 14.dp.toPx()
        drawCircle(ring, r, c, style = Stroke(1.2.dp.toPx()))
        drawCircle(line, r * 2 / 3, c, style = Stroke(1.dp.toPx()))
        drawCircle(line, r / 3, c, style = Stroke(1.dp.toPx()))
        drawLine(line, Offset(c.x, c.y - r), Offset(c.x, c.y + r), 1.dp.toPx())
        drawLine(line, Offset(c.x - r, c.y), Offset(c.x + r, c.y), 1.dp.toPx())
        val paint = android.graphics.Paint().apply { isAntiAlias = true; textAlign = android.graphics.Paint.Align.CENTER; textSize = 11.sp(); color = ink.toArgb(); isFakeBoldText = true }
        drawContext.canvas.nativeCanvas.apply {
            drawText("N", c.x, c.y - r - 3.dp.toPx(), paint); drawText("S", c.x, c.y + r + 12.dp.toPx(), paint)
            drawText("E", c.x + r + 8.dp.toPx(), c.y + 4.dp.toPx(), paint); drawText("W", c.x - r - 8.dp.toPx(), c.y + 4.dp.toPx(), paint)
        }
        sats.forEach { s ->
            val d = (90 - s.elevation).coerceIn(0, 90) / 90f * r
            val a = s.azimuth * PI.toFloat() / 180f
            val p = Offset(c.x + d * sin(a), c.y - d * cos(a))
            val rad = 10.dp.toPx()
            drawCircle(if (s.used) USED_COLOUR else Color.White, rad, p)
            drawCircle(if (s.used) Color(0xFF0E3A26) else UNUSED_COLOUR, rad, p, style = Stroke(1.5.dp.toPx()))
            val tp = android.graphics.Paint().apply { isAntiAlias = true; textAlign = android.graphics.Paint.Align.CENTER; textSize = 9.sp(); isFakeBoldText = true; color = if (s.used) android.graphics.Color.WHITE else android.graphics.Color.BLACK }
            drawContext.canvas.nativeCanvas.drawText("${s.prn}", p.x, p.y + 3.2.dp.toPx(), tp)
        }
    }
}

private fun Int.sp(): Float = this * android.content.res.Resources.getSystem().displayMetrics.scaledDensity
private fun Color.toArgb(): Int = android.graphics.Color.argb((alpha * 255).toInt(), (red * 255).toInt(), (green * 255).toInt(), (blue * 255).toInt())

// ---------- spectrum ----------

private class Band(val label: String, val startKhz: Long, val stepKhz: Int, val count: Int)
// Only the band the radio is built for (the tag's matching network and antenna are for 863-928 MHz).
private val BANDS = listOf(
    Band("EU 863–870", 863_000, 110, 64),
)

/** dBm of a raw value (-dBm x 2), null when it was not measured (248 and up are failure codes). */
private fun dbm(raw: Int): Float? = if (raw >= 248) null else -raw / 2f

/** Colours from quiet to strong; the steps are dB above the noise floor, so small differences show on any band. */
private val SCALE = listOf(-2f to Color(0xFF14201A), 3f to Color(0xFF1F4D36), 8f to Color(0xFF1F6F5C), 14f to Color(0xFF7FD1A5), 22f to Color(0xFFF2D16B), 32f to Color(0xFFE8833A))

private fun colourOf(d: Float?, floor: Float): Color {
    val x = (d ?: floor) - floor
    if (x <= SCALE.first().first) return SCALE.first().second
    if (x >= SCALE.last().first) return SCALE.last().second
    val i = SCALE.indexOfLast { it.first <= x }
    val (a, ca) = SCALE[i]; val (b, cb) = SCALE[i + 1]
    return androidx.compose.ui.graphics.lerp(ca, cb, (x - a) / (b - a))
}

/**
 * A sweep where most steps read stronger than -60 dBm: a transmitter very close to the radio (for example your other
 * radios on the desk) overloads the receiver, so every frequency looks strong. It is real, but it says nothing about
 * the band, so it is drawn in its own colour and left out of the numbers.
 */
private fun isOverload(r: dk.maarse.app.Ev.Sweep): Boolean {
    val v = r.rssi.toList().mapNotNull { dbm(it) }
    return v.size >= 8 && v.count { it > -60f } >= v.size * 0.6
}

private val OVERLOAD_COLOUR = Color(0xFF8E3B2A)

/** The typical noise of the latest sweeps: the middle value (sweeps that are overloaded do not count). */
private fun noiseFloor(rows: List<dk.maarse.app.Ev.Sweep>): Float {
    val v = rows.filter { !isOverload(it) }.takeLast(10).flatMap { r -> r.rssi.toList().mapNotNull { dbm(it) } }.sorted()
    return if (v.isEmpty()) -105f else v[v.size / 2]
}

/** What the finger points at: a frequency step and, on the waterfall, the sweep (index in the buffer) under it. */
private class Probe(val bin: Int, val row: Long?)

private const val LINES = 40
private val SPEEDS = listOf(0.25, 0.5, 1.0, 2.0)

/**
 * Experimental > Spectrum: the radio measures the signal strength on a row of frequencies (a sweep) and the
 * phone draws the shown sweep as a line and the last seconds as a waterfall that glides down. Drag a finger over
 * either to read a frequency. The last minute is kept, so you can pause, slow down (¼×, ½×) and wind back. While the
 * radio sweeps it cannot receive messages, so Live stops by itself (after 2 minutes, or when you leave the page).
 */
@Composable
fun SpectrumPage() {
    val rows by Node.sweeps.collectAsState()
    val dropped = Node.sweepDropped
    val band = 0
    var live by remember { mutableStateOf(false) }
    var probe by remember { mutableStateOf<Probe?>(null) }
    // Playback: `cursor` is the absolute number of the sweep sliding in at the top (the fraction is the slide).
    var cursor by remember { mutableStateOf(0.0) }
    var paused by remember { mutableStateOf(false) }
    var speedIdx by remember { mutableIntStateOf(2) }
    var following by remember { mutableStateOf(true) }
    var rate by remember { mutableStateOf(8.0) }
    var lastArrival by remember { mutableStateOf(0L) }
    val b = BANDS[band]
    // The sweep belongs to this page: stop it when leaving.
    DisposableEffect(Unit) { onDispose { Node.send(Cmd.sweep(0, 0, 0, 0)) } }
    LaunchedEffect(live, band) {
        if (live) { Node.clearSweeps(); following = true; paused = false; Node.send(Cmd.sweep(b.startKhz, b.stepKhz, b.count, 255)) }
        else Node.send(Cmd.sweep(0, 0, 0, 0))
    }
    // Live ends by itself at the radio after 2 minutes: show it.
    LaunchedEffect(live) { if (live) { delay(121_000); live = false } }
    // The rate of sweeps (rows per second), smoothed, from the arrival times.
    LaunchedEffect(rows.size, dropped) {
        val now = System.currentTimeMillis()
        if (rows.isEmpty()) { lastArrival = 0L; return@LaunchedEffect }
        if (lastArrival != 0L) {
            val inst = 1000.0 / (now - lastArrival).coerceAtLeast(20)
            rate = (rate * 0.85 + inst * 0.15).coerceIn(0.5, 30.0)
        }
        lastArrival = now
    }
    val rowsNow by rememberUpdatedState(rows)
    val droppedNow by rememberUpdatedState(dropped)
    val rateNow by rememberUpdatedState(rate)
    val arrivalNow by rememberUpdatedState(lastArrival)
    // One step per screen frame: the picture glides instead of jumping a line at a time.
    LaunchedEffect(Unit) {
        var last = 0L
        while (true) {
            val t = withFrameNanos { it }
            val dt = if (last == 0L) 0.0 else (t - last) / 1e9
            last = t
            val lo = droppedNow.toDouble(); val hi = lo + rowsNow.size
            if (rowsNow.isEmpty()) { cursor = lo; continue }
            // Where "live" is: the newest sweep slides in over the time one sweep takes.
            val progress = if (arrivalNow == 0L) 0.0 else ((System.currentTimeMillis() - arrivalNow) / 1000.0 * rateNow).coerceIn(0.0, 0.999)
            val liveCursor = hi - 1 + progress
            val speed = SPEEDS[speedIdx]
            if (following && !paused && speed >= 1.0) cursor = maxOf(cursor, liveCursor)
            else {
                if (!paused) cursor += dt * rateNow * speed
                if (cursor >= liveCursor && !paused && speed >= 1.0) following = true
            }
            cursor = cursor.coerceIn(minOf(lo + 1, liveCursor), liveCursor)
        }
    }
    // One quiet row on top: the band and the Live switch.
    Row(Modifier.fillMaxWidth().padding(end = 16.dp), verticalAlignment = Alignment.CenterVertically) {
        Text("${b.label} MHz", Modifier.weight(1f).padding(start = 24.dp), style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
        Button({ live = !live }, Modifier.height(40.dp), contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 20.dp)) { Text(if (live) "Stop" else "Live") }
    }
    val shown = rows.filter { it.startKhz == b.startKhz }
    val top = (cursor.toLong() - dropped).toInt().coerceIn(0, (rows.size - 1).coerceAtLeast(0))
    // The sweep to draw as a line and to put in the numbers: the newest one that is on screen and not overloaded.
    val current = if (rows.isEmpty()) null else (top downTo maxOf(0, top - LINES)).map { rows[it] }.firstOrNull { it.startKhz == b.startKhz && !isOverload(it) }
    val behind = if (rows.isEmpty()) 0.0 else ((dropped + rows.size - 1) - cursor).coerceAtLeast(0.0) / rate
    val pr = probe
    val busy = live || rows.isNotEmpty()

    Card2(Modifier.padding(top = 4.dp)) {
        // The reading: under the finger, else the noise and the strongest signal of the line shown.
        val row = pr?.row?.let { r -> rows.getOrNull((r - dropped).toInt()) } ?: current
        val d = pr?.let { row?.let { r -> dbm(r.rssi.getOrElse(pr.bin) { 255 }) } }
        Row(Modifier.fillMaxWidth().height(44.dp), verticalAlignment = Alignment.CenterVertically) {
            if (pr != null) {
                Text("%.2f MHz".format((b.startKhz + b.stepKhz.toLong() * pr.bin) / 1000.0), style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
                Text(d?.let { "  %.0f dBm".format(it) } ?: "  –", style = MaterialTheme.typography.titleMedium, color = MaterialTheme.colorScheme.primary)
                val ago = pr.row?.let { ((dropped + rows.size - 1) - it) / rate }
                Text(
                    ago?.let { if (it < 0.5) "  now" else "  %.1f s ago".format(it) } ?: "",
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.weight(1f),
                )
            } else if (current != null) {
                val vals = current.rssi.map { dbm(it) }
                val loud = vals.withIndex().filter { it.value != null }.maxByOrNull { it.value!! }
                val noise = vals.filterNotNull().sorted().let { if (it.isEmpty()) null else it[it.size / 2] }
                Text(
                    listOfNotNull(noise?.let { "noise %.0f dBm".format(it) }, loud?.let { "strongest %.1f MHz".format((b.startKhz + b.stepKhz * it.index) / 1000.0) }).joinToString("  ·  "),
                    style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.weight(1f),
                )
            } else {
                Text("Press Live to start", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.weight(1f))
            }
            if (busy) Text("%.0f/s".format(rate), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        if (pr != null && row != null && isOverload(row)) Text("A transmitter close by overloaded the radio in this line.", style = MaterialTheme.typography.labelSmall, color = Color(0xFFE08A78))
        if (current != null) SpectrumLine(shown.filter { !isOverload(it) }, current, b, pr?.bin) { probe = it?.let { bin -> Probe(bin, null) } ?: probe }
        Waterfall(rows, dropped, cursor, b, pr) { probe = it ?: probe }
        Row(Modifier.fillMaxWidth().padding(top = 2.dp), horizontalArrangement = Arrangement.SpaceBetween) {
            (0..4).forEach { Text("%.1f".format((b.startKhz + b.stepKhz * b.count * it / 4) / 1000.0), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
        }
        // Playback in one thin row: pause, wind back, speed.
        if (rows.size > 2) Row(Modifier.fillMaxWidth().height(44.dp), verticalAlignment = Alignment.CenterVertically) {
            androidx.compose.material3.IconButton({ paused = !paused; if (paused) following = false }, Modifier.size(40.dp)) {
                androidx.compose.material3.Icon(if (paused) androidx.compose.material.icons.Icons.Filled.PlayArrow else androidx.compose.material.icons.Icons.Filled.Pause, if (paused) "Play" else "Pause")
            }
            val lo = dropped.toDouble() + 1; val hi = dropped + rows.size - 1.0
            androidx.compose.material3.Slider(
                value = if (hi > lo) ((cursor - lo) / (hi - lo)).toFloat().coerceIn(0f, 1f) else 1f,
                onValueChange = { v -> following = false; paused = true; cursor = lo + v * (hi - lo) },
                enabled = hi > lo, modifier = Modifier.weight(1f),
            )
            // One tap goes quarter, half, 1x, 2x; the last button jumps back to live.
            androidx.compose.material3.TextButton({ speedIdx = (speedIdx + 1) % SPEEDS.size; if (SPEEDS[speedIdx] < 1.0) following = false }, contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 8.dp)) {
                Text(listOf("¼×", "½×", "1×", "2×")[speedIdx])
            }
            if (behind >= 0.3 || paused) androidx.compose.material3.TextButton({ following = true; paused = false; speedIdx = 2 }, contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 8.dp)) {
                Text("%.0f s · live".format(behind))
            }
        }
        if (rows.takeLast(LINES + 1).any { isOverload(it) }) Text("Dark red: a transmitter very close overloads the radio. Left out of the numbers.", style = MaterialTheme.typography.labelSmall, color = Color(0xFFE08A78))
    }
    Text(
        "The radio hears no messages while it sweeps. Live stops after 2 minutes, or when you leave. Drag over the picture to read a frequency.",
        Modifier.padding(horizontal = 24.dp, vertical = 8.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
}

/** A finger on a chart: the step under it (0 until `count - 1`). */
private fun binAt(x: Float, width: Float, count: Int) = ((x / width) * count).toInt().coerceIn(0, count - 1)

/** The shown sweep as a line with the highest value seen as a dashed line; a vertical mark where the finger is. */
@Composable
private fun SpectrumLine(rows: List<dk.maarse.app.Ev.Sweep>, shownRow: dk.maarse.app.Ev.Sweep, b: Band, probeBin: Int?, onProbe: (Int?) -> Unit) {
    val grid = MaterialTheme.colorScheme.outlineVariant
    val line = MaterialTheme.colorScheme.primary
    val ink = MaterialTheme.colorScheme.onSurfaceVariant
    val n = b.count
    // Smoothed over the shown sweep and the two before it, so the line does not flicker.
    val upto = rows.indexOf(shownRow).let { if (it < 0) rows.lastIndex else it }
    val near = rows.subList(maxOf(0, upto - 2), upto + 1)
    val smooth = FloatArray(n) { i -> near.mapNotNull { dbm(it.rssi.getOrElse(i) { 255 }) }.let { if (it.isEmpty()) -120f else it.average().toFloat() } }
    val peak = FloatArray(n) { i -> rows.takeLast(60).mapNotNull { dbm(it.rssi.getOrElse(i) { 255 }) }.maxOrNull() ?: -120f }
    Canvas(
        Modifier.fillMaxWidth().height(110.dp).padding(top = 4.dp).pointerInput(n) {
            // Horizontal drags are for reading; a vertical one still scrolls the page.
            detectHorizontalDragGestures { change, _ -> onProbe(binAt(change.position.x, size.width.toFloat(), n)); change.consume() }
        }.pointerInput(n) { detectTapGestures { onProbe(binAt(it.x, size.width.toFloat(), n)) } },
    ) {
        fun y(d: Float) = size.height - ((d.coerceIn(-120f, -60f) + 120f) / 60f) * size.height
        fun x(i: Int) = i / (n - 1f) * size.width
        listOf(-60f, -80f, -100f, -120f).forEach { drawLine(grid, Offset(0f, y(it)), Offset(size.width, y(it)), 1.dp.toPx()) }
        val p = Path().apply { smooth.forEachIndexed { i, d -> if (i == 0) moveTo(x(i), y(d)) else lineTo(x(i), y(d)) } }
        val area = Path().apply { addPath(p); lineTo(size.width, size.height); lineTo(0f, size.height); close() }
        drawPath(area, line.copy(alpha = 0.18f))
        val pk = Path().apply { peak.forEachIndexed { i, d -> if (i == 0) moveTo(x(i), y(d)) else lineTo(x(i), y(d)) } }
        drawPath(pk, Color(0xFF7FD1A5), style = Stroke(1.2.dp.toPx(), pathEffect = androidx.compose.ui.graphics.PathEffect.dashPathEffect(floatArrayOf(8f, 8f))))
        drawPath(p, line, style = Stroke(2.dp.toPx(), join = androidx.compose.ui.graphics.StrokeJoin.Round))
        // The Må:rse channel, when it is in this band.
        val marse = 869_525L
        if (marse in b.startKhz..(b.startKhz + b.stepKhz.toLong() * n)) {
            val mx = (marse - b.startKhz).toFloat() / (b.stepKhz * n) * size.width
            drawLine(line, Offset(mx, 0f), Offset(mx, size.height), 1.dp.toPx(), pathEffect = androidx.compose.ui.graphics.PathEffect.dashPathEffect(floatArrayOf(4f, 6f)))
        }
        probeBin?.let { bin ->
            val px = (bin + 0.5f) / n * size.width
            drawLine(Color.White, Offset(px, 0f), Offset(px, size.height), 1.5.dp.toPx())
            drawCircle(Color.White, 4.dp.toPx(), Offset(px, y(smooth[bin])))
        }
    }
}

/**
 * The waterfall: one line per sweep, the sweep at `cursor` sliding in at the top. The picture glides down with the
 * fraction of `cursor`. Drag a finger over it to read a frequency and a time.
 */
@Composable
private fun Waterfall(rows: List<dk.maarse.app.Ev.Sweep>, dropped: Long, cursor: Double, b: Band, probe: Probe?, onProbe: (Probe?) -> Unit) {
    val n = b.count
    val floor = noiseFloor(rows.takeLast(LINES * 2))
    val c = cursor
    val frac = (c - kotlin.math.floor(c)).toFloat()
    val firstIndex = kotlin.math.floor(c).toLong()
    // The cursor moves every frame: the touch handlers read it through state, so they are not restarted (which would drop a drag).
    val fracNow by rememberUpdatedState(frac)
    val firstNow by rememberUpdatedState(firstIndex)
    Canvas(
        Modifier.fillMaxWidth().height(300.dp).padding(top = 6.dp).clip(RoundedCornerShape(8.dp)).pointerInput(n) {
            val rowH = size.height / LINES
            // y of row (first - k) is (frac - 1 + k) x rowH
            fun at(p: Offset): Probe {
                val k = kotlin.math.floor(p.y / rowH - fracNow + 1).toLong().coerceIn(0, LINES.toLong())
                return Probe(binAt(p.x, size.width.toFloat(), n), firstNow - k)
            }
            detectHorizontalDragGestures { change, _ -> onProbe(at(change.position)); change.consume() }
        }.pointerInput(n) {
            val rowH = size.height / LINES
            detectTapGestures {
                val k = kotlin.math.floor(it.y / rowH - fracNow + 1).toLong().coerceIn(0, LINES.toLong())
                onProbe(Probe(binAt(it.x, size.width.toFloat(), n), firstNow - k))
            }
        },
    ) {
        drawRect(Color(0xFF101512), size = size)
        val rowH = size.height / LINES
        for (k in 0..LINES + 1) {
            val idx = (firstIndex - k - dropped).toInt()
            if (idx < 0 || idx >= rows.size) continue
            val row = rows[idx]
            val top = (frac - 1 + k) * rowH
            if (isOverload(row)) {
                drawRect(OVERLOAD_COLOUR, Offset(0f, top), Size(size.width, rowH + 0.8f))
            } else {
                // A smooth blend between neighbouring steps (like a real waterfall), not blocks.
                val stops = Array(n) { i -> ((i + 0.5f) / n) to colourOf(dbm(row.rssi.getOrElse(i) { 255 }), floor) }
                drawRect(androidx.compose.ui.graphics.Brush.horizontalGradient(colorStops = stops, startX = 0f, endX = size.width), Offset(0f, top), Size(size.width, rowH + 0.8f))
            }
        }
        probe?.let { p ->
            val px = (p.bin + 0.5f) / n * size.width
            drawLine(Color.White, Offset(px, 0f), Offset(px, size.height), 1.5.dp.toPx())
            p.row?.let { r ->
                val k = (firstIndex - r).toFloat()
                val py = (frac - 1 + k + 0.5f) * rowH
                if (py in 0f..size.height) drawLine(Color.White.copy(alpha = 0.6f), Offset(0f, py), Offset(size.width, py), 1.dp.toPx())
            }
        }
    }
}

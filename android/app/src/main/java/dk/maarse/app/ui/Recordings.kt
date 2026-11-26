package dk.maarse.app.ui

import android.graphics.Bitmap
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.background
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Slider
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableDoubleStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import dk.maarse.app.Cmd
import dk.maarse.app.Node
import dk.maarse.app.RadioRec
import dk.maarse.app.SweepRec
import dk.maarse.app.SweepRecording
import dk.maarse.app.SweepStore
import java.text.DateFormat
import java.util.Date

// ---------- record bar (on the Spectrum page) ----------

private val DURATIONS = listOf(1, 2, 5, 10)

/** Record 1, 2, 5 or 10 minutes of sweeps into a small file. While it runs the radio hears no messages. */
@Composable
fun RecordBar(go: (MePage) -> Unit) {
    val active by SweepRecording.active.collectAsState()
    val elapsed by SweepRecording.elapsedMs.collectAsState()
    val message by SweepRecording.message.collectAsState()
    val saved by SweepRecording.saved.collectAsState()
    var minutes by remember { mutableIntStateOf(5) }
    Card2(Modifier.padding(top = 4.dp)) {
        val a = active
        if (a == null) {
            Text("Record a scan", style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.SemiBold)
            Row(Modifier.fillMaxWidth().padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                DURATIONS.forEach { m -> FilterChip(selected = minutes == m, onClick = { minutes = m }, label = { Text("$m min") }) }
            }
            Button({ SweepRecording.start(minutes * 60) }, Modifier.padding(top = 8.dp)) { Text("Record") }
            Text(
                "The result is a file of about 1 KB with a summary of what stands out. The radio hears no messages while it records.",
                Modifier.padding(top = 6.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        } else {
            Text("Recording ${clock(elapsed / 1000.0)} of ${clock(a.durationS.toDouble())}", style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.SemiBold)
            LinearProgressIndicator(progress = { (elapsed / (a.durationS * 1000f)).coerceIn(0f, 1f) }, Modifier.fillMaxWidth().padding(vertical = 8.dp))
            Text(
                "The radio hears no messages meanwhile." + if (a.masked > 0) " ${a.masked} overloaded sweeps are left out." else "",
                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button({ SweepRecording.finish() }) { Text("Stop and save") }
                OutlinedButton({ SweepRecording.cancel() }) { Text("Cancel") }
            }
        }
        if (message.isNotEmpty() && a == null) {
            Text(message, Modifier.padding(top = 8.dp), style = MaterialTheme.typography.bodyMedium)
            if (saved != null) TextButton({ go(MePage.X_RECORDINGS) }) { Text("Open Recordings") }
        }
    }
}

// ---------- the Recordings page ----------

private fun clock(s: Double): String { val n = Math.round(s).toInt(); return "%02d:%02d".format(n / 60, n % 60) }
private fun mhz(khz: Long) = "%.2f".format(khz / 1000.0)
private fun size(n: Int) = if (n >= 1024) "%.1f KB".format(n / 1024.0) else "$n B"

/** Record on a radio itself: the one connected to this phone, or a repeater over LoRa. It sends the finished file back. */
@Composable
fun RadioRecordCard() {
    val target by RadioRec.target.collectAsState()
    val st by RadioRec.state.collectAsState()
    val message by RadioRec.message.collectAsState()
    val nodes by Node.nodes.collectAsState()
    var minutes by remember { mutableIntStateOf(5) }
    var picking by remember { mutableStateOf(false) }
    val running = st?.state in 1..3
    val names = listOf("Recording", "", "Done, about to send", "Sending the file over LoRa", "Failed")
    Card2(Modifier.padding(top = 4.dp)) {
        Text("Record on a radio", style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.SemiBold)
        Text(
            "The radio records on its own, 10 sweeps a second, and sends a small file back when it is done (a few hundred bytes for 5 minutes). Over LoRa that takes a minute or two. While it records it hears and sends nothing, so a repeater is out of the network for up to 10 minutes. A repeater only obeys administrators.",
            Modifier.padding(top = 4.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        OutlinedButton({ picking = !picking }, Modifier.padding(top = 8.dp), enabled = !running) { Text(if (target.isEmpty()) "This phone's radio" else Node.label(target)) }
        if (picking) {
            TextButton({ RadioRec.target.value = ""; picking = false }) { Text("This phone's radio") }
            nodes.keys.sortedBy { Node.label(it) }.forEach { a -> TextButton({ RadioRec.target.value = a; picking = false }) { Text(Node.label(a)) } }
        }
        Row(Modifier.fillMaxWidth().padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            DURATIONS.forEach { m -> FilterChip(selected = minutes == m, onClick = { minutes = m }, enabled = !running, label = { Text("$m min") }) }
        }
        Row(Modifier.padding(top = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Button({ RadioRec.start(minutes * 60) }, enabled = st?.state != 1) { Text("Start recording") }
            if (running) OutlinedButton({ RadioRec.stop() }) { Text("Stop") }
        }
        st?.let { s ->
            LinearProgressIndicator(progress = { if (s.state == 1 && s.durationS > 0) (s.elapsedS / s.durationS.toFloat()).coerceIn(0f, 1f) else 1f }, Modifier.fillMaxWidth().padding(top = 8.dp))
            val extra = (if (s.state == 1) " · ${clock(s.elapsedS.toDouble())} of ${clock(s.durationS.toDouble())} · ${s.sweeps} sweeps" else "") + (if (s.bytes > 0) " · file ${size(s.bytes)}" else "")
            Text((names.getOrNull(s.state)?.ifEmpty { "Idle" } ?: "Idle") + extra, Modifier.padding(top = 4.dp), style = MaterialTheme.typography.bodySmall)
        }
        if (message.isNotEmpty()) Text(message, Modifier.padding(top = 6.dp), style = MaterialTheme.typography.bodyMedium)
    }
}

/** Experimental > Recordings: the saved scans, with a player for the open one. */
@Composable
fun RecordingsPage() {
    val items by SweepStore.items.collectAsState()
    val open by SweepStore.open.collectAsState()
    val ctx = LocalContext.current
    var error by remember { mutableStateOf("") }
    var exporting by remember { mutableStateOf<SweepStore.Item?>(null) }
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        runCatching {
            val bytes = ctx.contentResolver.openInputStream(uri)!!.use { it.readBytes() }
            SweepRec.parse(bytes)
            val name = uri.lastPathSegment?.substringAfterLast('/')?.substringBeforeLast('.')?.ifBlank { null } ?: "Imported scan"
            SweepStore.open.value = SweepStore.save(name, bytes).id
            error = ""
        }.onFailure { error = (it as? SweepRec.BadFile)?.message ?: "Could not read the file." }
    }
    val export = rememberLauncherForActivityResult(ActivityResultContracts.CreateDocument("application/octet-stream")) { uri ->
        val it = exporting; exporting = null
        if (uri != null && it != null) runCatching { ctx.contentResolver.openOutputStream(uri)!!.use { o -> o.write(SweepStore.read(it)) } }
            .onFailure { error = "Could not save the file." }
    }
    RadioRecordCard()
    Card2(Modifier.padding(top = 4.dp)) {
        Text("Scans you recorded on the Spectrum page, or opened from a file. Each one is a small file you can keep or send to someone else.", style = MaterialTheme.typography.bodyMedium)
        OutlinedButton({ pick.launch(arrayOf("*/*")) }, Modifier.padding(top = 8.dp)) { Text("Open a file…") }
        if (error.isNotEmpty()) Text(error, Modifier.padding(top = 6.dp), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
    }
    if (items.isEmpty()) {
        Hint("No recordings yet. Open Spectrum and press Record.")
        return
    }
    GroupTitle("Saved scans")
    Group {
        items.forEachIndexed { i, it ->
            val parsed = remember(it.id, it.at) { runCatching { SweepRec.parse(SweepStore.read(it)) }.getOrNull() }
            val sub = parsed?.let { r -> "${clock(r.lengthMs / 1000.0)} · ${size(r.size)} · ${r.groups.count { g -> g.severity != SweepRec.Severity.Low }} things stand out" } ?: "Damaged file"
            GroupRow(it.name, DateFormat.getDateTimeInstance(DateFormat.SHORT, DateFormat.SHORT).format(Date(it.at)) + " · " + sub, last = i == items.lastIndex) { SweepStore.open.value = it.id }
        }
    }
    val cur = items.firstOrNull { it.id == open }
    if (cur != null) {
        val rec = remember(cur.id, cur.at) { runCatching { SweepRec.parse(SweepStore.read(cur)) } }
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedButton({ exporting = cur; export.launch(cur.name + ".msr") }) { Text("Save file") }
            OutlinedButton({ SweepStore.delete(cur); SweepStore.open.value = null }) { Text("Delete") }
        }
        rec.onSuccess { RecordingPlayer(it, cur.name) }.onFailure { Hint((it as? SweepRec.BadFile)?.message ?: "Could not read the recording.") }
    }
}

// ---------- the player: reads only the file ----------

private val STOPS = listOf(0f to intArrayOf(10, 18, 32), .25f to intArrayOf(27, 74, 107), .5f to intArrayOf(47, 163, 154), .75f to intArrayOf(232, 197, 71), 1f to intArrayOf(255, 90, 54))
private val LUT: IntArray = IntArray(16) { v ->
    val t = v / 15f
    val j = (1 until STOPS.size).first { t <= STOPS[it].first }
    val (a, ca) = STOPS[j - 1]; val (b, cb) = STOPS[j]
    val k = (t - a) / (b - a)
    val c = IntArray(3) { (ca[it] + (cb[it] - ca[it]) * k).toInt() }
    0xFF shl 24 or (c[0] shl 16) or (c[1] shl 8) or c[2]
}
private val GROUND = Color(0xFF0A1220)
private val SPEEDS = listOf(1, 5, 10, 30, 60)
private const val MARSE_KHZ = 869_525L

private fun freq(r: SweepRec.Recording, b0: Int, b1: Int) = if (b0 == b1) "${mhz(r.khz(b0))} MHz" else "${mhz(r.khz(b0))}–${mhz(r.khz(b1 + 1))} MHz"

private fun label(g: SweepRec.Group) = when (g.kind) {
    SweepRec.Kind.Constant -> "Constant carrier"
    SweepRec.Kind.Periodic -> "Periodic burst, every ${Math.round(g.intervalMs / 1000.0)} s"
    SweepRec.Kind.Rising -> "Noise that rises"
    SweepRec.Kind.Long -> "Long event"
    SweepRec.Kind.Pulses -> "Repeated short pulses"
    SweepRec.Kind.Single -> "Single pulse"
}

private fun describe(r: SweepRec.Recording, g: SweepRec.Group): String {
    val air = clock(g.airMs / 1000.0); val first = clock(g.firstMs / 1000.0)
    return when (g.kind) {
        SweepRec.Kind.Constant -> "constant carrier, +${g.peakDb} dB over the floor, on ${Math.round(g.airMs * 100.0 / r.lengthMs)} % of the time. Looks like a fixed transmitter or interference close by."
        SweepRec.Kind.Periodic -> "periodic burst every ${Math.round(g.intervalMs / 1000.0)} s, ${g.count} times, about ${Math.round(g.airMs / g.count / 1000.0)} s each and up to +${g.peakDb} dB. The regularity points at a timer or controlled equipment."
        SweepRec.Kind.Rising -> "noise that rises from $first and lasts $air, up to +${g.peakDb} dB. Fits a new noise source that was switched on, or equipment warming up."
        SweepRec.Kind.Long -> "long event from $first, lasts $air, up to +${g.peakDb} dB."
        SweepRec.Kind.Pulses -> "${g.count} short pulses, $air on the air in total, up to +${g.peakDb} dB. Scattered, no fixed pattern."
        SweepRec.Kind.Single -> "one short pulse at $first, +${g.peakDb} dB."
    }
}

@Composable
private fun RecordingPlayer(r: SweepRec.Recording, name: String) {
    val total = r.lengthMs / 1000.0
    val nb = r.buckets
    var t by remember(r) { mutableDoubleStateOf(0.0) }
    var playing by remember(r) { mutableStateOf(false) }
    var speedIdx by remember(r) { mutableIntStateOf(2) }
    var sel by remember(r) { mutableIntStateOf(-1) }
    val groups = r.groups
    val evGroup = remember(r) { HashMap<Int, Int>().also { m -> groups.forEachIndexed { gi, g -> g.members.forEach { m[it] = gi } } } }
    val bitmap = remember(r) {
        val bmp = Bitmap.createBitmap(nb, SweepRec.BINS, Bitmap.Config.ARGB_8888)
        val px = IntArray(nb * SweepRec.BINS)
        for (b in 0 until SweepRec.BINS) for (k in 0 until nb) px[(SweepRec.BINS - 1 - b) * nb + k] = LUT[r.cell(b, k)]
        bmp.setPixels(px, 0, nb, 0, 0, nb, SweepRec.BINS)
        bmp.asImageBitmap()
    }
    LaunchedEffect(playing, speedIdx, r) {
        if (!playing) return@LaunchedEffect
        var last = withFrameNanos { it }
        while (true) {
            val n = withFrameNanos { it }
            t += (n - last) / 1e9 * SPEEDS[speedIdx]
            last = n
            if (t >= total) { t = total - 0.001; playing = false; break }
        }
    }
    val k = (t * 1000 / r.bucketMs).toInt().coerceIn(0, nb - 1)
    val ink = MaterialTheme.colorScheme.onSurfaceVariant

    GroupTitle(name)
    Card2 {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton({ if (!playing && t >= total - 0.05) t = 0.0; playing = !playing }, Modifier.size(40.dp)) {
                androidx.compose.material3.Icon(if (playing) Icons.Filled.Pause else Icons.Filled.PlayArrow, if (playing) "Pause" else "Play")
            }
            Slider(value = (t / total).toFloat().coerceIn(0f, 1f), onValueChange = { t = it * total }, Modifier.weight(1f))
            TextButton({ speedIdx = (speedIdx + 1) % SPEEDS.size }, contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 8.dp)) { Text("${SPEEDS[speedIdx]}×") }
        }
        Text("${clock(t)} / ${clock(total)}", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
        // The band over time: time runs left to right, frequency upward.
        Canvas(
            Modifier.fillMaxWidth().height(190.dp).padding(top = 6.dp).clip(RoundedCornerShape(6.dp))
                .pointerInput(total) { detectTapGestures { t = (it.x / size.width * total).coerceIn(0.0, total - 0.001) } }
                .pointerInput(total) { detectHorizontalDragGestures { c, _ -> t = (c.position.x / size.width * total).coerceIn(0.0, total - 0.001); c.consume() } },
        ) {
            drawRect(GROUND, size = size)
            drawImage(bitmap, srcOffset = IntOffset.Zero, srcSize = IntSize(nb, SweepRec.BINS), dstOffset = IntOffset.Zero,
                dstSize = IntSize(size.width.toInt(), size.height.toInt()), filterQuality = FilterQuality.None)
            val cw = size.width / nb; val rh = size.height / SweepRec.BINS
            for (q in 0 until nb) if (r.masked[q]) drawRect(Color(0xFF7B8798), Offset(q * cw, size.height - 3.dp.toPx()), Size(cw + 0.5f, 3.dp.toPx()))
            r.events.forEachIndexed { i, e ->
                val on = evGroup[i] == sel
                drawRect(if (on) Color(0xFFFFD23F) else Color.White.copy(alpha = 0.4f), Offset(e.k0 * cw, (SweepRec.BINS - 1 - e.b1) * rh),
                    Size((e.k1 - e.k0 + 1) * cw, (e.b1 - e.b0 + 1) * rh), style = Stroke(if (on) 2.dp.toPx() else 1.dp.toPx()))
            }
            val cx = (t / total).toFloat() * size.width
            drawLine(Color.White, Offset(cx, 0f), Offset(cx, size.height), 2.dp.toPx())
        }
        // Activity: how many frequencies are over the threshold at each moment.
        Canvas(Modifier.fillMaxWidth().height(36.dp).padding(top = 4.dp).clip(RoundedCornerShape(4.dp))) {
            drawRect(GROUND, size = size)
            val hot = IntArray(nb) { q -> (0 until SweepRec.BINS).count { b -> r.cell(b, q) > 0 } }
            val mx = maxOf(8, hot.max()).toFloat(); val cw = size.width / nb
            for (q in 0 until nb) {
                if (r.masked[q]) { drawRect(Color(0xFF5B6779), Offset(q * cw, size.height - 3.dp.toPx()), Size(cw, 3.dp.toPx())); continue }
                val h = hot[q] / mx * size.height
                drawRect(if (q <= k) Color(0xFF5CC0A0) else Color(0xFF2A6F6A), Offset(q * cw, size.height - h), Size(maxOf(1f, cw - 0.5f), h))
            }
            val cx = (t / total).toFloat() * size.width
            drawLine(Color.White, Offset(cx, 0f), Offset(cx, size.height), 2.dp.toPx())
        }
        Row(Modifier.fillMaxWidth().padding(top = 2.dp), horizontalArrangement = Arrangement.SpaceBetween) {
            listOf(0.0, .25, .5, .75, 1.0).forEach { Text(clock(total * it), style = MaterialTheme.typography.labelSmall, color = ink) }
        }
    }

    GroupTitle("Now")
    Card2 {
        // The spectrum at this moment, rebuilt from the file: the floor plus the level stored for the bucket.
        val peak = IntArray(SweepRec.BINS) { b -> (0..k).maxOf { q -> r.cell(b, q) } }
        val line = MaterialTheme.colorScheme.outlineVariant
        Canvas(Modifier.fillMaxWidth().height(120.dp)) {
            fun y(d: Float) = size.height - ((d.coerceIn(-120f, -60f) + 120f) / 60f) * size.height
            val pw = size.width / SweepRec.BINS
            listOf(-60f, -80f, -100f, -120f).forEach { drawLine(line, Offset(0f, y(it)), Offset(size.width, y(it)), 1.dp.toPx()) }
            val mx = (MARSE_KHZ - r.startKhz).toFloat() / r.stepKhz
            if (mx in 0f..SweepRec.BINS.toFloat()) drawLine(Color(0xFF5CC0A0), Offset((mx + .5f) * pw, 0f), Offset((mx + .5f) * pw, size.height), 1.dp.toPx(), pathEffect = PathEffect.dashPathEffect(floatArrayOf(4f, 6f)))
            for (b in 0 until SweepRec.BINS) {
                val fl = r.floor[b].toFloat(); val v = r.cell(b, k)
                if (v > 0) drawRect(Color(0xFFE8833A), Offset(b * pw + 1f, y(fl + v * 3)), Size(pw - 2f, y(fl) - y(fl + v * 3)))
                drawLine(ink, Offset(b * pw + 1f, y(fl)), Offset((b + 1) * pw - 1f, y(fl)), 1.5.dp.toPx())
                if (peak[b] > 0) drawLine(ink.copy(alpha = 0.7f), Offset(b * pw + 1f, y(fl + peak[b] * 3)), Offset((b + 1) * pw - 1f, y(fl + peak[b] * 3)), 1.dp.toPx(), pathEffect = PathEffect.dashPathEffect(floatArrayOf(6f, 4f)))
            }
        }
        Row(Modifier.fillMaxWidth().padding(top = 2.dp), horizontalArrangement = Arrangement.SpaceBetween) {
            (0..4).forEach { Text("%.1f".format(r.khz(it * 16) / 1000.0), style = MaterialTheme.typography.labelSmall, color = ink) }
        }
        val active = HashMap<Int, Pair<SweepRec.Event, Int>>()
        r.events.forEachIndexed { i, e ->
            if (k < e.k0 || k > e.k1) return@forEachIndexed
            val lv = (e.b0..e.b1).maxOf { b -> r.cell(b, k) } * SweepRec.STEP_DB
            val gi = evGroup[i] ?: -1
            if (active[gi]?.second?.let { it < lv } != false) active[gi] = e to lv
        }
        if (r.masked[k]) Text("Not measured in this moment.", Modifier.padding(top = 6.dp), color = ink, style = MaterialTheme.typography.bodySmall)
        active.forEach { (gi, v) ->
            val (e, lv) = v
            Column(Modifier.padding(top = 6.dp)) {
                Text(groups.getOrNull(gi)?.let(::label) ?: "Event", fontWeight = FontWeight.SemiBold)
                Text("bin ${e.b0}${if (e.b1 != e.b0) "–${e.b1}" else ""} · ${freq(r, e.b0, e.b1)} · +$lv dB now", style = MaterialTheme.typography.bodySmall, color = ink)
            }
        }
        if (active.isEmpty() && !r.masked[k]) Text("Nothing over the threshold. Only the noise floor.", Modifier.padding(top = 6.dp), color = ink, style = MaterialTheme.typography.bodySmall)
        Text(
            "Rebuilt from the file. Frequencies under the threshold show as the floor, so the real level can be up to the threshold higher. The dashed line is the highest level seen so far.",
            Modifier.padding(top = 8.dp), style = MaterialTheme.typography.labelSmall, color = ink,
        )
    }

    GroupTitle("What stands out")
    Group {
        groups.take(12).forEachIndexed { gi, g ->
            val colour = when (g.severity) { SweepRec.Severity.High -> Color(0xFFE8833A); SweepRec.Severity.Medium -> Color(0xFFB8730A); else -> ink }
            GroupRow(label(g), "${freq(r, g.b0, g.b1)} · ${g.count}× · ${clock(g.airMs / 1000.0)} on the air · first at ${clock(g.firstMs / 1000.0)} · +${g.peakDb} dB",
                last = gi == minOf(groups.size, 12) - 1, leading = { Column(Modifier.size(10.dp).background(colour, CircleShape)) {} }) {
                sel = gi; t = maxOf(0.0, g.firstMs / 1000.0 - r.bucketMs / 1000.0)
            }
        }
    }
    if (groups.size > 12) Text("… and ${groups.size - 12} smaller groups", Modifier.padding(horizontal = 24.dp, vertical = 4.dp), color = ink, style = MaterialTheme.typography.bodySmall)
    if (groups.isEmpty()) Hint("Nothing stands out over the threshold.")
    val big = groups.filter { it.severity != SweepRec.Severity.Low }.take(6).ifEmpty { groups.take(3) }
    if (big.isNotEmpty()) Card2(Modifier.padding(top = 8.dp)) {
        big.forEach { g ->
            Text("bin ${g.b0}${if (g.b1 != g.b0) "–${g.b1}" else ""} (${freq(r, g.b0, g.b1)}): ${describe(r, g)}", Modifier.padding(bottom = 8.dp), style = MaterialTheme.typography.bodyMedium)
        }
    }

    GroupTitle("Statistics")
    val fs = r.floor.average(); val hiBin = (0 until SweepRec.BINS).maxBy { r.max[it] }; val busy = (0 until SweepRec.BINS).maxBy { r.duty[it] }
    val missing = (0 until nb).count { r.masked[it] }
    Card2 {
        listOf(
            clock(total) to "recorded, ${r.sweeps} sweeps",
            "%.0f dBm".format(fs) to "typical level between peaks (${r.floor.min()} to ${r.floor.max()})",
            "%.1f %%".format(r.duty.average()) to "average share over the threshold (${r.thrDb} dB)",
            "${r.max[hiBin]} dBm" to "highest level, bin $hiBin (${mhz(r.khz(hiBin))} MHz)",
            "bin $busy · ${r.duty[busy]} %" to "most active frequency, ${mhz(r.khz(busy))} MHz",
            "${r.events.size} / ${groups.size}" to "events / groups",
            "${Math.round(missing * 100.0 / nb)} %" to "of the time not measured",
            size(r.size) to "file size, instead of ${size(r.sweeps * SweepRec.BINS)} raw",
        ).chunked(2).forEach { row ->
            Row(Modifier.fillMaxWidth().padding(bottom = 10.dp)) {
                row.forEach { (v, l) ->
                    Column(Modifier.weight(1f)) {
                        Text(v, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.Bold)
                        Text(l, style = MaterialTheme.typography.bodySmall, color = ink)
                    }
                }
            }
        }
    }
}

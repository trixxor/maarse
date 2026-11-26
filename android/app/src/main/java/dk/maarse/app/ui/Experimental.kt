package dk.maarse.app.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.BatteryChargingFull
import androidx.compose.material.icons.outlined.GpsFixed
import androidx.compose.material.icons.outlined.GraphicEq
import androidx.compose.material.icons.outlined.ShowChart
import androidx.compose.material.icons.outlined.Timeline
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dk.maarse.app.History
import dk.maarse.app.Node
import dk.maarse.app.SweepStore
import dk.maarse.app.tr

/** The Experimental menu (Settings > Experimental): new tools that only read from the radio. */
@Composable
fun ExperimentalMenu(onGo: (MePage) -> Unit) {
    val stats by Node.stats.collectAsState()
    val coverage by History.coverageOn.collectAsState()
    var keep by remember { mutableStateOf(History.keep) }
    Card2(Modifier.padding(top = 4.dp)) {
        Text("These tools are new. They only read from the radio and never change how messages are sent. Numbers can be rough.", style = MaterialTheme.typography.bodyMedium)
    }
    GroupTitle("Tools")
    Group {
        GroupRow("Spectrum and waterfall", "Who is on the air around you", leading = { RowIcon(Icons.Outlined.GraphicEq) }) { onGo(MePage.X_SPECTRUM) }
        GroupRow("Recordings", SweepStore.items.collectAsState().value.size.let { if (it == 0) "Record a scan for minutes and play it back" else "$it saved scans" }, leading = { RowIcon(Icons.Outlined.Timeline) }) { onGo(MePage.X_RECORDINGS) }
        GroupRow("GPS", stats[103]?.let { "$it satellites in use" } ?: "Satellites, sky view, height, speed", leading = { RowIcon(Icons.Outlined.GpsFixed) }) { onGo(MePage.X_GPS) }
        GroupRow("Signal per neighbour", "How each link changes over time", leading = { RowIcon(Icons.Outlined.ShowChart) }) { onGo(MePage.X_NEIGHBOURS) }
        GroupRow("Ping test", "Loss, round trip and range along a route", leading = { RowIcon(Icons.Outlined.Timeline) }) { onGo(MePage.X_PING) }
        GroupRow("Battery and temperature", stats[85]?.let { "$it %" } ?: "History and time left", last = true, leading = { RowIcon(Icons.Outlined.BatteryChargingFull) }) { onGo(MePage.X_POWER) }
    }
    GroupTitle("Options")
    Group {
        SwitchRow("Show coverage on the map", "Adds an optional layer to the Map tab: how far your messages reach, from the places you have been.", coverage) { History.showCoverage = it }
        SwitchRow("Keep history", "Battery, temperature, noise and neighbour signals, for the last 7 days. Stays on the phone.", keep, last = true) { keep = it; History.keep = it }
    }
}

// ---------- charts ----------

private val LINE_COLOURS = listOf(Color(0xFF1F6F5C), Color(0xFF5B8CC6), Color(0xFFB8730A), Color(0xFF9C4A6E), Color(0xFF6B5B95))

/** One line in a chart: points as (time in ms, value). */
class Series(val color: Color, val points: List<Pair<Long, Float>>)

/**
 * A line chart on a time axis. `shade` marks time spans (for example "on USB power"), `low` a band under that value.
 * `area` fills under the first line.
 */
@Composable
private fun TimeChart(
    series: List<Series>, from: Long, to: Long, yMin: Float, yMax: Float, ticks: List<Float>, unit: String = "",
    shade: List<Pair<Long, Long>> = emptyList(), low: Float? = null, area: Boolean = false, height: Int = 150,
) {
    val grid = MaterialTheme.colorScheme.outlineVariant
    val ink = MaterialTheme.colorScheme.onSurfaceVariant
    val shadeColor = Color(0xFF7FD1A5).copy(alpha = 0.28f)
    val lowColor = Color(0xFFFDF1E6).copy(alpha = 0.9f)
    Row(Modifier.fillMaxWidth().height(height.dp)) {
        Column(Modifier.size(width = 34.dp, height = height.dp), verticalArrangement = Arrangement.SpaceBetween, horizontalAlignment = Alignment.End) {
            ticks.sortedDescending().forEach { Text("%.0f".format(it), style = MaterialTheme.typography.labelSmall, color = ink) }
        }
        Canvas(Modifier.fillMaxWidth().height(height.dp).padding(start = 6.dp)) {
            fun x(t: Long) = ((t - from).toFloat() / (to - from).coerceAtLeast(1)) * size.width
            fun y(v: Float) = size.height - ((v.coerceIn(yMin, yMax) - yMin) / (yMax - yMin)) * size.height
            low?.let { drawRect(lowColor, Offset(0f, y(it)), androidx.compose.ui.geometry.Size(size.width, size.height - y(it))) }
            ticks.forEach { drawLine(grid, Offset(0f, y(it)), Offset(size.width, y(it)), 1.dp.toPx()) }
            shade.forEach { (a, b) ->
                val xa = x(a.coerceAtLeast(from)); val xb = x(b.coerceAtMost(to))
                if (xb > xa) drawRect(shadeColor, Offset(xa, 0f), androidx.compose.ui.geometry.Size(xb - xa, size.height))
            }
            series.forEachIndexed { si, s ->
                val pts = s.points.filter { it.first in from..to }
                if (pts.size < 2) return@forEachIndexed
                val path = Path()
                pts.forEachIndexed { i, p -> if (i == 0) path.moveTo(x(p.first), y(p.second)) else path.lineTo(x(p.first), y(p.second)) }
                if (area && si == 0) {
                    val fill = Path().apply { addPath(path); lineTo(x(pts.last().first), size.height); lineTo(x(pts.first().first), size.height); close() }
                    drawPath(fill, s.color.copy(alpha = 0.14f))
                }
                drawPath(path, s.color, style = Stroke(width = 2.dp.toPx(), cap = StrokeCap.Round, join = androidx.compose.ui.graphics.StrokeJoin.Round))
            }
        }
    }
}

@Composable
private fun RangeChips(labels: List<String>, selected: Int, onSelect: (Int) -> Unit) {
    ChipRow(labels, selected, onSelect)
}

private fun hoursText(h: Double): String {
    val total = h.toInt()
    return if (total >= 48) "about ${total / 24} d ${total % 24} h" else if (total >= 1) "about $total h" else "under an hour"
}

// ---------- battery and temperature ----------

@Composable
fun PowerPage() {
    val stats by Node.stats.collectAsState()
    val hist by History.power.collectAsState()
    var range by remember { mutableIntStateOf(0) }
    val now = System.currentTimeMillis()
    val span = if (range == 0) 24 * 3600_000L else 7 * 24 * 3600_000L
    val pct = stats[85]; val mv = stats[49]?.takeIf { it > 0 }; val ext = stats[82] == 1
    Card2(Modifier.padding(top = 4.dp)) {
        Text("Radio battery", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (pct == null && mv == null) Text("No battery value from the radio yet.", style = MaterialTheme.typography.bodyMedium)
        else {
            Row(verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Text(pct?.let { "$it %" } ?: "–", style = MaterialTheme.typography.displaySmall, fontWeight = FontWeight.Bold)
                mv?.let { Text("%.2f V".format(it / 1000.0), color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(bottom = 6.dp)) }
                if (ext) Text("charging or on USB power", color = MaterialTheme.colorScheme.primary, modifier = Modifier.padding(bottom = 6.dp))
            }
            val low = (pct ?: 100) < 20 && !ext
            LinearProgressIndicator(
                progress = { (pct ?: 0).coerceIn(3, 100) / 100f }, modifier = Modifier.fillMaxWidth().height(16.dp).clip(RoundedCornerShape(8.dp)),
                color = if (low) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.primary,
            )
            val drain = History.drainPerHour()
            Row(Modifier.fillMaxWidth().padding(top = 8.dp)) {
                Column(Modifier.weight(1f)) {
                    Text("Time left", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Text(if (ext) "on power" else if (drain != null && pct != null) hoursText(pct / drain) else "not enough data yet", fontWeight = FontWeight.SemiBold)
                }
                Column(Modifier.weight(1f)) {
                    Text("Uses", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Text(if (drain != null && !ext) "%.1f %% per hour".format(drain) else "–", fontWeight = FontWeight.SemiBold)
                }
            }
            Text("Estimated from the last 6 hours. It changes when you send or receive a lot.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
    GroupTitle("Battery history")
    RangeChips(listOf("24 h", "7 days"), range) { range = it }
    Card2(Modifier.padding(top = 8.dp)) {
        val data = hist.filter { it.pct != null && it.t >= now - span }
        if (data.size < 2) Text(if (History.keep) "History builds up while the app runs and a radio answers (one value a minute)." else "History is off. Turn it on under Experimental.", style = MaterialTheme.typography.bodyMedium)
        else {
            // On-power spans: runs of samples with external power.
            val shade = mutableListOf<Pair<Long, Long>>()
            var start: Long? = null
            data.forEach { if (it.ext) { if (start == null) start = it.t } else if (start != null) { shade += start!! to it.t; start = null } }
            start?.let { shade += it to now }
            TimeChart(listOf(Series(LINE_COLOURS[0], data.map { it.t to it.pct!!.toFloat() })), now - span, now, 0f, 100f, listOf(0f, 25f, 50f, 75f, 100f), shade = shade, low = 20f, area = true)
            Text("Green: on USB power · amber band: under 20 %", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
    GroupTitle("Chip temperature")
    Card2 {
        val temp = hist.filter { it.tempDc != null && it.t >= now - span }
        stats[86]?.let { Text("now %.1f °C".format(it / 10.0), fontWeight = FontWeight.SemiBold) }
        if (temp.size < 2) Text("Not enough readings yet.", style = MaterialTheme.typography.bodyMedium)
        else {
            val lo = (temp.minOf { it.tempDc!! } / 10f - 3).coerceAtMost(10f); val hi = (temp.maxOf { it.tempDc!! } / 10f + 3).coerceAtLeast(40f)
            TimeChart(listOf(Series(LINE_COLOURS[2], temp.map { it.t to it.tempDc!! / 10f })), now - span, now, lo, hi, listOf(lo, (lo + hi) / 2, hi), height = 110)
        }
        Text("Warmer than the air around it, because it is the chip's own temperature.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

// ---------- signal per neighbour ----------

@Composable
fun NeighbourSignalPage() {
    val hist by History.links.collectAsState()
    val nodes by Node.nodes.collectAsState()
    var range by remember { mutableIntStateOf(1) }
    val now = System.currentTimeMillis()
    val span = longArrayOf(3600_000L, 24 * 3600_000L, 7 * 24 * 3600_000L)[range]
    LaunchedNeighbours()
    RangeChips(listOf("1 h", "24 h", "7 days"), range) { range = it }
    val data = hist.filter { it.t >= now - span }
    val byLink = data.groupBy { it.prefix }.entries.sortedByDescending { it.value.size }.take(5)
    Card2(Modifier.padding(top = 8.dp)) {
        Text("Signal over time", fontWeight = FontWeight.SemiBold)
        Text("SNR in dB, one line per neighbour. Higher is better; under -10 dB gets shaky.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (byLink.isEmpty()) Text(if (History.keep) "No neighbours heard yet. A value is saved once a minute while the app runs." else "History is off. Turn it on under Experimental.", style = MaterialTheme.typography.bodyMedium)
        else TimeChart(byLink.mapIndexed { i, e -> Series(LINE_COLOURS[i % LINE_COLOURS.size], e.value.map { it.t to it.snr.toFloat() }) }, now - span, now, -20f, 10f, listOf(-20f, -10f, 0f, 10f), low = -10f, height = 170)
    }
    if (byLink.isNotEmpty()) Group {
        byLink.forEachIndexed { i, e ->
            val last = e.value.last()
            val addr = nodes.keys.firstOrNull { it.startsWith(e.key) }
            val title = addr?.let { Node.label(it) } ?: e.key
            val first = e.value.take(3).map { it.snr }.average(); val end = e.value.takeLast(3).map { it.snr }.average()
            val trend = when { e.value.size < 6 -> "new"; end - first <= -3 -> "dropping"; end - first >= 3 -> "improving"; else -> "steady" }
            val colour = when { last.snr >= 0 -> MaterialTheme.colorScheme.primary; last.snr >= -10 -> Color(0xFFB8730A); else -> MaterialTheme.colorScheme.error }
            Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Box(Modifier.size(10.dp).clip(CircleShape).background(LINE_COLOURS[i % LINE_COLOURS.size]))
                Column(Modifier.weight(1f)) {
                    Text(title, fontWeight = FontWeight.SemiBold)
                    Text("${e.value.size} readings · last ${ago(now - last.t)} ago", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                Column(horizontalAlignment = Alignment.End) {
                    Text("${if (last.snr > 0) "+" else ""}${last.snr} dB", fontWeight = FontWeight.Bold, color = colour)
                    Text("${last.rssi} dBm · ${tr(trend)}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
            if (i < byLink.lastIndex) RowDivider()
        }
    }
}

@Composable
private fun LaunchedNeighbours() {
    androidx.compose.runtime.DisposableEffect(Unit) {
        Node.fastPoll = true
        Node.send(dk.maarse.app.Cmd.getNeighbors())
        onDispose { Node.fastPoll = false }
    }
}

private fun ago(ms: Long): String {
    val s = ms / 1000
    return if (s < 90) "$s s" else if (s < 5400) "${s / 60} min" else "${s / 3600} h"
}

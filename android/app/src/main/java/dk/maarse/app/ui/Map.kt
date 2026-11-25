@file:OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)

package dk.maarse.app.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dk.maarse.app.History
import dk.maarse.app.Node
import dk.maarse.app.Sensors
import dk.maarse.app.tr
import kotlin.math.abs
import kotlin.math.cos
import kotlin.math.floor
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min

/** What is drawn on the map. */
private enum class PKind { PERSON, REPEATER, SENSOR, NODE }

private data class MapPoint(
    val id: String, val name: String, val kind: PKind, val lat: Double, val lon: Double,
    val hops: Int? = null, val heard: Long = 0, val state: Int = 0, val value: String = "",
)

private val MAP_FILTERS = listOf("All", "People", "Repeaters", "Sensors")
private val NICE_KM = doubleArrayOf(0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0)

/**
 * The Map place: an offline grid, no tiles and no internet. Only nodes that share a position are drawn
 * (a dot is a person, a rounded square a repeater, a teal square a sensor). Tap one to see who it is.
 * `onNode` opens a person or repeater, `onSensor` a sensor, `onMessage` starts a chat (card button, optional).
 */
@Composable
fun MapScreen(onNode: (String) -> Unit, onSensor: (String) -> Unit = {}, onMessage: ((String) -> Unit)? = null) {
    val nodes by Node.nodes.collectAsState()
    Node.nicknames.collectAsState().value
    val sensors by Sensors.list.collectAsState()
    LaunchedEffect(Unit) { Sensors.refresh() }
    var filter by remember { mutableIntStateOf(0) }
    var selected by remember { mutableStateOf<String?>(null) }
    val sensorNodes = (sensors ?: emptyList()).map { it.node }.toSet()

    val stats by Node.stats.collectAsState()
    val all = buildList {
        // Our own GPS position, exact (a fix from the radio), is drawn as "You".
        val glat = stats[116]; val glon = stats[117]
        if (glat != null && glon != null && (glat != 0 || glon != 0)) add(MapPoint("me", "You (GPS)", PKind.PERSON, glat / 1e7, glon / 1e7))
        for (n in nodes.values) {
            if (n.lat == null || n.lon == null || n.addr in sensorNodes) continue
            add(MapPoint(n.addr, friendlyName(Node.label(n.addr)), when (kindOf(n)) { NodeKind.PERSON -> PKind.PERSON; NodeKind.REPEATER -> PKind.REPEATER; NodeKind.UNKNOWN -> PKind.NODE }, n.lat!!, n.lon!!, n.hops, n.lastHeard))
        }
        for (s in sensors ?: emptyList()) {
            val e = nodes[s.node] ?: continue
            if (e.lat == null || e.lon == null) continue
            val lead = s.channels.firstOrNull { it.value != null }
            add(MapPoint(s.node, s.title, PKind.SENSOR, e.lat!!, e.lon!!, e.hops, e.lastHeard,
                state = if (s.alarm != null) 2 else if (s.quiet) 1 else 0,
                value = if (s.quiet) "quiet" else lead?.let { it.value?.let { v -> (if (abs(v) >= 1000) v.toLong().toString() else "%.1f".format(v).removeSuffix(".0")) + if (it.unit.isNotBlank()) " " + it.unit else "" } } ?: ""))
        }
    }
    val shown = all.filter { filter == 0 || (filter == 1 && it.kind == PKind.PERSON) || (filter == 2 && it.kind == PKind.REPEATER) || (filter == 3 && it.kind == PKind.SENSOR) }
    val sel = all.firstOrNull { it.id == selected }
    val noPosition = nodes.values.count { it.lat == null && it.addr !in sensorNodes }
    val coverageOn by History.coverageOn.collectAsState()
    val spots by History.spots.collectAsState()

    // The grid takes all the height that is left; the card or hint, the legend and the one note sit below it.
    Column(Modifier.fillMaxSize()) {
        ChipRow(MAP_FILTERS, filter, { filter = it; selected = null })
        // The coverage layer is a choice (Experimental): where the radio heard others, from the places you have been.
        androidx.compose.material3.FilterChip(
            coverageOn, { History.showCoverage = !coverageOn }, { Text("Coverage") },
            Modifier.padding(horizontal = 16.dp),
        )
        MapCanvas(all, shown, selected, { selected = it }, Modifier.weight(1f).heightIn(min = 160.dp), if (coverageOn) spots else emptyList())
        Column(Modifier.heightIn(max = 300.dp).verticalScroll(rememberScrollState()).padding(bottom = 8.dp)) {
            if (sel != null) PointCard(sel, onNode, onSensor, onMessage)
            else if (all.isEmpty()) Column(Modifier.padding(horizontal = 24.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text("No node shares a position yet", fontWeight = FontWeight.SemiBold)
                Text("A node shows up here when it shares where it is (under Name and position). Nothing is guessed.", color = MaterialTheme.colorScheme.onSurfaceVariant)
            } else if (shown.isEmpty()) Text("Nothing in this filter shares a position.", Modifier.padding(horizontal = 24.dp, vertical = 8.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
            else Text("Tap a point to see who it is and how well you hear it.", Modifier.padding(horizontal = 24.dp, vertical = 6.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
            if (coverageOn) CoverageNote(spots)
            Legend()
            if (all.isNotEmpty() && noPosition > 0) Text(
                if (noPosition == 1) "1 node has not shared a position and is not drawn." else "$noPosition nodes have not shared a position and are not drawn.",
                Modifier.padding(horizontal = 24.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

/** Where the points are in kilometres from a middle point, and how big a square of the grid is. */
private class View(val lat0: Double, val lon0: Double, val kmPerPx: Double, val step: Double, val w: Float, val h: Float) {
    private val kx = 111.32 * cos(Math.toRadians(lat0))
    fun x(lon: Double) = (w / 2 + (lon - lon0) * kx / kmPerPx).toFloat()
    fun y(lat: Double) = (h / 2 - (lat - lat0) * 110.57 / kmPerPx).toFloat()
}

private fun makeView(points: List<MapPoint>, w: Float, h: Float): View {
    if (points.isEmpty() || w <= 0f) return View(0.0, 0.0, 1.0, 1.0, w, h)
    val latMin = points.minOf { it.lat }; val latMax = points.maxOf { it.lat }
    val lonMin = points.minOf { it.lon }; val lonMax = points.maxOf { it.lon }
    val lat0 = (latMin + latMax) / 2; val lon0 = (lonMin + lonMax) / 2
    val kx = 111.32 * cos(Math.toRadians(lat0))
    // At least 2 km across so a single point does not fill the map; 20 % room around the edge.
    val spanX = max((lonMax - lonMin) * kx, 2.0) * 1.4
    val spanY = max((latMax - latMin) * 110.57, 2.0 * h / w) * 1.4
    val kmPerPx = max(spanX / w, spanY / h)
    val kmWide = kmPerPx * w
    val step = NICE_KM.firstOrNull { kmWide / it <= 8 } ?: NICE_KM.last()
    return View(lat0, lon0, kmPerPx, step, w, h)
}

private fun kmText(km: Double) = if (km < 1) "${(km * 1000).toInt()} m" else if (km == floor(km)) "${km.toInt()} km" else "$km km"

@Composable
private fun MapCanvas(all: List<MapPoint>, shown: List<MapPoint>, selected: String?, onSelect: (String?) -> Unit, modifier: Modifier = Modifier, spots: List<History.Spot> = emptyList()) {
    val measurer = rememberTextMeasurer()
    val cs = MaterialTheme.colorScheme
    val grid = cs.outlineVariant
    val ink = cs.onSurface
    val muted = cs.onSurfaceVariant
    val primary = cs.primary
    val teal = SensorColors.teal
    val red = cs.error
    val amber = Amber
    val label = TextStyle(fontSize = 11.sp, color = ink, fontWeight = FontWeight.SemiBold)
    val small = TextStyle(fontSize = 10.sp, color = muted)
    val colours = all.filter { it.kind == PKind.PERSON }.associate { it.id to avatarColor(it.id) }
    var box by remember { mutableStateOf(Size.Zero) }
    // The view also holds the coverage spots, so they are all on the map.
    val view = remember(all, spots, box) { makeView(all + spots.map { MapPoint("spot", "", PKind.NODE, it.latE6 / 1e6, it.lonE6 / 1e6) }, box.width, box.height) }
    Canvas(
        modifier.padding(horizontal = 16.dp, vertical = 6.dp).fillMaxWidth().clip(RoundedCornerShape(16.dp))
            .background(cs.surfaceContainerHigh)
            .onSizeChanged { box = Size(it.width.toFloat(), it.height.toFloat()) }
            .pointerInput(shown, view) {
                detectTapGestures { tap ->
                    val near = shown.minByOrNull { hypot((view.x(it.lon) - tap.x).toDouble(), (view.y(it.lat) - tap.y).toDouble()) }
                    val d = near?.let { hypot((view.x(it.lon) - tap.x).toDouble(), (view.y(it.lat) - tap.y).toDouble()) } ?: Double.MAX_VALUE
                    onSelect(if (near != null && d < 40.dp.toPx()) near.id else null)
                }
            },
    ) {
        // The grid: lines at whole steps, so a square is one step wide.
        val stepPx = (view.step / view.kmPerPx).toFloat()
        if (stepPx > 4f) {
            val kx = 111.32 * cos(Math.toRadians(view.lat0))
            fun pm(v: Double, m: Double) = ((v % m) + m) % m
            val offX = pm(view.lon0 * kx, view.step) / view.kmPerPx
            val offY = pm(view.lat0 * 110.57, view.step) / view.kmPerPx
            var x = (((size.width / 2 - offX) % stepPx) + stepPx).toFloat() % stepPx
            while (x < size.width) { drawLine(grid, Offset(x, 0f), Offset(x, size.height), 1.dp.toPx()); x += stepPx }
            var y = (((size.height / 2 + offY) % stepPx) + stepPx).toFloat() % stepPx
            while (y < size.height) { drawLine(grid, Offset(0f, y), Offset(size.width, y), 1.dp.toPx()); y += stepPx }
        }
        drawText(measurer, tr("Offline grid · 1 square ≈ {0}", kmText(view.step)), Offset(8.dp.toPx(), size.height - 18.dp.toPx()), small)
        // Coverage: a dot per place where something was heard (green strong, amber weak, red barely), and the outline of the reach.
        if (spots.isNotEmpty()) {
            hull(spots).takeIf { it.size >= 3 }?.let { h ->
                val path = androidx.compose.ui.graphics.Path().apply { h.forEachIndexed { i, sp -> val o = Offset(view.x(sp.lonE6 / 1e6), view.y(sp.latE6 / 1e6)); if (i == 0) moveTo(o.x, o.y) else lineTo(o.x, o.y) }; close() }
                drawPath(path, primary.copy(alpha = 0.08f))
                drawPath(path, primary, style = Stroke(1.5.dp.toPx(), pathEffect = androidx.compose.ui.graphics.PathEffect.dashPathEffect(floatArrayOf(14f, 10f))))
            }
            for (sp in spots) {
                val c = if (sp.snr >= 0) primary else if (sp.snr >= -10) amber else red
                drawCircle(c.copy(alpha = 0.6f), 5.dp.toPx(), Offset(view.x(sp.lonE6 / 1e6), view.y(sp.latE6 / 1e6)))
            }
        }
        // Dim the points the filter hides, draw the rest on top.
        val order = all.sortedBy { it.id == selected }
        for (p in order) {
            val on = p in shown
            val c = Offset(view.x(p.lon), view.y(p.lat))
            val a = if (on) 1f else 0.18f
            val r = 7.dp.toPx() + if (p.id == selected) 3.dp.toPx() else 0f
            when (p.kind) {
                PKind.PERSON -> drawCircle(colours.getValue(p.id).copy(alpha = a), r, c)
                PKind.NODE -> drawCircle(cs.outline.copy(alpha = a), r, c)
                PKind.REPEATER -> drawRoundRect(primary.copy(alpha = a), Offset(c.x - r, c.y - r), Size(2 * r, 2 * r), CornerRadius(3.dp.toPx()))
                PKind.SENSOR -> {
                    val fill = if (p.state == 1) amber else teal
                    drawRoundRect(fill.copy(alpha = a), Offset(c.x - r, c.y - r), Size(2 * r, 2 * r), CornerRadius(4.dp.toPx()))
                    if (p.state == 2 && on) drawRoundRect(red, Offset(c.x - r - 5.dp.toPx(), c.y - r - 5.dp.toPx()), Size(2 * r + 10.dp.toPx(), 2 * r + 10.dp.toPx()), CornerRadius(8.dp.toPx()), style = Stroke(2.dp.toPx()))
                }
            }
            if (p.id == selected) drawCircle(primary, r + 8.dp.toPx(), c, style = Stroke(2.dp.toPx()))
            if (on) {
                val name = p.name.take(14)
                val x = c.x + r + 5.dp.toPx()
                val room = size.width - x - 4.dp.toPx()
                val lay = measurer.measure(name, label, overflow = TextOverflow.Ellipsis, maxLines = 1, constraints = androidx.compose.ui.unit.Constraints(maxWidth = max(room, 40f).toInt()))
                val flip = room < lay.size.width
                drawText(lay, topLeft = Offset(if (flip) c.x - r - 5.dp.toPx() - lay.size.width else x, c.y - lay.size.height / 2f))
                if (p.kind == PKind.SENSOR && p.value.isNotBlank()) {
                    val v = measurer.measure(p.value, small)
                    drawText(v, topLeft = Offset(if (flip) c.x - r - 5.dp.toPx() - v.size.width else x, c.y + lay.size.height / 2f - 2.dp.toPx()))
                }
            }
        }
    }
}

@Composable
private fun PointCard(p: MapPoint, onNode: (String) -> Unit, onSensor: (String) -> Unit, onMessage: ((String) -> Unit)?) {
    Column(Modifier.padding(horizontal = 16.dp, vertical = 4.dp).fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(MaterialTheme.colorScheme.surfaceContainer).padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Avatar(p.name, p.id, 44.dp)
            Column(Modifier.weight(1f)) {
                Text(p.name, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                val kind = when (p.kind) { PKind.PERSON -> "Person"; PKind.REPEATER -> "Repeater"; PKind.SENSOR -> "Sensor"; PKind.NODE -> "Node" }
                Text(listOfNotNull(kind, if (p.kind == PKind.SENSOR) p.value.ifBlank { null } else hopsText(p.hops), if (p.heard > 0) tr("heard {0}", time(p.heard)) else null).joinToString(" · "),
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            if (p.kind != PKind.SENSOR) snrOf(p.id)?.let { SignalBars(it) }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            val message = onMessage
            if (message != null && p.kind == PKind.PERSON) Button(onClick = { message("dm:" + p.id) }, Modifier.heightIn(min = 48.dp)) { Text("Message", maxLines = 1) }
            val open = { if (p.kind == PKind.SENSOR) onSensor(p.id) else onNode(p.id) }
            if (message != null && p.kind == PKind.PERSON) OutlinedButton(onClick = open, Modifier.heightIn(min = 48.dp)) { Text("Open", maxLines = 1) }
            else Button(onClick = open, Modifier.heightIn(min = 48.dp)) { Text("Open", maxLines = 1) }
        }
    }
}

@Composable
private fun Legend() {
    FlowRow(Modifier.padding(horizontal = 24.dp, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(16.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        LegendItem(MaterialTheme.colorScheme.onSurfaceVariant, true, "Person")
        LegendItem(MaterialTheme.colorScheme.primary, false, "Repeater")
        LegendItem(MaterialTheme.colorScheme.outline, true, "Node")
        LegendItem(SensorColors.teal, false, "Sensor")
    }
}

@Composable
private fun LegendItem(color: Color, round: Boolean, text: String) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Box(Modifier.size(10.dp).clip(if (round) RoundedCornerShape(50) else RoundedCornerShape(3.dp)).background(color))
        Text(text, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, softWrap = false)
    }
}

/** The outline of the places something was heard (convex hull, Andrew's monotone chain). */
private fun hull(spots: List<History.Spot>): List<History.Spot> {
    val pts = spots.distinctBy { it.latE6 to it.lonE6 }.sortedWith(compareBy({ it.lonE6 }, { it.latE6 }))
    if (pts.size < 3) return pts
    fun cross(o: History.Spot, a: History.Spot, b: History.Spot) =
        (a.lonE6 - o.lonE6).toLong() * (b.latE6 - o.latE6) - (a.latE6 - o.latE6).toLong() * (b.lonE6 - o.lonE6)
    val lower = ArrayList<History.Spot>()
    for (p in pts) { while (lower.size >= 2 && cross(lower[lower.size - 2], lower.last(), p) <= 0) lower.removeAt(lower.size - 1); lower.add(p) }
    val upper = ArrayList<History.Spot>()
    for (p in pts.asReversed()) { while (upper.size >= 2 && cross(upper[upper.size - 2], upper.last(), p) <= 0) upper.removeAt(upper.size - 1); upper.add(p) }
    return lower.dropLast(1) + upper.dropLast(1)
}

/** One line under the map when the coverage layer is on: how many spots and how far apart the farthest two are. */
@Composable
private fun CoverageNote(spots: List<History.Spot>) {
    val text = if (spots.isEmpty())
        "No coverage yet. A spot is saved when the radio hears someone and the phone knows where it is. It stays on the phone."
    else {
        val h = hull(spots)
        var far = 0.0
        for (a in h) for (b in h) far = max(far, History.distanceM(a.latE6 / 1e6, a.lonE6 / 1e6, b.latE6 / 1e6, b.lonE6 / 1e6))
        tr("{0} spots in the last 7 days, farthest apart {1}.", spots.size, if (far < 1000) "${far.toInt()} m" else "%.1f km".format(far / 1000))
    }
    Text(text, Modifier.padding(horizontal = 24.dp, vertical = 4.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

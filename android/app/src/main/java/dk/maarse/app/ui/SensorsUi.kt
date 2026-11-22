@file:OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class, androidx.compose.material3.ExperimentalMaterial3Api::class)

package dk.maarse.app.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import android.content.ClipData
import android.content.ClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.foundation.layout.size
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.sp
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.Button
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Switch
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.unit.dp
import dk.maarse.app.SensorChan
import dk.maarse.app.SensorInfo
import dk.maarse.app.Sensors
import org.json.JSONArray
import org.json.JSONObject

/** Whether the Nodes tab shows sensors (kept when you go into one and back). */
object SensorsNav { var on by mutableStateOf(false); var filter by mutableStateOf("all") }

private fun JSONArray.objs(): List<JSONObject> = (0 until length()).map { getJSONObject(it) }
private fun fmt(v: Double?): String = if (v == null) "–" else if (kotlin.math.abs(v) >= 1000) v.toLong().toString() else String.format(java.util.Locale.ROOT, "%.1f", v).removeSuffix(".0")
private fun ago(s: Long): String = when {
    s < 90 -> "just now"; s < 5400 -> "${s / 60} min ago"; s < 129600 -> "${s / 3600} h ago"; else -> "${s / 86400} days ago"
}

// ---------- small parts ----------

/** A rounded card that fills the width of the screen's own padding (the app's Card2 adds a margin of its own). */
@Composable
private fun SCard(content: @Composable () -> Unit) = Column(
    Modifier.fillMaxWidth().clip(RoundedCornerShape(18.dp)).background(MaterialTheme.colorScheme.surfaceContainer).padding(16.dp),
    verticalArrangement = Arrangement.spacedBy(6.dp)) { content() }

@Composable
private fun SGroup(content: @Composable () -> Unit) = Column(
    Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(MaterialTheme.colorScheme.surfaceContainer)) { content() }

@Composable
private fun STitle(t: String) = Text(t, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary,
    fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(start = 12.dp, top = 6.dp))

@Composable
private fun Chip(text: String, kind: Int, onClick: () -> Unit) {
    val c = MaterialTheme.colorScheme
    val dark = c.background.luminance() < 0.5f
    val (bg, fg) = when (kind) {
        0 -> if (dark) Color(0xFF1B2B4A) to Color(0xFF8AB4FF) else Color(0xFFDBE6FB) to Color(0xFF173F86)
        1 -> if (dark) Color(0xFF352A15) to Color(0xFFF0B252) else Color(0xFFFBEFD9) to Color(0xFF6E4300)
        2 -> if (dark) Color(0xFF1D3A31) to Color(0xFF5CC0A0) else Color(0xFFDCECE6) to Color(0xFF16503F)
        4 -> if (dark) Color(0xFF34234A) to Color(0xFFCDA2FF) else Color(0xFFEBDCF7) to Color(0xFF5D2C8A)
        else -> c.surfaceContainerHigh to c.onSurface
    }
    Box(Modifier.defaultMinSize(minHeight = 44.dp).clip(RoundedCornerShape(24.dp)).background(bg).clickable(onClick = onClick).padding(horizontal = 16.dp), contentAlignment = Alignment.Center) {
        Text(text, color = fg, fontWeight = FontWeight.SemiBold)
    }
}

@Composable
private fun Kw(t: String) = Text(t.uppercase(), style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.ExtraBold,
    color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.fillMaxWidth())

@Composable
private fun PickDialog(title: String, options: List<Pair<String, String>>, current: String?, onPick: (String) -> Unit, onDismiss: () -> Unit) {
    AlertDialog(onDismissRequest = onDismiss, title = { Text(title) }, confirmButton = {}, dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState())) {
                for ((v, l) in options) Row(Modifier.fillMaxWidth().clickable { onPick(v); onDismiss() }.padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    RadioButton(selected = v == current, onClick = { onPick(v); onDismiss() }); Text(l)
                }
            }
        })
}

@Composable
private fun NumberDialog(title: String, initial: Double, unit: String, onOk: (Double) -> Unit, onDismiss: () -> Unit) {
    var t by remember { mutableStateOf(fmt(initial)) }
    AlertDialog(onDismissRequest = onDismiss, title = { Text(title) },
        text = { OutlinedTextField(t, { t = it }, singleLine = true, suffix = { Text(unit) }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal)) },
        confirmButton = { TextButton(onClick = { t.replace(',', '.').toDoubleOrNull()?.let { onOk(it); onDismiss() } }) { Text("OK") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } })
}

@Composable
private fun TextDialog(title: String, initial: String, onOk: (String) -> Unit, onDismiss: () -> Unit) {
    var t by remember { mutableStateOf(initial) }
    AlertDialog(onDismissRequest = onDismiss, title = { Text(title) },
        text = { OutlinedTextField(t, { t = it.take(40) }, singleLine = true) },
        confirmButton = { TextButton(onClick = { onOk(t.trim()); onDismiss() }) { Text("OK") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } })
}

/** A simple line chart: readings over time, with dashed threshold lines, alarm marks and shaded spans. */
@Composable
private fun LineChart(points: List<Pair<Double, Double?>>, modifier: Modifier = Modifier, from: Double? = null, to: Double? = null,
                      lines: List<Double> = emptyList(), marks: List<Pair<Double, Boolean>> = emptyList(), bands: List<Pair<Double, Double>> = emptyList()) {
    val line = MaterialTheme.colorScheme.primary
    val warn = MaterialTheme.colorScheme.error
    val amber = Amber
    val span = MaterialTheme.colorScheme.tertiaryContainer
    Canvas(modifier.fillMaxWidth().height(110.dp).clip(RoundedCornerShape(10.dp)).background(MaterialTheme.colorScheme.surfaceContainerHigh)) {
        val ys = points.mapNotNull { it.second } + lines
        if (points.size < 2 || ys.isEmpty()) return@Canvas
        val t0 = from ?: points.first().first; val t1 = to ?: points.last().first
        var lo = ys.min(); var hi = ys.max(); if (hi - lo < 1) { lo -= .5; hi += .5 }
        val pad = 8.dp.toPx()
        fun x(t: Double) = (pad + (t - t0) / maxOf(1.0, t1 - t0) * (size.width - 2 * pad)).toFloat()
        fun y(v: Double) = (pad + (1 - (v - lo) / (hi - lo)) * (size.height - 2 * pad)).toFloat()
        for ((a, b) in bands) drawRect(span, Offset(x(a), pad), androidx.compose.ui.geometry.Size(maxOf(2f, x(b) - x(a)), size.height - 2 * pad))
        for (l in lines) drawLine(amber, Offset(pad, y(l)), Offset(size.width - pad, y(l)), 3f, pathEffect = PathEffect.dashPathEffect(floatArrayOf(14f, 10f)))
        val p = Path(); var first = true
        for ((t, v) in points) { if (v == null) continue; if (first) { p.moveTo(x(t), y(v)); first = false } else p.lineTo(x(t), y(v)) }
        drawPath(p, line, style = Stroke(width = 5f))
        for ((t, bad) in marks) drawLine(if (bad) warn else line, Offset(x(t), pad), Offset(x(t), size.height - pad), 3f)
    }
}

// ---------- the Nodes tab: Nodes | Sensors ----------

@Composable
fun SensorsToggle() {
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        FilterChip(selected = !SensorsNav.on, onClick = { SensorsNav.on = false }, label = { Text("Nodes") })
        FilterChip(selected = SensorsNav.on, onClick = { SensorsNav.on = true; Sensors.refresh() }, label = { Text("Sensors") })
    }
}

/** Your sensors, favourites first: values at a glance, alarms and quiet sensors stand out. */
@Composable
fun SensorsPanel(onOpen: (String) -> Unit, onNew: () -> Unit) {
    val list by Sensors.list.collectAsState()
    val err by Sensors.error.collectAsState()
    LaunchedEffect(Unit) { Sensors.refresh() }
    val shown = (list ?: emptyList()).filter {
        when (SensorsNav.filter) { "fav" -> it.favourite; "alarm" -> it.alarm != null || it.quiet; else -> true }
    }
    Box(Modifier.fillMaxSize()) {
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.padding(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            for ((k, l) in listOf("all" to "Mine", "fav" to "Favourites", "alarm" to "Alarms"))
                FilterChip(selected = SensorsNav.filter == k, onClick = { SensorsNav.filter = k }, label = { Text(l) })
        }
        if (err.isNotBlank()) Text(err, Modifier.padding(24.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
        else if (list != null && shown.isEmpty()) Text(if (list!!.isEmpty()) "No sensors yet. Set one up, or wait for one to send." else "Nothing here.",
            Modifier.padding(24.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
        LazyColumn(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(12.dp), contentPadding = androidx.compose.foundation.layout.PaddingValues(start = 16.dp, end = 16.dp, top = 12.dp, bottom = 96.dp)) {
            items(shown, key = { it.node }) { s -> SensorCard(s) { onOpen(s.node) } }
        }
    }
    ExtendedFloatingActionButton(onClick = onNew, Modifier.align(Alignment.BottomEnd).padding(20.dp),
        containerColor = MaterialTheme.colorScheme.primary, contentColor = MaterialTheme.colorScheme.onPrimary) { Text("+ Add sensor", fontWeight = FontWeight.SemiBold) }
    }
}

/** The last 24 hours of one value as a thin line. */
@Composable
private fun Spark(points: List<Pair<Double, Double?>>, color: Color) {
    Canvas(Modifier.fillMaxWidth().height(44.dp)) {
        val ys = points.mapNotNull { it.second }
        if (points.size < 2 || ys.size < 2) return@Canvas
        val t0 = points.first().first; val t1 = points.last().first
        var lo = ys.min(); var hi = ys.max(); if (hi - lo < 1) { lo -= .5; hi += .5 }
        val pad = 4.dp.toPx()
        val p = Path(); var first = true
        for ((t, v) in points) {
            if (v == null) continue
            val x = pad + ((t - t0) / maxOf(1.0, t1 - t0) * (size.width - 2 * pad)).toFloat()
            val y = pad + ((1 - (v - lo) / (hi - lo)) * (size.height - 2 * pad)).toFloat()
            if (first) { p.moveTo(x, y); first = false } else p.lineTo(x, y)
        }
        drawPath(p, color, style = Stroke(width = 5f, cap = androidx.compose.ui.graphics.StrokeCap.Round, join = androidx.compose.ui.graphics.StrokeJoin.Round))
    }
}

@Composable
private fun SensorCard(s: SensorInfo, onClick: () -> Unit) {
    val cs = MaterialTheme.colorScheme
    val quiet = s.quiet
    val alarm = s.alarm != null
    var pts by remember(s.node) { mutableStateOf<List<Pair<Double, Double?>>>(emptyList()) }
    val lead = s.channels.firstOrNull { it.unit.isNotBlank() } ?: s.channels.firstOrNull()
    LaunchedEffect(s.node, lead?.chan, s.ageS / 300) { if (lead != null && !quiet) Sensors.series(s.node, lead.chan, 86400) { pts = it } }
    val bg = if (quiet && !alarm) cs.tertiaryContainer else cs.surfaceContainer
    val shape = RoundedCornerShape(18.dp)
    Column(Modifier.fillMaxWidth().clip(shape).background(bg).then(if (alarm) Modifier.border(2.dp, cs.error, shape) else Modifier)
        .clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 14.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text((if (s.favourite) "★ " else "") + s.title, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
            if (alarm || quiet) Box(Modifier.clip(RoundedCornerShape(99.dp)).background(if (alarm) cs.errorContainer else cs.surface).padding(horizontal = 10.dp, vertical = 4.dp)) {
                Text(if (alarm) "Alarm" else "Quiet ${s.ageS / 3600} h", style = MaterialTheme.typography.labelMedium, fontWeight = FontWeight.Bold,
                    color = if (alarm) cs.onErrorContainer else cs.onTertiaryContainer)
            }
        }
        if (alarm) Text(s.alarm!!, fontWeight = FontWeight.SemiBold, color = cs.error)
        if (quiet && !alarm) {
            val bat = s.channels.firstOrNull { it.name.contains("battery", true) && it.value != null }
            Text("No reading for ${s.ageS / 3600} hours." + (if (bat != null) " Last battery reading was ${fmt(bat.value)} ${bat.unit}." else "") + " It may be flat.", color = cs.onTertiaryContainer)
        } else {
            Row(horizontalArrangement = Arrangement.spacedBy(20.dp)) {
                for (c in s.channels.take(3)) Column {
                    Text(c.name, style = MaterialTheme.typography.labelSmall, color = cs.onSurfaceVariant)
                    Text(fmt(c.value) + if (c.unit.isNotBlank()) " " + c.unit else "", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold, color = if (alarm && c === lead) cs.error else cs.onSurface)
                }
            }
            if (pts.size >= 2) Spark(pts, if (alarm) cs.error else cs.primary)
            Text("Heard " + ago(s.ageS), style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant)
        }
    }
}

// ---------- one sensor ----------

@Composable
fun SensorDetailScreen(node: String, onBack: () -> Unit, onEdit: (String) -> Unit) {
    val list by Sensors.list.collectAsState()
    val s = list?.firstOrNull { it.node == node }
    var range by remember { mutableStateOf(86400L) }
    var renaming by remember { mutableStateOf(false) }
    var forgetting by remember { mutableStateOf(false) }
    var alarms by remember { mutableStateOf<List<Triple<Long, Boolean, String>>>(emptyList()) }
    LaunchedEffect(node) { Sensors.alarms(node) { alarms = it } }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        if (s == null) { Text("This sensor is not known to the host (any more)."); return@Column }
        Text(s.title, style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold)
        Text(node + " · heard " + ago(s.ageS), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (s.alarm != null) Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(MaterialTheme.colorScheme.errorContainer).padding(14.dp)) {
            Text("⚠ " + s.alarm, fontWeight = FontWeight.SemiBold)
            TextButton(onClick = { Sensors.dismiss(node) }) { Text("Mark as read") }
        }
        else if (s.quiet) Text("No reading for ${s.ageS / 3600} hours. The battery may be flat, or the sensor is out of range.",
            Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(MaterialTheme.colorScheme.tertiaryContainer).padding(14.dp))
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChip(selected = s.favourite, onClick = { Sensors.meta(node, favourite = !s.favourite) }, label = { Text(if (s.favourite) "★ Favourite" else "☆ Favourite") })
            FilterChip(selected = range == 86400L, onClick = { range = 86400L }, label = { Text("24 hours") })
            FilterChip(selected = range == 604800L, onClick = { range = 604800L }, label = { Text("7 days") })
        }
        for (c in s.channels) ChannelCard(node, c, range)
        SCard { Text("Alarms", fontWeight = FontWeight.SemiBold)
            if (alarms.isEmpty()) Text("None so far.", style = MaterialTheme.typography.bodySmall)
            for ((t, raised, text) in alarms.take(10)) Text((if (raised) "⚠ " else "✓ Cleared: ") + text + " · " + time(t * 1000), style = MaterialTheme.typography.bodySmall)
        }
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            FilledTonalButton(onClick = { onEdit(node) }) { Text("Edit setup") }
            OutlinedButton(onClick = { renaming = true }) { Text("Rename") }
            OutlinedButton(onClick = { forgetting = true }) { Text("Forget") }
        }
    }
    if (renaming) TextDialog("Name of this sensor", s?.name ?: "", { Sensors.meta(node, name = it) }, { renaming = false })
    if (forgetting) AlertDialog(onDismissRequest = { forgetting = false }, title = { Text("Forget this sensor?") },
        text = { Text("Its history is removed from this host. The sensor itself is not changed.") },
        confirmButton = { TextButton(onClick = { Sensors.forget(node); forgetting = false; onBack() }) { Text("Forget") } },
        dismissButton = { TextButton(onClick = { forgetting = false }) { Text("Cancel") } })
}

@Composable
private fun ChannelCard(node: String, c: SensorChan, range: Long) {
    var pts by remember(node, c.chan, range) { mutableStateOf<List<Pair<Double, Double?>>>(emptyList()) }
    LaunchedEffect(node, c.chan, range) { Sensors.series(node, c.chan, range) { pts = it } }
    SCard {
        Text(c.name + " · channel " + c.chan, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(fmt(c.value) + " " + c.unit, style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.SemiBold)
        LineChart(pts)
    }
}

// ---------- start, designer, rule, send ----------

@Composable
fun SensorStartScreen(onPick: (String) -> Unit, onImported: () -> Unit) {
    Sensors.catalog.collectAsState().value
    val ctx = LocalContext.current
    val mine by Sensors.list.collectAsState()
    var msg by remember { mutableStateOf("") }
    var copyDialog by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) { Sensors.loadCatalog() }
    val templates = Sensors.templates()
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text("Pick what it should do, test it here, then send it to the node. You can change it later, also over the radio.", color = MaterialTheme.colorScheme.onSurfaceVariant)
        SCard { Text("The first setup of a new board is done on a computer over USB (the sensor takes you as its owner). Changes after that can be sent from here.",
            style = MaterialTheme.typography.bodySmall) }
        STitle("Pick a starting point")
        if (templates.isEmpty()) Text("Loading the starting points from your host … If this stays, the host is too old: update maarse-host.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        else SGroup {
            templates.forEachIndexed { i, t -> GroupRow(t.title, t.text, last = i == templates.lastIndex) { onPick(t.key) } }
        }
        STitle("Or start from something you have")
        SGroup {
            GroupRow("Paste a shared link", "Copy the link first, then press here", last = false) {
                val text = (ctx.getSystemService(android.content.Context.CLIPBOARD_SERVICE) as ClipboardManager).primaryClip?.getItemAt(0)?.coerceToText(ctx)?.toString() ?: ""
                runCatching { Sensors.fromShare(text) }
                    .onSuccess { Sensors.openImported(it); onImported() }
                    .onFailure { msg = if (text.isBlank()) "Nothing is copied yet. Copy the link first." else it.message ?: "This is not a Må:rse sensor link." }
            }
            GroupRow("Copy from one of my sensors", "Start from a setup you already have", last = true) { copyDialog = true }
        }
        if (msg.isNotBlank()) Text(msg, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
        if (copyDialog) {
            val withSetup = (mine ?: emptyList()).filter { it.hasSetup }
            PickDialog("Copy from", withSetup.map { it.node to it.title }.ifEmpty { listOf("" to "None of your sensors has a saved setup yet.") }, null, { node ->
                if (node.isNotEmpty()) Sensors.copyFrom(node) { r ->
                    r.onSuccess { s -> s.put("name", s.optString("name") + " copy"); Sensors.openImported(s); onImported() }
                        .onFailure { msg = it.message ?: "Error" }
                }
            }, { copyDialog = false })
        }
    }
}

private fun readable(setup: JSONObject) = setup.getJSONArray("sources").objs().filter { it.getJSONObject("input").getString("type") != "gpio_out" }
private fun outputs(setup: JSONObject) = setup.getJSONArray("sources").objs().filter { it.getJSONObject("input").getString("type") == "gpio_out" }
private fun srcName(setup: JSONObject, chan: Int) = setup.getJSONArray("sources").objs().firstOrNull { it.getInt("chan") == chan }?.optString("name") ?: "?"
private fun unitOf(setup: JSONObject, chan: Int): String {
    val k = setup.getJSONArray("sources").objs().firstOrNull { it.getInt("chan") == chan }?.optString("kind") ?: return ""
    return Sensors.unitOf(k)
}
private val OPS = listOf("<" to "is below", ">" to "is above", "<=" to "is at most", ">=" to "is at least", "==" to "is exactly", "!=" to "is not")

private fun ruleSummary(setup: JSONObject, r: JSONObject): String {
    val w = r.getJSONObject("when")
    val head = when (w.getString("type")) {
        "every" -> "Every ${w.getInt("secs") / 60} minutes"
        "compare" -> "When ${srcName(setup, w.getInt("chan"))} ${OPS.firstOrNull { it.first == w.getString("op") }?.second ?: ""} ${fmt(w.getDouble("value"))} ${unitOf(setup, w.getInt("chan"))}"
        "change" -> "When ${srcName(setup, w.getInt("chan"))} changes by more than ${fmt(w.getDouble("delta"))}"
        else -> "When ${srcName(setup, w.getInt("chan"))} ${when (w.optString("to")) { "high" -> "reads 1"; "low" -> "reads 0"; else -> "changes" }}"
    }
    val acts = r.getJSONArray("then").objs().joinToString(", ") {
        when (it.getString("type")) { "report" -> "send readings"; "alarm" -> "alarm “${it.optString("text")}”"; "set_out" -> "set ${srcName(setup, it.getInt("chan"))} ${if (it.getBoolean("on")) "on" else "off"}"; else -> "wake every ${it.optInt("secs")} s" }
    }
    return "$head → $acts"
}

@Composable
fun SensorDesignScreen(onRule: (Int) -> Unit, onSend: () -> Unit) {
    Sensors.rev.collectAsState().value
    val d = Sensors.design ?: return
    val setup = d.setup
    var dialog by remember { mutableStateOf<String?>(null) }
    var tplKey by remember { mutableStateOf<String?>(null) }
    var copied by remember { mutableStateOf(false) }
    val ctx = LocalContext.current
    val board = setup.optString("board", "xiao_nrf52840")
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        OutlinedTextField(setup.optString("name"), { setup.put("name", it.take(30)); Sensors.changed() }, Modifier.fillMaxWidth(), label = { Text("Name, e.g. Cellar") }, singleLine = true)
        if (d.note.isNotBlank()) Text(d.note, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        STitle("This sensor")
        SCard {
            Chip(Sensors.boardName(board), 3) { dialog = "board" }
            Sensors.boardNote(board).takeIf { it.isNotBlank() }?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
            Sensors.boardGrove(board).takeIf { it.isNotBlank() }?.let { g ->
                var open by remember { mutableStateOf(false) }
                TextButton(onClick = { open = !open }, contentPadding = androidx.compose.foundation.layout.PaddingValues(0.dp)) { Text((if (open) "▾ " else "▸ ") + "Plugging in Grove sensors", style = MaterialTheme.typography.bodyMedium) }
                if (open) Text(g, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Text("Wake and read every", style = MaterialTheme.typography.bodySmall)
            Chip("${setup.optInt("interval_s", 900) / 60} minutes", 3) { dialog = "interval" }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Sleep between readings (saves battery)", Modifier.weight(1f))
                Switch(setup.optBoolean("sleep", true), { setup.put("sleep", it); Sensors.changed() })
            }
        }
        STitle("Inputs and outputs")
        SCard {
            val chips = mutableMapOf<String, Int>()
            for (s in setup.getJSONArray("sources").objs()) {
                val inp = s.getJSONObject("input")
                val chipKey = if (inp.getString("type") in I2C_TYPES) inp.getString("type") + ":" + inp.optString("device") + ":" + inp.optInt("addr") else null
                val first = chipKey == null || chipKey !in chips
                if (s.getInt("chan") != setup.getJSONArray("sources").getJSONObject(0).getInt("chan")) androidx.compose.foundation.layout.Spacer(Modifier.height(10.dp))
                if (chipKey != null && first) chips[chipKey] = s.getInt("chan")
                Text("#${s.getInt("chan")}  ${s.optString("name")} · " + if (first) inputText(inp, board) else "same chip as #${chips[chipKey]}", style = MaterialTheme.typography.bodyMedium)
                SourceExtras(setup, s, board, first)
                if (inp.getString("type") == "gpio_in") Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("Wake the sensor when this pin changes (a reed switch that closes, a button)", Modifier.weight(1f), style = MaterialTheme.typography.bodySmall)
                    Switch(inp.optBoolean("wake"), { on ->
                        inp.put("wake", on)
                        // A switch that closes when it happens, to ground: no current flows while nothing happens.
                        if (on) { inp.put("invert", true); inp.put("pull", "up") }
                        Sensors.changed()
                    })
                }
            }
            OutlinedButton(onClick = { dialog = "addsource" }) { Text("+ Add sensor or pin") }
        }
        STitle("Who gets it")
        SCard {
            for ((key, label) in listOf("report_to" to "Readings", "alarm_to" to "Alarms")) {
                val to = setup.optJSONObject(key) ?: JSONObject("""{"owners":[0],"channel":false}""").also { setup.put(key, it) }
                val me = (0 until to.getJSONArray("owners").length()).any { to.getJSONArray("owners").getInt(it) == 0 }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("$label to me", Modifier.weight(1f))
                    Switch(me, { on -> to.put("owners", JSONArray().also { a -> if (on) a.put(0) }); Sensors.changed() })
                }
            }
            Text("Owners are people (accounts). Never the public channel.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        STitle("Rules")
        SGroup {
            val rules = setup.getJSONArray("rules").objs()
            rules.forEachIndexed { i, r -> GroupRow(r.optString("name").ifBlank { "Rule ${i + 1}" } + if (r.optBoolean("on", true)) "" else " (off)", ruleSummary(setup, r), last = false) { onRule(i) } }
            val q = setup.optJSONObject("quiet")
            GroupRow("Node went quiet", "On your host · " + if (q?.optBoolean("on", true) == false) "off" else "no reading for ${q?.optInt("min", 120) ?: 120} minutes", last = false) { dialog = "quiet" }
            GroupRow("Start from a template", "Replaces the inputs and rules", last = false) { dialog = "template" }
            GroupRow(if (copied) "Link copied" else "Copy link", "A line of text with this setup, to send to someone", last = false) {
                (ctx.getSystemService(android.content.Context.CLIPBOARD_SERVICE) as ClipboardManager).setPrimaryClip(ClipData.newPlainText("Må:rse sensor", Sensors.shareText(setup)))
                copied = true
            }
            GroupRow("+ Add a rule", null, last = true) {
                val id = (rules.maxOfOrNull { it.optInt("id") } ?: 0) + 1
                val src = readable(setup).firstOrNull()
                val rule = JSONObject("""{"id":$id,"name":"Rule $id","then":[{"type":"alarm","text":"Alarm"}]}""")
                rule.put("when", if (src != null) JSONObject("""{"type":"compare","chan":${src.getInt("chan")},"op":"<","value":10,"clear":10,"count":1}""") else JSONObject("""{"type":"every","secs":900}"""))
                setup.getJSONArray("rules").put(rule); Sensors.changed(); onRule(rules.size)
            }
        }
        Button(onClick = onSend, Modifier.fillMaxWidth()) { Text("Test and send") }
        Text("Rules run on the sensor itself, so alarms work even when your phone is off.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        androidx.compose.foundation.layout.Spacer(Modifier.height(24.dp))
    }
    when (dialog) {
        "board" -> PickDialog("Board", Sensors.boards(), board, { id ->
            if (Sensors.boardAvailable(id)) { setup.put("board", id); d.note = ""; Sensors.changed() }
            else { d.note = Sensors.boardName(id).substringBefore(" (") + " cannot read sensors yet: its firmware is not ready."; Sensors.changed() }
        }, { dialog = null })
        "interval" -> NumberDialog("Wake and read every", (setup.optInt("interval_s", 900) / 60).toDouble(), "minutes", { setup.put("interval_s", maxOf(1, it.toInt()) * 60); Sensors.changed() }, { dialog = null })
        "addsource" -> AddSensorDialog(setup, board, onCustom = { dialog = "custom" }, onDismiss = { dialog = null })
        "custom" -> CustomSensorDialog(setup, null, onDismiss = { dialog = null })
        "quiet" -> QuietDialog(setup, { dialog = null })
        "template" -> PickDialog("Start from a template", Sensors.templates().map { it.key to it.title }, null, { tplKey = it }, { dialog = null })
    }
    tplKey?.let { key ->
        val title = Sensors.templates().firstOrNull { it.key == key }?.title ?: key
        AlertDialog(onDismissRequest = { tplKey = null }, title = { Text(title) },
            text = { Text("Replace the inputs and rules of this sensor with this starting point? The name and who gets the messages stay.") },
            confirmButton = { TextButton(onClick = {
                val n = Sensors.newSetup(key)
                setup.put("sources", n.get("sources")); setup.put("rules", n.get("rules")); setup.put("interval_s", n.optInt("interval_s", 900))
                if (setup.optString("name").isBlank()) setup.put("name", n.optString("name"))
                Sensors.changed(); tplKey = null
            }) { Text("Replace") } },
            dismissButton = { TextButton(onClick = { tplKey = null }) { Text("Cancel") } })
    }
}

/** The host rule "node went quiet": on or off, and after how many minutes without a reading. */
@Composable
private fun QuietDialog(setup: JSONObject, onDismiss: () -> Unit) {
    val q = setup.optJSONObject("quiet") ?: JSONObject("""{"on":true,"min":120}""")
    var on by remember { mutableStateOf(q.optBoolean("on", true)) }
    var min by remember { mutableStateOf(q.optInt("min", 120).toString()) }
    AlertDialog(onDismissRequest = onDismiss, title = { Text("Node went quiet") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text("Your phone or PC host shows the sensor as quiet when it sends nothing for a while. It catches a dead battery, and needs the host to be running.", style = MaterialTheme.typography.bodySmall)
                Row(verticalAlignment = Alignment.CenterVertically) { Text("Rule is on", Modifier.weight(1f)); Switch(on, { on = it }) }
                OutlinedTextField(min, { min = it.filter { c -> c.isDigit() }.take(5) }, singleLine = true, label = { Text("No reading for") }, suffix = { Text("minutes") },
                    enabled = on, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number))
            }
        },
        confirmButton = { TextButton(onClick = {
            setup.put("quiet", JSONObject().put("on", on).put("min", maxOf(5, min.toIntOrNull() ?: 120)))
            Sensors.changed(); onDismiss()
        }) { Text("OK") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } })
}

// ----- adding sensors (the choices come from the host's catalog) -----

private val I2C_TYPES = setOf("bme280", "i2c", "i2c_custom")

private fun freePins(setup: JSONObject, board: String): List<Pair<Int, String>> {
    val srcs = setup.getJSONArray("sources").objs()
    val used = srcs.mapNotNull { it.getJSONObject("input").optInt("pin", -1).takeIf { p -> p >= 0 } }.toSet()
    val i2c = if (srcs.any { it.getJSONObject("input").getString("type") in I2C_TYPES }) Sensors.i2cPins(board)?.toSet() ?: emptySet() else emptySet()
    return Sensors.pins(board).filter { it.first !in used && it.first !in i2c }
}

private fun nextChan(setup: JSONObject): Int { var c = 1; val a = setup.getJSONArray("sources").objs(); while (a.any { it.getInt("chan") == c }) c++; return c }

/** A free analogue connection: a pin of the board, else a channel of an ADS1115 on I2C. */
private fun freeAnalog(setup: JSONObject, board: String): JSONObject? {
    val srcs = setup.getJSONArray("sources").objs().map { it.getJSONObject("input") }
    for ((p, _) in Sensors.adcPins(board)) if (srcs.none { it.getString("type") == "adc" && it.optInt("pin") == p }) return JSONObject("""{"type":"adc","pin":$p,"mul":1}""")
    if (Sensors.i2cPins(board) != null) for (k in 0..3) if (srcs.none { it.getString("type") == "i2c" && it.optString("device") == "ads1115" && it.optInt("what") == k }) return JSONObject("""{"type":"i2c","device":"ads1115","addr":72,"what":$k}""")
    return null
}

private fun hex(n: Int) = "0x" + n.toString(16)

/** The plain title of a sensor; when two different chips share it, the chip name tells them apart. */
private fun sensorLabel(x: JSONObject): String {
    val t = x.optString("title").ifBlank { x.optString("name") }
    val same = Sensors.catalogSensors().count { it.optString("bus") == "i2c" && it.optString("title").ifBlank { it.optString("name") } == t } > 1
    return if (same && x.optString("bus") == "i2c") t + " (" + x.optString("name") + ")" else t
}

private fun inputText(inp: JSONObject, board: String): String = when (inp.getString("type")) {
    "i2c" -> (Sensors.sensorByDevice(inp.optString("device"))?.let { sensorLabel(it) } ?: inp.optString("device")) + " · sensor port"
    "i2c_custom" -> "your own sensor · sensor port"
    "bme280" -> "Temperature, humidity and air pressure · sensor port"
    "battery", "chip_temp" -> "built in"
    "adc" -> "analogue pin " + (Sensors.adcPins(board).firstOrNull { it.first == inp.optInt("pin") }?.second ?: "${inp.optInt("pin")}")
    else -> inp.getString("type").replace('_', ' ')
}

@Composable
private fun AddSensorDialog(setup: JSONObject, board: String, onCustom: () -> Unit, onDismiss: () -> Unit) {
    var q by remember { mutableStateOf("") }
    val srcs = setup.getJSONArray("sources")
    val hasI2c = Sensors.i2cPins(board) != null
    val builtin = buildList {
        val types = srcs.objs().map { it.getJSONObject("input").getString("type") }
        if ("battery" !in types) add(JSONObject("""{"id":"battery","name":"Battery voltage","group":"Built in","bus":"builtin","note":""}"""))
        if ("chip_temp" !in types) add(JSONObject("""{"id":"chip_temp","name":"Chip temperature","group":"Built in","bus":"builtin","note":""}"""))
    }
    val own = JSONObject("""{"id":"custom_i2c","name":"Build your own (I²C)","group":"Your own","bus":"custom","note":"A chip that is not in the list: describe how to talk to it.","grove":""}""")
    var more by remember { mutableStateOf(false) }
    val simple = builtin + Sensors.catalogSensors().filter { !it.optBoolean("advanced") }
    val adv = Sensors.catalogSensors().filter { it.optBoolean("advanced") } + own
    fun label(x: JSONObject) = sensorLabel(x)
    fun matches(x: JSONObject) = q.isBlank() || (label(x) + " " + x.optString("name") + " " + x.optString("grove") + " " + x.optString("note")).contains(q.trim(), ignoreCase = true)
    fun sub(x: JSONObject): String = listOf(x.optString("grove").ifBlank { if (x.getString("bus") == "i2c") "Chip " + x.optString("name") else "" }, x.optString("note")).filter { it.isNotBlank() }.joinToString(" · ")
    fun usable(x: JSONObject) = when (x.getString("bus")) {
        "i2c", "custom" -> hasI2c
        "analog" -> Sensors.adcPins(board).isNotEmpty() || hasI2c
        "builtin" -> true
        else -> freePins(setup, board).isNotEmpty()
    }
    fun why(x: JSONObject) = when (x.getString("bus")) { "i2c", "custom" -> "This board has no sensor port."; "analog" -> "This board cannot read analogue sensors."; else -> "No free pin: remove one first." }
    val update = Sensors.catalog.collectAsState().value?.optJSONObject("update")
    var note by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    AlertDialog(onDismissRequest = onDismiss, title = { Text("Add a sensor or pin") }, confirmButton = {}, dismissButton = { TextButton(onClick = onDismiss) { Text("Close") } },
        text = {
            Column(Modifier.heightIn(max = 520.dp)) {
                OutlinedTextField(q, { q = it }, Modifier.fillMaxWidth(), singleLine = true, label = { Text("Search") }, placeholder = { Text("SHT31, soil, door, CO2") })
                Column(Modifier.weight(1f, fill = false).verticalScroll(rememberScrollState())) {
                    @Composable fun Items(items: List<JSONObject>) {
                        for (g in items.map { it.optString("group") }.distinct()) {
                            val rows = items.filter { it.optString("group") == g && matches(it) }
                            if (rows.isEmpty()) continue
                            Text(g, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary, modifier = Modifier.padding(top = 10.dp, bottom = 2.dp))
                            for (x in rows) {
                                val ok = usable(x)
                                Column(Modifier.fillMaxWidth().clickable(enabled = ok) { onDismiss(); if (x.getString("bus") == "custom") onCustom() else addSensor(setup, board, x) }.padding(vertical = 6.dp)) {
                                    Text(label(x), color = if (ok) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurfaceVariant)
                                    val line = if (ok) sub(x) else why(x)
                                    if (line.isNotBlank()) Text(line, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                                }
                            }
                        }
                    }
                    Items(simple)
                    val advRows = adv.filter { matches(it) }
                    if (advRows.isNotEmpty()) {
                        val shown = more || q.isNotBlank()
                        TextButton(onClick = { more = !more }, contentPadding = androidx.compose.foundation.layout.PaddingValues(0.dp)) { Text((if (shown) "▾ " else "▸ ") + "More, for tinkerers", style = MaterialTheme.typography.bodyMedium) }
                        if (shown) Items(advRows)
                    }
                    if (simple.none { matches(it) } && advRows.isEmpty()) Text("Nothing matches. Try another word, like the name on the box.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(top = 12.dp))
                }
                if (update != null) {
                    val day = { t: Long -> java.text.DateFormat.getDateInstance().format(java.util.Date(t * 1000)) }
                    val ver = update.optLong("version")
                    Text("Sensor list from ${day(ver)} (" + (if (ver == update.optLong("built_in_version")) "came with this program" else "updated from GitHub") + ")" +
                        update.optString("error").takeIf { it.isNotBlank() }?.let { " · $it" }.orEmpty() + if (note.isNotBlank()) " · $note" else "",
                        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(top = 8.dp))
                    if (update.optBoolean("can_update")) Row(verticalAlignment = Alignment.CenterVertically) {
                        TextButton(enabled = !busy, onClick = { busy = true; note = "Checking GitHub …"; Sensors.updateCatalog { note = it; busy = false } }) { Text("Check for updates") }
                        Checkbox(update.optBoolean("auto", true), { Sensors.setCatalogAuto(it) }); Text("automatically", style = MaterialTheme.typography.bodySmall)
                    } else Text("This program cannot check for newer lists.", style = MaterialTheme.typography.bodySmall)
                }
            }
        })
}

private fun addSensor(setup: JSONObject, board: String, x: JSONObject) {
    val arr = setup.getJSONArray("sources")
    val room = { n: Int -> arr.length() + n <= 8 }
    fun add(kind: String, name: String, input: JSONObject, preset: String? = null, map: JSONObject? = null) {
        val o = JSONObject().put("chan", nextChan(setup)).put("kind", kind).put("name", name).put("input", input)
        if (preset != null) o.put("preset", preset)
        if (map != null) o.put("map", map)
        arr.put(o)
    }
    when (x.getString("bus")) {
        "builtin" -> if (room(1)) { if (x.getString("id") == "battery") add("voltage", "Battery", JSONObject("""{"type":"battery"}""")) else add("temperature", "Chip temperature", JSONObject("""{"type":"chip_temp"}""")) }
        "i2c" -> { val ch = x.getJSONArray("channels").objs(); if (room(ch.size)) for (c in ch) add(c.getString("kind"), c.getString("name"), JSONObject().put("type", "i2c").put("device", x.getString("device")).put("addr", x.getJSONArray("addrs").getInt(0)).put("what", c.getInt("what")), x.getString("id")) }
        "analog" -> { val inp = freeAnalog(setup, board); if (inp != null && room(1)) add(x.getString("kind"), x.getString("name"), inp, x.getString("id"), x.optJSONObject("map")?.let { JSONObject(it.toString()) }) }
        else -> {
            val free = freePins(setup, board)
            if (free.isNotEmpty() && room(1)) {
                val inp = if (x.getString("bus") == "digital_in") JSONObject().put("type", "gpio_in").put("pin", free[0].first).put("pull", x.optString("pull", "none")).put("invert", x.optBoolean("invert"))
                          else JSONObject().put("type", "gpio_out").put("pin", free[0].first).put("active_low", x.optBoolean("active_low"))
                add("digital", x.getString("name"), inp, x.getString("id"))
            }
        }
    }
    Sensors.changed()
}

/** Pin, analogue connection, calibration and I²C address of one source. */
@Composable
private fun SourceExtras(setup: JSONObject, s: JSONObject, board: String, first: Boolean = true) {
    val inp = s.getJSONObject("input")
    val type = inp.getString("type")
    val chan = s.getInt("chan")
    var dlg by remember(chan) { mutableStateOf<String?>(null) }
    val ads = type == "i2c" && inp.optString("device") == "ads1115"
    val analog = type == "adc" || ads
    val srcs = setup.getJSONArray("sources").objs()
    val unit = Sensors.unitOf(s.optString("kind"))
    val addrCount = Sensors.sensorByDevice(inp.optString("device"))?.optJSONArray("addrs")?.length() ?: 0
    val pinChoice = (type == "gpio_in" || type == "gpio_out") && freePins(setup, board).size + (if (Sensors.pins(board).any { it.first == inp.optInt("pin") }) 1 else 0) > 1
    val hasExtras = (type == "i2c" && first && addrCount > 1) || pinChoice || analog || type == "i2c_custom"
    var open by remember(chan) { mutableStateOf(false) }
    // What a person needs to know in plain words, without opening the details.
    if (ads && s.optString("preset").let { it.isNotBlank() && it != "ads1115" })
        Text("This board has no analogue input. Plug this sensor into a Grove ADC module (ADS1115), and the module into the sensor port.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
    if (hasExtras) TextButton(onClick = { open = !open }, contentPadding = androidx.compose.foundation.layout.PaddingValues(0.dp)) { Text((if (open) "▾ " else "▸ ") + "Details", style = MaterialTheme.typography.bodyMedium) }
    if (open) {
    FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        if (type == "i2c") {
            val addrs = Sensors.sensorByDevice(inp.optString("device"))?.optJSONArray("addrs")
            if (first && addrs != null && addrs.length() > 1) Chip("I²C address ${hex(inp.optInt("addr"))}", 3) { dlg = "addr" }
        }
        if (type == "gpio_in" || type == "gpio_out") {
            val free = freePins(setup, board)
            val here = Sensors.pins(board).firstOrNull { it.first == inp.optInt("pin") }
            if (free.size + (if (here != null) 1 else 0) > 1) Chip("Pin " + (here?.second ?: inp.optInt("pin")), 3) { dlg = "pin" }
        }
        if (analog) Chip("Connected to " + if (ads) "ADS1115 A${inp.optInt("what")}" else (Sensors.adcPins(board).firstOrNull { it.first == inp.optInt("pin") }?.second ?: "pin"), 3) { dlg = "conn" }
        if (type == "i2c_custom") Chip("Edit how it is read", 3) { dlg = "custom" }
        if (analog && !s.has("map")) Chip("Show as a level (0 to 100 %)", 3) { s.put("kind", "percent"); s.put("map", JSONObject("""{"in":[0,3000],"out":[0,100]}""")); Sensors.changed() }
    }
    if (s.has("map")) {
        val m = s.getJSONObject("map")
        Text("Calibration: reading in mV, then what to show", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            for (k in 0..1) Chip("${fmt(m.getJSONArray("in").getDouble(k))} mV → ${fmt(m.getJSONArray("out").getDouble(k))} $unit", 3) { dlg = "cal$k" }
        }
        Sensors.sensorById(s.optString("preset"))?.optString("calibrate")?.takeIf { it.isNotBlank() }?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
    }
    }
    when {
        dlg == "pin" -> {
            val here = Sensors.pins(board).firstOrNull { it.first == inp.optInt("pin") }
            val opts = (listOfNotNull(here) + freePins(setup, board)).distinctBy { it.first }
            PickDialog("Pin", opts.map { it.first.toString() to it.second }, inp.optInt("pin").toString(), { inp.put("pin", it.toInt()); Sensors.changed() }, { dlg = null })
        }
        dlg == "conn" -> {
            val opts = mutableListOf<Pair<String, String>>()
            for ((p, l) in Sensors.adcPins(board)) if (srcs.none { it !== s && it.getJSONObject("input").getString("type") == "adc" && it.getJSONObject("input").optInt("pin") == p }) opts.add("adc,$p" to "$l (on the board)")
            if (Sensors.i2cPins(board) != null) for (k in 0..3) if (srcs.none { it !== s && it.getJSONObject("input").optString("device") == "ads1115" && it.getJSONObject("input").optInt("what") == k }) opts.add("ads,$k" to "ADS1115 input A$k (I²C)")
            PickDialog("Connected to", opts, if (ads) "ads,${inp.optInt("what")}" else "adc,${inp.optInt("pin")}", { v ->
                val (k, n) = v.split(",")
                s.put("input", if (k == "adc") JSONObject("""{"type":"adc","pin":$n,"mul":1}""") else JSONObject("""{"type":"i2c","device":"ads1115","addr":72,"what":$n}"""))
                Sensors.changed()
            }, { dlg = null })
        }
        dlg == "addr" -> {
            val addrs = Sensors.sensorByDevice(inp.optString("device"))!!.getJSONArray("addrs")
            PickDialog("I²C address", (0 until addrs.length()).map { addrs.getInt(it).toString() to hex(addrs.getInt(it)) }, inp.optInt("addr").toString(), { v ->
                val old = inp.optInt("addr")
                for (o in srcs) { val i = o.getJSONObject("input"); if (i.getString("type") == "i2c" && i.optString("device") == inp.optString("device") && i.optInt("addr") == old) i.put("addr", v.toInt()) }
                Sensors.changed()
            }, { dlg = null })
        }
        dlg?.startsWith("cal") == true -> {
            val k = dlg!!.removePrefix("cal").toInt()
            val m = s.getJSONObject("map")
            CalibrateDialog(m.getJSONArray("in").getDouble(k), m.getJSONArray("out").getDouble(k), unit, { a, b -> m.getJSONArray("in").put(k, a); m.getJSONArray("out").put(k, b); Sensors.changed() }, { dlg = null })
        }
        dlg == "custom" -> CustomSensorDialog(setup, s, onDismiss = { dlg = null })
    }
}

@Composable
private fun CalibrateDialog(inMv: Double, out: Double, unit: String, onOk: (Double, Double) -> Unit, onDismiss: () -> Unit) {
    var a by remember { mutableStateOf(fmt(inMv)) }
    var b by remember { mutableStateOf(fmt(out)) }
    AlertDialog(onDismissRequest = onDismiss, title = { Text("Calibration point") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Measure the sensor in this state (the sensor's page shows the millivolts) and say what it should read.", style = MaterialTheme.typography.bodySmall)
                OutlinedTextField(a, { a = it }, singleLine = true, label = { Text("Reading") }, suffix = { Text("mV") }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal))
                OutlinedTextField(b, { b = it }, singleLine = true, label = { Text("Shown as") }, suffix = { Text(unit) }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal))
            }
        },
        confirmButton = { TextButton(onClick = { val x = a.replace(',', '.').toDoubleOrNull(); val y = b.replace(',', '.').toDoubleOrNull(); if (x != null && y != null) { onOk(x, y); onDismiss() } }) { Text("OK") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } })
}

/** A chip the catalogue does not have: the steps and the bits, as a form. */
@Composable
private fun CustomSensorDialog(setup: JSONObject, src: JSONObject?, onDismiss: () -> Unit) {
    val old = src?.getJSONObject("input")
    val f = old?.optJSONObject("field")
    val kinds = Sensors.kindKeys()
    var name by remember { mutableStateOf(src?.optString("name") ?: "My sensor") }
    var kind by remember { mutableStateOf(src?.optString("kind") ?: "custom") }
    var addr by remember { mutableStateOf(old?.optInt("addr")?.toString(16) ?: "40") }
    var init by remember { mutableStateOf(old?.optJSONArray("init")?.let { a -> (0 until a.length()).joinToString("\n") { a.getString(it) } } ?: "") }
    var trigger by remember { mutableStateOf(old?.optString("trigger") ?: "") }
    var wait by remember { mutableStateOf((old?.optInt("wait_ms") ?: 0).toString()) }
    var ptr by remember { mutableStateOf(old?.optString("ptr") ?: "00") }
    var len by remember { mutableStateOf((old?.optInt("read_len") ?: 2).toString()) }
    var crc by remember { mutableStateOf(old?.optBoolean("crc") ?: false) }
    var bitOff by remember { mutableStateOf((f?.optInt("bit_off") ?: 0).toString()) }
    var bits by remember { mutableStateOf((f?.optInt("bits") ?: 16).toString()) }
    var signed by remember { mutableStateOf(f?.optBoolean("signed") ?: false) }
    var little by remember { mutableStateOf(f?.optBoolean("little") ?: false) }
    var mul by remember { mutableStateOf((f?.optInt("mul") ?: 1).toString()) }
    var div by remember { mutableStateOf((f?.optInt("div") ?: 1).toString()) }
    var add by remember { mutableStateOf((f?.optInt("add") ?: 0).toString()) }
    var kindPick by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf("") }
    val max = Sensors.catalog.value?.optJSONObject("custom_i2c")?.optInt("max", 2) ?: 2
    @Composable fun field(label: String, v: String, set: (String) -> Unit, help: String? = null, number: Boolean = false) {
        OutlinedTextField(v, set, Modifier.fillMaxWidth(), label = { Text(label) }, supportingText = help?.let { { Text(it) } }, singleLine = number,
            keyboardOptions = if (number) KeyboardOptions(keyboardType = KeyboardType.Number) else KeyboardOptions.Default)
    }
    AlertDialog(onDismissRequest = onDismiss, title = { Text("Your own I²C sensor") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Most small I²C chips work the same way: send a few bytes to start a measurement, wait, ask for a register and read some bytes. The data sheet lists these. The result is in the unit of the kind: 0.01 °C for temperature (2500 is 25.00 °C), 0.01 % for humidity, whole numbers for the rest.", style = MaterialTheme.typography.bodySmall)
                OutlinedButton(onClick = {
                    name = "Temperature"; kind = "temperature"; addr = "44"; init = ""; trigger = "24 00"; wait = "20"; ptr = ""; len = "6"; crc = true
                    bitOff = "0"; bits = "16"; signed = false; little = false; mul = "17500"; div = "65535"; add = "-4500"
                }) { Text("Fill in an example (SHT31 temperature)") }
                field("Name", name, { name = it.take(30) })
                OutlinedButton(onClick = { kindPick = true }) { Text("Measures: $kind") }
                field("I²C address (hex)", addr, { addr = it.take(2) }, "Two hex digits from the data sheet, like 44 or 76.")
                field("Set-up commands", init, { init = it }, "One per line, hex, like 2D 08. Sent every time before measuring. Up to 4 bytes each.")
                field("Start measurement (hex)", trigger, { trigger = it }, "Bytes that make the chip measure, like 24 00. Empty if it measures by itself.")
                field("Wait (ms)", wait, { wait = it }, "How long the measurement takes. Up to 10 000.", number = true)
                field("Register to read (hex)", ptr, { ptr = it }, "Written before reading, like 05. Empty: read straight away.")
                field("Bytes to read", len, { len = it }, "1 to 16. A chip with check bytes sends 3 bytes for every 2 of data.", number = true)
                Row(verticalAlignment = Alignment.CenterVertically) { Checkbox(crc, { crc = it }); Text("Check byte after every two bytes (Sensirion CRC-8)") }
                Text("Which bits are the number", style = MaterialTheme.typography.labelLarge)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(bitOff, { bitOff = it }, Modifier.weight(1f), label = { Text("Starts at bit") }, singleLine = true, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number))
                    OutlinedTextField(bits, { bits = it }, Modifier.weight(1f), label = { Text("Length (1-32)") }, singleLine = true, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number))
                }
                Row(verticalAlignment = Alignment.CenterVertically) { Checkbox(signed, { signed = it }); Text("Can be negative") }
                Row(verticalAlignment = Alignment.CenterVertically) { Checkbox(little, { little = it }); Text("Least significant byte first (whole bytes)") }
                Text("value = raw × a ÷ b + c", style = MaterialTheme.typography.labelLarge)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(mul, { mul = it }, Modifier.weight(1f), label = { Text("a") }, singleLine = true)
                    OutlinedTextField(div, { div = it }, Modifier.weight(1f), label = { Text("b") }, singleLine = true)
                    OutlinedTextField(add, { add = it }, Modifier.weight(1f), label = { Text("c") }, singleLine = true)
                }
                if (error.isNotBlank()) Text(error, color = MaterialTheme.colorScheme.error)
            }
        },
        confirmButton = {
            TextButton(onClick = {
                val a = addr.trim().toIntOrNull(16)
                val arr = setup.getJSONArray("sources")
                when {
                    a == null || a !in 1..119 -> error = "The address is two hex digits, 01 to 77."
                    src == null && arr.objs().count { it.getJSONObject("input").getString("type") == "i2c_custom" } >= max -> error = "At most $max of your own I²C chips."
                    src == null && arr.length() >= 8 -> error = "A sensor can have at most 8 inputs and outputs."
                    else -> {
                        val input = JSONObject().put("type", "i2c_custom").put("addr", a)
                            .put("init", JSONArray(init.split('\n', ';').map { it.trim() }.filter { it.isNotEmpty() }))
                            .put("trigger", trigger.trim()).put("wait_ms", wait.toIntOrNull() ?: 0).put("ptr", ptr.trim()).put("read_len", len.toIntOrNull() ?: 0).put("crc", crc)
                            .put("field", JSONObject().put("bit_off", bitOff.toIntOrNull() ?: 0).put("bits", bits.toIntOrNull() ?: 0).put("signed", signed).put("little", little)
                                .put("mul", mul.toIntOrNull() ?: 1).put("div", div.toIntOrNull() ?: 1).put("add", add.toIntOrNull() ?: 0))
                        if (src != null) { src.put("input", input).put("name", name).put("kind", kind) }
                        else arr.put(JSONObject().put("chan", nextChan(setup)).put("kind", kind).put("name", name).put("input", input))
                        Sensors.changed(); onDismiss()
                    }
                }
            }) { Text(if (src == null) "Add" else "Save") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } })
    if (kindPick) PickDialog("Measures", kinds.map { it to it }, kind, { kind = it }, { kindPick = false })
}

/** Change one number with big buttons, quick values and (for a threshold) the level where it clears again. */
@Composable
private fun ThresholdSheet(title: String, value: Double, unit: String, clear: Double, clearAbove: Boolean, onOk: (Double, Double) -> Unit, onDismiss: () -> Unit) {
    val step = if (unit == "°C") 0.5 else 1.0
    var v by remember { mutableStateOf(value) }
    val gap = kotlin.math.abs(clear - value).takeIf { it > 0 } ?: 2.0
    var auto by remember { mutableStateOf(clear != value) }
    fun snap(x: Double) = Math.round(x / step) * step
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.padding(horizontal = 20.dp).padding(bottom = 24.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Text(title, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedButton(onClick = { v = snap(v - step) }, Modifier.width(64.dp).height(56.dp)) { Text("−", fontSize = 24.sp) }
                Text(fmt(v) + if (unit.isNotBlank()) " $unit" else "", Modifier.weight(1f), textAlign = TextAlign.Center, fontSize = 34.sp, fontWeight = FontWeight.Bold, fontFamily = FontFamily.Monospace)
                OutlinedButton(onClick = { v = snap(v + step) }, Modifier.width(64.dp).height(56.dp)) { Text("+", fontSize = 24.sp) }
            }
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                for (d in listOf(-5.0, -2.0, 0.0, 2.0, 5.0)) { val q = Math.round(value) + d; FilterChip(selected = v == q, onClick = { v = q }, label = { Text(fmt(q)) }) }
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text("Clear ${fmt(gap)} ${unit} ${if (clearAbove) "above" else "below"}".replace("  ", " "), fontWeight = FontWeight.SemiBold)
                    Text("So it does not send again and again", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                Switch(auto, { auto = it })
            }
            Button(onClick = { onOk(v, if (auto) v + (if (clearAbove) gap else -gap) else v); onDismiss() }, Modifier.fillMaxWidth().height(52.dp)) { Text("Set threshold") }
        }
    }
}

@Composable
fun SensorRuleScreen(index: Int, onBack: () -> Unit) {
    Sensors.rev.collectAsState().value
    val d = Sensors.design ?: return
    val setup = d.setup
    val rules = setup.getJSONArray("rules")
    if (index >= rules.length()) { onBack(); return }
    val r = rules.getJSONObject(index)
    val w = r.getJSONObject("when")
    var dialog by remember { mutableStateOf<String?>(null) }
    var actionIdx by remember { mutableStateOf(0) }
    val readableSrc = readable(setup).map { it.getInt("chan").toString() to it.optString("name") }
    fun set(f: () -> Unit) { f(); Sensors.changed() }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text((setup.optString("name").ifBlank { "Sensor" }) + " · rule ${index + 1} of ${rules.length()} · " + (if (d.compiled?.optBoolean("ok") == true) "fits on the sensor" else "being checked"),
                Modifier.weight(1f), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            TextButton(onClick = onBack) { Text("Done", fontWeight = FontWeight.Bold) }
        }
        OutlinedTextField(r.optString("name"), { r.put("name", it.take(30)); Sensors.changed() }, Modifier.fillMaxWidth(), label = { Text("Rule name") }, singleLine = true)
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text("Rule is on", fontWeight = FontWeight.SemiBold)
                Text("A rule that is off is not sent to the sensor.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Switch(r.optBoolean("on", true), { set { r.put("on", it) } })
        }
        SCard {
            Kw("When")
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Chip(when (w.getString("type")) { "every" -> "every …"; "compare" -> "a value …"; "change" -> "a value changes by …"; else -> "a pin …" }, 4) { dialog = "trigger" }
                when (w.getString("type")) {
                    "every" -> Chip("${w.getInt("secs") / 60} minutes", 4) { dialog = "secs" }
                    "compare" -> {
                        Chip(srcName(setup, w.getInt("chan")), 0) { dialog = "src" }
                        Chip(OPS.first { it.first == w.getString("op") }.second, 1) { dialog = "op" }
                        Chip(fmt(w.getDouble("value")) + " " + unitOf(setup, w.getInt("chan")), 1) { dialog = "value" }
                        Chip("for ${w.optInt("count", 1)} readings", 1) { dialog = "count" }
                    }
                    "change" -> { Chip(srcName(setup, w.getInt("chan")), 0) { dialog = "src" }; Chip("by more than " + fmt(w.getDouble("delta")), 1) { dialog = "delta" } }
                    else -> { Chip(srcName(setup, w.getInt("chan")), 0) { dialog = "src" }; Chip(when (w.optString("to")) { "high" -> "reads 1 (door open)"; "low" -> "reads 0"; else -> "changes" }, 1) { dialog = "edge" } }
                }
            }
            if (w.getString("type") == "compare") {
                Kw("Clear")
                Chip("back to normal at " + fmt(w.getDouble("clear")) + " " + unitOf(setup, w.getInt("chan")), 1) { dialog = "clear" }
            }
            val acts = r.getJSONArray("then")
            for (i in 0 until acts.length()) {
                val a = acts.getJSONObject(i)
                Kw(if (i == 0) "Then" else "and")
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    when (a.getString("type")) {
                        "report" -> Chip("send readings", 2) {}
                        "alarm" -> { Chip("send alarm", 2) {}; Chip("“" + a.optString("text") + "”", 2) { actionIdx = i; dialog = "text" } }
                        "set_out" -> { Chip("set " + srcName(setup, a.getInt("chan")) + if (a.getBoolean("on")) " on" else " off", 2) { actionIdx = i; dialog = "out" } }
                        else -> Chip("wake every ${a.optInt("secs")} s", 2) { actionIdx = i; dialog = "interval" }
                    }
                    Chip("remove", 3) { set { val n = JSONArray(); for (j in 0 until acts.length()) if (j != i) n.put(acts.get(j)); r.put("then", n) } }
                }
            }
            OutlinedButton(onClick = { dialog = "addaction" }) { Text("+ Add a step") }
            if (r.getJSONArray("then").objs().any { it.getString("type") == "alarm" }) {
                Kw("Limit")
                Chip("at most one alarm every ${r.optInt("limit_min", 0)} minutes", 1) { dialog = "limit" }
            }
        }
        OutlinedButton(onClick = { set { val n = JSONArray(); for (j in 0 until rules.length()) if (j != index) n.put(rules.get(j)); setup.put("rules", n) }; onBack() }) { Text("Remove this rule") }
        androidx.compose.foundation.layout.Spacer(Modifier.height(24.dp))
    }
    when (dialog) {
        "trigger" -> PickDialog("When", listOf("every" to "every …", "compare" to "a value …", "change" to "a value changes by …", "edge" to "a pin …"), w.getString("type"), { v ->
            val src = (if (v == "edge") readable(setup).firstOrNull { it.optString("kind") == "digital" } else readable(setup).firstOrNull())?.getInt("chan")
            set { r.put("when", when {
                v == "every" -> JSONObject("""{"type":"every","secs":900}""")
                src == null -> JSONObject("""{"type":"every","secs":900}""")
                v == "compare" -> JSONObject("""{"type":"compare","chan":$src,"op":"<","value":10,"clear":10,"count":1}""")
                v == "change" -> JSONObject("""{"type":"change","chan":$src,"delta":1}""")
                else -> JSONObject("""{"type":"edge","chan":$src,"to":"high"}""") }) }
        }, { dialog = null })
        "secs" -> NumberDialog("Every", (w.getInt("secs") / 60).toDouble(), "minutes", { set { w.put("secs", maxOf(1, it.toInt()) * 60) } }, { dialog = null })
        "src" -> PickDialog("Which value", readableSrc, w.getInt("chan").toString(), { set { w.put("chan", it.toInt()) } }, { dialog = null })
        "op" -> PickDialog("Compare", OPS, w.getString("op"), { set { w.put("op", it) } }, { dialog = null })
        "value" -> ThresholdSheet("Threshold", w.getDouble("value"), unitOf(setup, w.getInt("chan")), w.getDouble("clear"), w.getString("op").startsWith("<"),
            { nv, nc -> set { w.put("value", nv); w.put("clear", nc) } }, { dialog = null })
        "clear" -> NumberDialog("Back to normal at", w.getDouble("clear"), unitOf(setup, w.getInt("chan")), { set { w.put("clear", it) } }, { dialog = null })
        "count" -> NumberDialog("For how many readings", w.optInt("count", 1).toDouble(), "readings", { set { w.put("count", maxOf(1, it.toInt())) } }, { dialog = null })
        "delta" -> NumberDialog("Change by more than", w.getDouble("delta"), unitOf(setup, w.getInt("chan")), { set { w.put("delta", kotlin.math.abs(it)) } }, { dialog = null })
        "edge" -> PickDialog("The pin", listOf("high" to "reads 1 (door open)", "low" to "reads 0", "any" to "changes"), w.optString("to"), { set { w.put("to", it) } }, { dialog = null })
        "limit" -> NumberDialog("At most one alarm every", r.optInt("limit_min", 0).toDouble(), "minutes (0 = no limit)", { set { r.put("limit_min", maxOf(0, it.toInt())) } }, { dialog = null })
        "text" -> TextDialog("Alarm message", r.getJSONArray("then").getJSONObject(actionIdx).optString("text"), { set { r.getJSONArray("then").getJSONObject(actionIdx).put("text", it) } }, { dialog = null })
        "interval" -> NumberDialog("Wake every", r.getJSONArray("then").getJSONObject(actionIdx).optInt("secs", 60).toDouble(), "seconds", { set { r.getJSONArray("then").getJSONObject(actionIdx).put("secs", maxOf(30, it.toInt())) } }, { dialog = null })
        "out" -> PickDialog("Set output", outputs(setup).flatMap { listOf("${it.getInt("chan")}:1" to "${it.optString("name")} on", "${it.getInt("chan")}:0" to "${it.optString("name")} off") }, null, { v ->
            set { val a = r.getJSONArray("then").getJSONObject(actionIdx); a.put("chan", v.substringBefore(':').toInt()); a.put("on", v.endsWith(":1")) } }, { dialog = null })
        "addaction" -> PickDialog("Add a step", buildList {
            add("report" to "Send readings"); add("alarm" to "Send an alarm")
            if (outputs(setup).isNotEmpty()) add("set_out" to "Set an output"); add("interval" to "Change the interval") }, null, { v ->
            set { r.getJSONArray("then").put(when (v) {
                "report" -> JSONObject("""{"type":"report"}""")
                "alarm" -> JSONObject("""{"type":"alarm","text":"Alarm"}""")
                "set_out" -> JSONObject("""{"type":"set_out","chan":${outputs(setup)[0].getInt("chan")},"on":true}""")
                else -> JSONObject("""{"type":"interval","secs":60}""") }) } }, { dialog = null })
    }
}

@Composable
private fun Meter(label: String, text: String, frac: Float) {
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Row { Text(label, Modifier.weight(1f)); Text(text, fontFamily = FontFamily.Monospace) }
        LinearProgressIndicator(progress = { frac.coerceIn(0f, 1f) }, Modifier.fillMaxWidth().height(8.dp).clip(RoundedCornerShape(99.dp)),
            color = if (frac > .9f) Amber else MaterialTheme.colorScheme.primary, trackColor = MaterialTheme.colorScheme.surfaceContainerHigh)
    }
}

@Composable
fun SensorSendScreen() {
    Sensors.rev.collectAsState().value
    val sent by Sensors.status.collectAsState()
    val d = Sensors.design ?: return
    val k = d.compiled
    var result by remember { mutableStateOf("") }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        SCard {
            Text("Pretend the sensor sees", fontWeight = FontWeight.SemiBold)
            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                for ((key, label) in listOf("cold_night" to "A cold night", "door" to "Door opens", "silent" to "Sensor stops"))
                    FilterChip(selected = d.scenario == key, onClick = { d.scenario = key; Sensors.changed() }, label = { Text(label) })
            }
            val sim = d.sim
            if (sim != null) {
                val setup = d.setup
                val chan = setup.getJSONArray("rules").objs().firstOrNull { it.getJSONObject("when").has("chan") }?.getJSONObject("when")?.getInt("chan")
                    ?: readable(setup).firstOrNull()?.getInt("chan")
                val times = sim.getJSONArray("times"); val series = sim.getJSONObject("series").optJSONArray(chan.toString())
                val pts = (0 until times.length()).map { times.getDouble(it) to (if (series == null || series.isNull(it)) null else series.getDouble(it)) }
                val events = sim.getJSONArray("events").objs()
                val lines = setup.getJSONArray("rules").objs().map { it.getJSONObject("when") }.filter { it.getString("type") == "compare" && it.getInt("chan") == chan }.map { it.getDouble("value") }
                val marks = events.filter { it.getString("type") == "alarm" }.map { it.getDouble("t") to it.getBoolean("raised") }
                val bands = mutableListOf<Pair<Double, Double>>(); var on: Double? = null
                for (e in events) if (e.getString("type") == "output") { if (e.getBoolean("on")) on = e.getDouble("t") else if (on != null) { bands.add(on to e.getDouble("t")); on = null } }
                if (on != null) bands.add(on to 86400.0)
                LineChart(pts, from = 0.0, to = 86400.0, lines = lines, marks = marks, bands = bands)
                val s = sim.getJSONObject("summary")
                Text("${s.getInt("alarms")} alarm(s) · ${s.getInt("reports")} reports. Runs the same code as the sensor.", style = MaterialTheme.typography.bodySmall)
            } else Text(if (k != null && !k.optBoolean("ok")) "" else "Working …", style = MaterialTheme.typography.bodySmall)
        }
        SCard {
            if (k != null && k.optBoolean("ok")) {
                Text("Fits on the sensor", fontWeight = FontWeight.SemiBold)
                Meter("Rules", "${k.getInt("rules")} of ${k.getInt("rule_limit")}", k.getInt("rules").toFloat() / maxOf(1, k.getInt("rule_limit")))
                Meter("Inputs and outputs", "${k.getInt("sources")} of ${k.getInt("source_limit")}", k.getInt("sources").toFloat() / maxOf(1, k.getInt("source_limit")))
                Meter("Setup size", "${k.getInt("size")} of ${k.getInt("limit")} bytes", k.getInt("size").toFloat() / maxOf(1, k.getInt("limit")))
                Row { Text("Radio time", Modifier.weight(1f)); Text("≈ ${k.getDouble("airtime_s_per_day")} s per day", fontFamily = FontFamily.Monospace) }
                val w = k.optJSONArray("warnings")
                if (w != null) for (i in 0 until w.length()) Text("! " + w.getString(i), color = Amber, style = MaterialTheme.typography.bodySmall)
            } else Text(k?.optString("error") ?: "Checking …", color = MaterialTheme.colorScheme.error)
        }
        val prog by Sensors.progress.collectAsState()
        val who = d.setup.optString("name").ifBlank { "the sensor" }
        if (prog != null || sent != null || result.isNotBlank()) Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(18.dp)).background(MaterialTheme.colorScheme.primaryContainer).padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("Sending over radio", fontWeight = FontWeight.SemiBold)
            Text(sent ?: "Waiting at a post office. $who fetches it after its next reading, within ${d.setup.optInt("interval_s", 900) / 60} minutes.")
            prog?.let { (n, tot) ->
                LinearProgressIndicator(progress = { if (tot == 0) 0f else n.toFloat() / tot }, Modifier.fillMaxWidth().height(8.dp).clip(RoundedCornerShape(99.dp)))
                Text("$n of $tot packets sent", style = MaterialTheme.typography.bodySmall)
            }
            if (prog == null && result.isNotBlank()) Text(result, style = MaterialTheme.typography.bodySmall)
        }
        Button(onClick = { result = Sensors.sendRadio() }, Modifier.fillMaxWidth().height(52.dp), enabled = k?.optBoolean("ok") == true) { Text("Send to $who") }
        OutlinedButton(onClick = { Sensors.saveSetup(); result = "Saved on this host" }, Modifier.fillMaxWidth(), enabled = d.node != null) { Text("Save on this host") }
        Text("First setup of a new board needs the PC. Changes can be sent from here.",
            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        androidx.compose.foundation.layout.Spacer(Modifier.height(24.dp))
    }
}

@file:OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class, androidx.compose.material3.ExperimentalMaterial3Api::class)

package dk.maarse.app.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import android.content.ClipData
import android.content.ClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.layout.imePadding
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
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.AcUnit
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Air
import androidx.compose.material.icons.filled.BatteryAlert
import androidx.compose.material.icons.filled.Cloud
import androidx.compose.material.icons.filled.Mail
import androidx.compose.material.icons.filled.MeetingRoom
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Sensors as SensorsIcon
import androidx.compose.material.icons.filled.Thermostat
import androidx.compose.material.icons.filled.Usb
import dk.maarse.app.tr
import dk.maarse.app.SensorDesign
import dk.maarse.app.SensorChan
import dk.maarse.app.SensorInfo
import dk.maarse.app.Sensors
import org.json.JSONArray
import org.json.JSONObject

/** The sensors filter and the designer step, kept when you go into a rule and back. */
object SensorsNav {
    var filter by mutableStateOf("all")
    /** The designer's step: 1 Setup, 2 Rules, 3 Test and send. `stepFor` is the design it belongs to. */
    var step by mutableStateOf(1)
    var stepFor: Any? = null
    /** The USB explanation page is shown inside the start page (a phone cannot claim a board). */
    var usb by mutableStateOf(false)
}

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
    // Roles in a rule sentence: 0 what to watch (teal, like sensors), 1 the condition (blue), 2 what to do (green),
    // 5 an alarm (red), 3 and 4 neutral. Amber means "quiet" and is not used here.
    val (bg, fg) = when (kind) {
        0 -> if (dark) Color(0xFF16363A) to Color(0xFF5CC8D0) else Color(0xFFDDF0F1) to Color(0xFF0E5359)
        1 -> if (dark) Color(0xFF1B2B4A) to Color(0xFF8AB4FF) else Color(0xFFDBE6FB) to Color(0xFF173F86)
        2 -> if (dark) Color(0xFF1D3A31) to Color(0xFF5CC0A0) else Color(0xFFDCECE6) to Color(0xFF16503F)
        5 -> if (dark) Color(0xFF4A1D1A) to Color(0xFFFF8A80) else Color(0xFFFBDAD6) to Color(0xFF8C1D14)
        else -> c.surfaceContainerHigh to c.onSurface
    }
    Box(Modifier.defaultMinSize(minHeight = 48.dp).clip(RoundedCornerShape(24.dp)).background(bg).clickable(onClick = onClick).padding(horizontal = 16.dp), contentAlignment = Alignment.Center) {
        Text(text, color = fg, fontWeight = FontWeight.SemiBold)
    }
}

@Composable
private fun Kw(t: String) = Text(tr(t).uppercase(), style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.ExtraBold,
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

// ---------- the Sensors segment of Nodes ----------

/** Your sensors, favourites first: a summary line, then cards. Alarms and quiet sensors stand out. */
@Composable
fun SensorsPanel(onOpen: (String) -> Unit, onNew: () -> Unit) {
    val list by Sensors.list.collectAsState()
    val err by Sensors.error.collectAsState()
    LaunchedEffect(Unit) { Sensors.refresh() }
    val all = list ?: emptyList()
    val alarms = all.count { it.alarm != null }
    val quiet = all.count { it.quiet && it.alarm == null }
    val shown = all.filter {
        when (SensorsNav.filter) { "fav" -> it.favourite; "alarm" -> it.alarm != null || it.quiet; else -> true }
    }
    val filters = listOf("all", "fav", "alarm")
    Box(Modifier.fillMaxSize()) {
        LazyColumn(Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(12.dp),
            contentPadding = androidx.compose.foundation.layout.PaddingValues(bottom = 96.dp)) {
            if (all.isNotEmpty()) {
                item { SensorSummary(alarms, quiet, all.size - alarms - quiet) }
                item {
                    ChipRow(listOf("Mine", "Favourites", if (alarms + quiet > 0) "Alarms · ${alarms + quiet}" else "Alarms"),
                        filters.indexOf(SensorsNav.filter).coerceAtLeast(0), { SensorsNav.filter = filters[it] })
                }
            }
            if (err.isNotBlank() && all.isEmpty()) item { Text(err, Modifier.padding(24.dp), color = MaterialTheme.colorScheme.onSurfaceVariant) }
            else if (list != null && all.isEmpty()) item { SensorsEmpty(onNew) }
            else if (list != null && shown.isEmpty()) item { Text("Nothing here.", Modifier.padding(24.dp), color = MaterialTheme.colorScheme.onSurfaceVariant) }
            items(shown, key = { it.node }) { s -> Box(Modifier.padding(horizontal = 16.dp)) { SensorCard(s) { onOpen(s.node) } } }
        }
        if (all.isNotEmpty()) ExtendedFloatingActionButton(onClick = onNew, Modifier.align(Alignment.BottomEnd).padding(20.dp),
            containerColor = MaterialTheme.colorScheme.primary, contentColor = MaterialTheme.colorScheme.onPrimary) { Text("+ Add sensor", fontWeight = FontWeight.SemiBold) }
    }
}

/** "1 alarm · 2 quiet · 4 fine" with the status colours: red, amber, green. */
@Composable
private fun SensorSummary(alarm: Int, quiet: Int, fine: Int) {
    Row(Modifier.padding(horizontal = 20.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(18.dp), verticalAlignment = Alignment.CenterVertically) {
        for ((n, label, color) in listOf(Triple(alarm, "alarm", Red), Triple(quiet, "quiet", Amber), Triple(fine, "fine", MaterialTheme.colorScheme.primary))) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Box(Modifier.size(9.dp).clip(androidx.compose.foundation.shape.CircleShape).background(color))
                Text("$n $label", style = MaterialTheme.typography.labelLarge, fontWeight = FontWeight.SemiBold)
            }
        }
    }
}

@Composable
private fun SensorsEmpty(onNew: () -> Unit) {
    Column(Modifier.fillMaxWidth().padding(horizontal = 32.dp, vertical = 40.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Box(Modifier.size(72.dp).clip(androidx.compose.foundation.shape.CircleShape).background(SensorColors.tealContainer), contentAlignment = Alignment.Center) {
            Icon(Icons.Default.SensorsIcon, null, tint = SensorColors.teal, modifier = Modifier.size(36.dp))
        }
        Text("No sensors yet", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold)
        Text("Set one up with a Grove or I²C sensor and a small radio, or wait for one nearby to announce itself.", textAlign = TextAlign.Center,
            color = MaterialTheme.colorScheme.onSurfaceVariant)
        Button(onClick = onNew, Modifier.heightIn(min = 48.dp)) { Text("Set up a sensor") }
        OutlinedButton(onClick = onNew, Modifier.heightIn(min = 48.dp)) { Text("Paste a shared link") }
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

/** A small tag: Alarm (red) or Quiet (amber). */
@Composable
private fun StatePill(alarm: Boolean, text: String) {
    val dark = MaterialTheme.colorScheme.background.luminance() < 0.5f
    val bg = if (alarm) (if (dark) Color(0xFF4A1D1A) else Color(0xFFFBDAD6)) else (if (dark) Color(0xFF4A3214) else Color(0xFFFDE9C8))
    val fg = if (alarm) (if (dark) Color(0xFFFF8A80) else Color(0xFF8C1D14)) else (if (dark) Color(0xFFFFDDB8) else Color(0xFF5A2D05))
    Box(Modifier.clip(RoundedCornerShape(99.dp)).background(bg).padding(horizontal = 10.dp, vertical = 4.dp)) {
        Text(text, style = MaterialTheme.typography.labelMedium, fontWeight = FontWeight.Bold, color = fg, maxLines = 1)
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
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            Box(Modifier.size(28.dp).clip(RoundedCornerShape(8.dp)).background(if (quiet && !alarm) Amber else SensorColors.teal), contentAlignment = Alignment.Center) {
                Icon(Icons.Default.SensorsIcon, null, tint = Color.White, modifier = Modifier.size(16.dp))
            }
            Text((if (s.favourite) "★ " else "") + s.title, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold,
                modifier = Modifier.weight(1f), maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis)
            if (alarm) StatePill(true, "Alarm") else if (quiet) StatePill(false, "Quiet ${s.ageS / 3600} h")
        }
        if (alarm) Text(s.alarm!!, fontWeight = FontWeight.SemiBold, color = cs.error)
        if (quiet && !alarm) {
            val bat = s.channels.firstOrNull { it.name.contains("battery", true) && it.value != null }
            Text("No reading for ${s.ageS / 3600} hours." + (if (bat != null) " Last battery reading was ${fmt(bat.value)} ${bat.unit}." else "") + " It may be flat.", color = cs.onTertiaryContainer)
        } else {
            Row(horizontalArrangement = Arrangement.spacedBy(20.dp)) {
                for (c in s.channels.take(3)) Column(Modifier.weight(1f, fill = false)) {
                    Text(c.name, style = MaterialTheme.typography.labelSmall, color = cs.onSurfaceVariant, maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis)
                    Text(fmt(c.value) + if (c.unit.isNotBlank()) " " + c.unit else "", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold,
                        color = if (alarm && c === lead) cs.error else cs.onSurface, maxLines = 1)
                }
            }
            if (pts.size >= 2) Spark(pts, if (alarm) cs.error else SensorColors.teal)
            Text("Heard " + ago(s.ageS), style = MaterialTheme.typography.bodySmall, color = cs.onSurfaceVariant)
        }
    }
}

// ---------- one sensor ----------

@Composable
fun SensorDetailScreen(node: String, onBack: () -> Unit, onEdit: (String) -> Unit) {
    val list by Sensors.list.collectAsState()
    val s = list?.firstOrNull { it.node == node }
    val ctx = LocalContext.current
    var range by remember { mutableStateOf(86400L) }
    var renaming by remember { mutableStateOf(false) }
    var forgetting by remember { mutableStateOf(false) }
    var menu by remember { mutableStateOf(false) }
    var note by remember { mutableStateOf("") }
    var alarms by remember { mutableStateOf<List<Triple<Long, Boolean, String>>>(emptyList()) }
    LaunchedEffect(node) { Sensors.alarms(node) { alarms = it } }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        if (s == null) { Text("This sensor is not known to the host (any more)."); return@Column }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(s.title, style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold)
                Text(node + " · heard " + ago(s.ageS), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            // Rename, Copy link and Forget live in a menu; Edit setup is the one main action.
            Box {
                androidx.compose.material3.IconButton(onClick = { menu = true }) {
                    Icon(Icons.Default.MoreVert, "More")
                }
                androidx.compose.material3.DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    androidx.compose.material3.DropdownMenuItem(text = { Text("Rename") }, onClick = { menu = false; renaming = true })
                    androidx.compose.material3.DropdownMenuItem(text = { Text("Copy link") }, enabled = s.hasSetup, onClick = {
                        menu = false
                        Sensors.copyFrom(node) { r ->
                            r.onSuccess { setup ->
                                (ctx.getSystemService(android.content.Context.CLIPBOARD_SERVICE) as ClipboardManager).setPrimaryClip(ClipData.newPlainText("Må:rse sensor", Sensors.shareText(setup)))
                                note = "Link copied"
                            }.onFailure { note = it.message ?: "Could not copy the link" }
                        }
                    })
                    androidx.compose.material3.DropdownMenuItem(text = { Text("Forget this sensor…", color = MaterialTheme.colorScheme.error) }, onClick = { menu = false; forgetting = true })
                }
            }
        }
        if (note.isNotBlank()) Text(note, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.primary)
        if (s.alarm != null) Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(MaterialTheme.colorScheme.errorContainer).padding(14.dp)) {
            Text("⚠ " + s.alarm, fontWeight = FontWeight.SemiBold, color = MaterialTheme.colorScheme.onErrorContainer)
            TextButton(onClick = { Sensors.dismiss(node) }, Modifier.heightIn(min = 48.dp)) { Text("Mark as read") }
        }
        else if (s.quiet) Text("No reading for ${s.ageS / 3600} hours. The battery may be flat, or the sensor is out of range.",
            Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(MaterialTheme.colorScheme.tertiaryContainer).padding(14.dp),
            color = MaterialTheme.colorScheme.onTertiaryContainer)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChip(selected = s.favourite, onClick = { Sensors.meta(node, favourite = !s.favourite) }, label = { Text(if (s.favourite) "★ Favourite" else "☆ Favourite") })
            FilterChip(selected = range == 86400L, onClick = { range = 86400L }, label = { Text("24 hours") })
            FilterChip(selected = range == 604800L, onClick = { range = 604800L }, label = { Text("7 days") })
        }
        for (c in s.channels) ChannelCard(node, c, range, alarmed = s.alarm != null && c === s.channels.firstOrNull { it.unit.isNotBlank() })
        SCard { Text("Alarms", fontWeight = FontWeight.SemiBold)
            if (alarms.isEmpty()) Text("None so far.", style = MaterialTheme.typography.bodySmall)
            for ((t, raised, text) in alarms.take(10)) Text((if (raised) "⚠ " else "✓ Cleared: ") + text + " · " + time(t * 1000), style = MaterialTheme.typography.bodySmall,
                color = if (raised) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurface)
        }
        Button(onClick = { onEdit(node) }, Modifier.fillMaxWidth().height(52.dp)) { Text("Edit setup") }
        androidx.compose.foundation.layout.Spacer(Modifier.height(8.dp))
    }
    if (renaming) TextDialog("Name of this sensor", s?.name ?: "", { Sensors.meta(node, name = it) }, { renaming = false })
    if (forgetting) AlertDialog(onDismissRequest = { forgetting = false }, title = { Text("Forget this sensor?") },
        text = { Text("Its history is removed from this host. The sensor itself is not changed.") },
        confirmButton = { TextButton(onClick = { Sensors.forget(node); forgetting = false; onBack() }) { Text("Forget", color = MaterialTheme.colorScheme.error) } },
        dismissButton = { TextButton(onClick = { forgetting = false }) { Text("Cancel") } })
}

@Composable
private fun ChannelCard(node: String, c: SensorChan, range: Long, alarmed: Boolean = false) {
    var pts by remember(node, c.chan, range) { mutableStateOf<List<Pair<Double, Double?>>>(emptyList()) }
    LaunchedEffect(node, c.chan, range) { Sensors.series(node, c.chan, range) { pts = it } }
    SCard {
        Text(c.name + " · channel " + c.chan, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(fmt(c.value) + " " + c.unit, style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.SemiBold,
            color = if (alarmed) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurface)
        LineChart(pts)
    }
}

// ---------- start, designer, rule, send ----------

private fun templateIcon(key: String) = when (key) {
    "climate" -> Icons.Default.Thermostat
    "frost" -> Icons.Default.AcUnit
    "door" -> Icons.Default.MeetingRoom
    "mailbox" -> Icons.Default.Mail
    "weather" -> Icons.Default.Cloud
    "air" -> Icons.Default.Air
    "motion" -> Icons.Default.SensorsIcon
    "battery" -> Icons.Default.BatteryAlert
    else -> Icons.Default.Add
}

/**
 * Set up a sensor: the USB card first (a new board is claimed on a computer), then the starting points as a grid.
 * `onUsb`, when given, opens the USB explanation as a page of its own; without it the page opens inside this screen.
 */
@Composable
fun SensorStartScreen(onPick: (String) -> Unit, onImported: () -> Unit, onUsb: (() -> Unit)? = null) {
    Sensors.catalog.collectAsState().value
    val ctx = LocalContext.current
    val mine by Sensors.list.collectAsState()
    var msg by remember { mutableStateOf("") }
    var copyDialog by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) { Sensors.loadCatalog() }
    androidx.activity.compose.BackHandler(SensorsNav.usb) { SensorsNav.usb = false }
    androidx.compose.runtime.DisposableEffect(Unit) { onDispose { SensorsNav.usb = false } }
    if (SensorsNav.usb) { SensorUsbScreen(onBack = { SensorsNav.usb = false }); return }
    val templates = Sensors.templates()
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text("Pick what it should do, test it here, then send it to the node. You can change it later, also over the radio.", color = MaterialTheme.colorScheme.onSurfaceVariant)
        Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(SensorColors.tealContainer).clickable { if (onUsb != null) onUsb() else SensorsNav.usb = true }.padding(14.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Box(Modifier.size(40.dp).clip(androidx.compose.foundation.shape.CircleShape).background(SensorColors.teal), contentAlignment = Alignment.Center) {
                Icon(Icons.Default.Usb, null, tint = Color.White)
            }
            Column(Modifier.weight(1f)) {
                Text("First setup of a new board", fontWeight = FontWeight.SemiBold, color = MaterialTheme.colorScheme.onSurface)
                Text("Done once on a computer over USB. The sensor takes you as its owner. After that, changes can be sent from here.", style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        STitle("Pick a starting point")
        if (templates.isEmpty()) Text("Loading the starting points from your host … If this stays, the host is too old: update maarse-host.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        else templates.chunked(2).forEach { row ->
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                for (t in row) Column(Modifier.weight(1f).heightIn(min = 128.dp).clip(RoundedCornerShape(16.dp)).background(MaterialTheme.colorScheme.surfaceContainer)
                    .clickable { onPick(t.key) }.padding(14.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Icon(templateIcon(t.key), null, tint = SensorColors.teal, modifier = Modifier.size(26.dp))
                    Text(t.title, fontWeight = FontWeight.SemiBold, style = MaterialTheme.typography.titleSmall.copy(hyphens = androidx.compose.ui.text.style.Hyphens.Auto, lineBreak = androidx.compose.ui.text.style.LineBreak.Paragraph))
                    Text(t.text, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                if (row.size == 1) androidx.compose.foundation.layout.Spacer(Modifier.weight(1f))
            }
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

/**
 * First setup over USB, explained. A phone cannot claim a new board (the claim needs a USB serial link
 * to the board), so this page says so and shows what to do on the computer.
 */
@Composable
fun SensorUsbScreen(onBack: (() -> Unit)? = null) {
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        if (onBack != null) TextButton(onClick = onBack, Modifier.heightIn(min = 48.dp)) { Text("‹ Back to starting points") }
        Text("First setup over USB", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold)
        Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(MaterialTheme.colorScheme.tertiaryContainer).padding(14.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Text("Continue on your computer", fontWeight = FontWeight.SemiBold, color = MaterialTheme.colorScheme.onTertiaryContainer)
            Text("A phone cannot claim a new board yet. Open Må:rse on the computer: Sensors, Set up a sensor, then the USB step. After that, changes can be sent from this phone over the radio.",
                color = MaterialTheme.colorScheme.onTertiaryContainer)
        }
        for ((i, step) in listOf(
            Triple("Plug the board in", "Connect the sensor board to the computer with a USB cable.", 1),
            Triple("Claim it", "The board learns that you own it. This takes a second.", 2),
            Triple("Send the setup", "Your name, inputs and rules go onto the board.", 3),
        ).withIndex()) {
            Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp)).background(MaterialTheme.colorScheme.surfaceContainer).padding(14.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                Box(Modifier.size(32.dp).clip(androidx.compose.foundation.shape.CircleShape).background(SensorColors.tealContainer), contentAlignment = Alignment.Center) {
                    Text("${i + 1}", fontWeight = FontWeight.Bold, color = SensorColors.teal)
                }
                Column(Modifier.weight(1f)) {
                    Text(step.first, fontWeight = FontWeight.SemiBold)
                    Text(step.second, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
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
        "every" -> tr("Every {0} minutes", w.getInt("secs") / 60)
        "compare" -> tr("When") + " ${srcName(setup, w.getInt("chan"))} " + tr(OPS.firstOrNull { it.first == w.getString("op") }?.second ?: "") + " ${fmt(w.getDouble("value"))} ${unitOf(setup, w.getInt("chan"))}"
        "change" -> tr("When {0} changes by more than {1}", srcName(setup, w.getInt("chan")), fmt(w.getDouble("delta")))
        else -> tr(when (w.optString("to")) { "high" -> "When {0} reads 1"; "low" -> "When {0} reads 0"; else -> "When {0} changes" }, srcName(setup, w.getInt("chan")))
    }
    val acts = r.getJSONArray("then").objs().joinToString(", ") {
        when (it.getString("type")) {
            "report" -> tr("send readings")
            "alarm" -> tr("alarm “{0}”", it.optString("text"))
            "set_out" -> tr(if (it.getBoolean("on")) "set {0} on" else "set {0} off", srcName(setup, it.getInt("chan")))
            else -> tr("wake every {0} s", it.optInt("secs"))
        }
    }
    return "$head → $acts"
}

/** "✓ Fits on the sensor" with the numbers; always visible at the bottom of the designer. */
@Composable
private fun FitBar(d: SensorDesign, @Suppress("UNUSED_PARAMETER") rev: Int) {
    val k = d.compiled
    val ok = k?.optBoolean("ok") == true
    val bad = k != null && !ok
    val cs = MaterialTheme.colorScheme
    Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(if (bad) cs.errorContainer else cs.primaryContainer).padding(horizontal = 14.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(if (ok) "✓" else if (bad) "!" else "…", fontWeight = FontWeight.Bold, color = if (bad) cs.onErrorContainer else cs.onPrimaryContainer)
        Column(Modifier.weight(1f)) {
            Text(if (ok) "Fits on the sensor" else if (bad) "Does not fit yet" else "Checking …", fontWeight = FontWeight.SemiBold, color = if (bad) cs.onErrorContainer else cs.onPrimaryContainer)
            val line = when {
                ok -> "Rules ${k!!.getInt("rules")} of ${k.getInt("rule_limit")} · Inputs ${k.getInt("sources")} of ${k.getInt("source_limit")} · ${k.getInt("size")} of ${k.getInt("limit")} bytes · ≈ ${"%.0f".format(k.getDouble("airtime_s_per_day"))} s radio per day"
                bad -> k!!.optString("error")
                else -> ""
            }
            if (line.isNotBlank()) Text(line, style = MaterialTheme.typography.bodySmall, color = if (bad) cs.onErrorContainer else cs.onPrimaryContainer)
        }
    }
}

/** Setup, Rules, Test and send. Tap a step to jump to it. */
@Composable
private fun Stepper(step: Int, onStep: (Int) -> Unit) {
    val cs = MaterialTheme.colorScheme
    Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
        listOf("Setup", "Rules", "Test and send").forEachIndexed { i, label ->
            val n = i + 1
            val on = n == step
            Row(Modifier.weight(if (n == 3) 1.5f else if (n == 1) 1.3f else 1f).heightIn(min = 48.dp).clip(RoundedCornerShape(12.dp)).clickable { onStep(n) }.padding(horizontal = 4.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Box(Modifier.size(24.dp).clip(androidx.compose.foundation.shape.CircleShape).background(if (on) cs.primary else if (n < step) cs.primaryContainer else cs.surfaceContainerHigh),
                    contentAlignment = Alignment.Center) {
                    Text(if (n < step) "✓" else "$n", fontSize = 12.sp, fontWeight = FontWeight.Bold, color = if (on) cs.onPrimary else if (n < step) cs.onPrimaryContainer else cs.onSurfaceVariant)
                }
                Text(label, style = MaterialTheme.typography.labelLarge, fontWeight = if (on) FontWeight.Bold else FontWeight.Normal, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    color = if (on) cs.onSurface else cs.onSurfaceVariant)
            }
        }
    }
}

@Composable
fun SensorDesignScreen(onRule: (Int) -> Unit, onSend: () -> Unit) {
    val rev = Sensors.rev.collectAsState().value
    val d = Sensors.design ?: return
    // A new design starts at step 1; coming back from a rule keeps the step.
    remember(d) { if (SensorsNav.stepFor !== d) { SensorsNav.stepFor = d; SensorsNav.step = 1 }; 0 }
    val step = SensorsNav.step
    val setup = d.setup
    var dialog by remember { mutableStateOf<String?>(null) }
    var tplKey by remember { mutableStateOf<String?>(null) }
    var copied by remember { mutableStateOf(false) }
    var result by remember { mutableStateOf("") }
    val ctx = LocalContext.current
    val board = setup.optString("board", "xiao_nrf52840")
    val who = setup.optString("name").ifBlank { "the sensor" }
    Column(Modifier.fillMaxSize()) {
        Stepper(step) { SensorsNav.step = it }
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            if (step == 1) SetupStep(setup, board, d.note, rev, { dialog = it })
            else if (step == 2) RulesStep(setup, ctx, copied, rev, { copied = true }, { dialog = it }, onRule)
            else TestAndSendBody(d, result, rev)
            androidx.compose.foundation.layout.Spacer(Modifier.height(8.dp))
        }
        // Fixed at the bottom: whether it fits, and the way on.
        Column(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceContainer).padding(horizontal = 16.dp, vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            FitBar(d, rev)
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                if (step > 1) OutlinedButton(onClick = { SensorsNav.step = step - 1 }, Modifier.weight(1f).height(48.dp)) { Text("Back") }
                if (step < 3) Button(onClick = { SensorsNav.step = step + 1 }, Modifier.weight(1f).height(48.dp)) { Text("Next") }
                else Button(onClick = { result = Sensors.sendRadio() }, Modifier.weight(1f).height(48.dp), enabled = d.compiled?.optBoolean("ok") == true) { Text("Send to $who", maxLines = 1) }
            }
        }
    }
    when (dialog) {
        "board" -> PickDialog("Board", Sensors.boards(), board, { id ->
            if (Sensors.boardAvailable(id)) { setup.put("board", id); d.note = ""; Sensors.changed() }
            else { d.note = Sensors.boardName(id).substringBefore(" (") + " cannot read sensors yet: its firmware is not ready."; Sensors.changed() }
        }, { dialog = null })
        "interval" -> NumberDialog("Wake and read every", (setup.optInt("interval_s", 900) / 60).toDouble(), "minutes", { setup.put("interval_s", maxOf(1, it.toInt()) * 60); Sensors.changed() }, { dialog = null })
        "addsource" -> AddSensorPage(setup, board, onCustom = { dialog = "custom" }, onDismiss = { dialog = null })
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

/** Step 1: the name, the board, the inputs and outputs, and who gets the messages. */
@Composable
private fun SetupStep(setup: JSONObject, board: String, note: String, @Suppress("UNUSED_PARAMETER") rev: Int, open: (String) -> Unit) {
    OutlinedTextField(setup.optString("name"), { setup.put("name", it.take(30)); Sensors.changed() }, Modifier.fillMaxWidth(), label = { Text("Name, e.g. Cellar") }, singleLine = true)
    if (note.isNotBlank()) Text(note, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
    STitle("This sensor")
    SCard {
        Chip(Sensors.boardName(board), 3) { open("board") }
        Sensors.boardNote(board).takeIf { it.isNotBlank() }?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
        Sensors.boardGrove(board).takeIf { it.isNotBlank() }?.let { g ->
            var more by remember { mutableStateOf(false) }
            TextButton(onClick = { more = !more }, Modifier.heightIn(min = 48.dp), contentPadding = androidx.compose.foundation.layout.PaddingValues(0.dp)) { Text((if (more) "▾ " else "▸ ") + tr("Plugging in Grove sensors"), style = MaterialTheme.typography.bodyMedium) }
            if (more) Text(g, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Text("Wake and read every", style = MaterialTheme.typography.bodySmall)
        Chip("${setup.optInt("interval_s", 900) / 60} minutes", 3) { open("interval") }
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
            Text("#${s.getInt("chan")}  ${tr(s.optString("name"))} · " + if (first) inputText(inp, board) else tr("same chip as #{0}", chips[chipKey]), style = MaterialTheme.typography.bodyMedium)
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
        OutlinedButton(onClick = { open("addsource") }, Modifier.heightIn(min = 48.dp)) { Text("+ Add sensor or pin") }
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
}

/** Step 2: the rules as sentences, the host's "went quiet" rule, templates and the share link. */
@Composable
private fun RulesStep(setup: JSONObject, ctx: android.content.Context, copied: Boolean, @Suppress("UNUSED_PARAMETER") rev: Int, onCopied: () -> Unit, open: (String) -> Unit, onRule: (Int) -> Unit) {
    val rules = setup.getJSONArray("rules").objs()
    STitle("Rules")
    SGroup {
        if (rules.isEmpty()) Text("No rules yet. Add one to get an alarm or a regular report.", Modifier.padding(14.dp), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        rules.forEachIndexed { i, r ->
            Row(Modifier.fillMaxWidth().heightIn(min = 64.dp).clickable { onRule(i) }.padding(start = 14.dp, end = 6.dp, top = 8.dp, bottom = 8.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Text(r.optString("name").ifBlank { "Rule ${i + 1}" }, fontWeight = FontWeight.SemiBold)
                    Text(ruleSummary(setup, r), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                Switch(r.optBoolean("on", true), { r.put("on", it); Sensors.changed() })
            }
            androidx.compose.material3.HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
        }
        TextButton(onClick = {
            val id = (rules.maxOfOrNull { it.optInt("id") } ?: 0) + 1
            val src = readable(setup).firstOrNull()
            val rule = JSONObject("""{"id":$id,"name":"Rule $id","then":[{"type":"alarm","text":"Alarm"}]}""")
            rule.put("when", if (src != null) JSONObject("""{"type":"compare","chan":${src.getInt("chan")},"op":"<","value":10,"clear":10,"count":1}""") else JSONObject("""{"type":"every","secs":900}"""))
            setup.getJSONArray("rules").put(rule); Sensors.changed(); onRule(rules.size)
        }, Modifier.heightIn(min = 48.dp).padding(horizontal = 6.dp)) { Text("+ Add a rule") }
    }
    SGroup {
        val q = setup.optJSONObject("quiet")
        GroupRow("Node went quiet", tr("On your host · {0}", if (q?.optBoolean("on", true) == false) tr("off") else tr("no reading for {0} minutes", q?.optInt("min", 120) ?: 120)), last = false) { open("quiet") }
        GroupRow("Start from a template", "Replaces the inputs and rules", last = false) { open("template") }
        GroupRow(if (copied) "Link copied" else "Copy link", "A line of text with this setup, to send to someone", last = true) {
            (ctx.getSystemService(android.content.Context.CLIPBOARD_SERVICE) as ClipboardManager).setPrimaryClip(ClipData.newPlainText("Må:rse sensor", Sensors.shareText(setup)))
            onCopied()
        }
    }
    Text("Rules run on the sensor itself, so alarms work even when your phone is off.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
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
    "i2c" -> (Sensors.sensorByDevice(inp.optString("device"))?.let { sensorLabel(it) } ?: inp.optString("device")) + " · " + tr("sensor port")
    "i2c_custom" -> tr("your own sensor") + " · " + tr("sensor port")
    "bme280" -> tr("Temperature, humidity and air pressure") + " · " + tr("sensor port")
    "battery", "chip_temp" -> tr("built in")
    "adc" -> tr("analogue pin") + " " + (Sensors.adcPins(board).firstOrNull { it.first == inp.optInt("pin") }?.second ?: "${inp.optInt("pin")}")
    else -> inp.getString("type").replace('_', ' ')
}

/** A page that covers the screen with its own back arrow (a sub-page of the designer that the shell has no screen for). */
@Composable
private fun FullPage(title: String, onBack: () -> Unit, content: @Composable androidx.compose.foundation.layout.ColumnScope.() -> Unit) {
    androidx.compose.ui.window.Dialog(onDismissRequest = onBack,
        properties = androidx.compose.ui.window.DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
        androidx.compose.material3.Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
            Column(Modifier.fillMaxSize().systemBarsPadding().imePadding()) {
                Row(Modifier.fillMaxWidth().padding(4.dp), verticalAlignment = Alignment.CenterVertically) {
                    androidx.compose.material3.IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
                    Text(title, style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold)
                }
                content()
            }
        }
    }
}

/** Add a sensor or pin: the board note first, then the choices grouped, each with a tag for how it connects. */
@Composable
private fun AddSensorPage(setup: JSONObject, board: String, onCustom: () -> Unit, onDismiss: () -> Unit) {
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
    fun matches(x: JSONObject) = q.isBlank() || (sensorLabel(x) + " " + x.optString("name") + " " + x.optString("grove") + " " + x.optString("note")).contains(q.trim(), ignoreCase = true)
    fun sub(x: JSONObject): String = listOf(x.optString("grove").ifBlank { if (x.getString("bus") == "i2c") "Chip " + x.optString("name") else "" }, x.optString("note")).filter { it.isNotBlank() }.joinToString(" · ")
    fun usable(x: JSONObject) = when (x.getString("bus")) {
        "i2c", "custom" -> hasI2c
        "analog" -> Sensors.adcPins(board).isNotEmpty() || hasI2c
        "builtin" -> true
        else -> freePins(setup, board).isNotEmpty()
    }
    fun why(x: JSONObject) = when (x.getString("bus")) { "i2c", "custom" -> "This board has no sensor port."; "analog" -> "This board cannot read analogue sensors."; else -> "No free pin: remove one first." }
    fun tag(x: JSONObject) = when (x.getString("bus")) {
        "i2c", "custom" -> "I²C"
        "analog" -> if (Sensors.adcPins(board).isNotEmpty()) "analog pin" else "needs ADS1115"
        "builtin" -> "built in"
        else -> "1 pin"
    }
    val update = Sensors.catalog.collectAsState().value?.optJSONObject("update")
    var note by remember { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    FullPage("Add a sensor or pin", onDismiss) {
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            OutlinedTextField(q, { q = it }, Modifier.fillMaxWidth(), singleLine = true, label = { Text("Search") }, placeholder = { Text("SHT31, soil, door, CO2") })
            // What this board can take, so a choice that does not fit is not a surprise.
            Column(Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(SensorColors.tealContainer).padding(12.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(Sensors.boardName(board), fontWeight = FontWeight.SemiBold, color = MaterialTheme.colorScheme.onSurface)
                Text((if (hasI2c) tr("I²C on {0}", Sensors.i2cText(board)) else tr("No sensor port")) + " · " + tr(plural(freePins(setup, board).size, "free pin")) +
                    (if (Sensors.adcPins(board).isEmpty() && hasI2c) ". " + tr("Analog sensors need an ADS1115.") else "."),
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            @Composable fun Items(items: List<JSONObject>) {
                for (g in items.map { it.optString("group") }.distinct()) {
                    val rows = items.filter { it.optString("group") == g && matches(it) }
                    if (rows.isEmpty()) continue
                    STitle(g)
                    SGroup {
                        rows.forEachIndexed { i, x ->
                            val ok = usable(x)
                            Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).clickable(enabled = ok) { onDismiss(); if (x.getString("bus") == "custom") onCustom() else addSensor(setup, board, x) }
                                .padding(horizontal = 14.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                                Column(Modifier.weight(1f)) {
                                    Text(sensorLabel(x), color = if (ok) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurfaceVariant, fontWeight = FontWeight.Medium)
                                    val line = if (ok) sub(x) else why(x)
                                    if (line.isNotBlank()) Text(line, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                                }
                                Box(Modifier.clip(RoundedCornerShape(6.dp)).background(if (ok) SensorColors.tealContainer else MaterialTheme.colorScheme.surfaceContainerHigh).padding(horizontal = 7.dp, vertical = 2.dp)) {
                                    Text(tag(x), style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.Bold, maxLines = 1,
                                        color = if (ok) SensorColors.teal else MaterialTheme.colorScheme.onSurfaceVariant)
                                }
                            }
                            if (i < rows.lastIndex) androidx.compose.material3.HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                        }
                    }
                }
            }
            Items(simple)
            val advRows = adv.filter { matches(it) }
            if (advRows.isNotEmpty()) {
                val shown = more || q.isNotBlank()
                TextButton(onClick = { more = !more }, Modifier.heightIn(min = 48.dp), contentPadding = androidx.compose.foundation.layout.PaddingValues(0.dp)) { Text((if (shown) "▾ " else "▸ ") + "More, for tinkerers", style = MaterialTheme.typography.bodyMedium) }
                if (shown) Items(advRows)
            }
            if (simple.none { matches(it) } && advRows.isEmpty()) Text("Nothing matches. Try another word, like the name on the box.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(top = 12.dp))
            if (update != null) {
                val day = { t: Long -> java.text.DateFormat.getDateInstance().format(java.util.Date(t * 1000)) }
                val ver = update.optLong("version")
                Text("Sensor list from ${day(ver)} (" + (if (ver == update.optLong("built_in_version")) "came with this program" else "updated from GitHub") + ")" +
                    update.optString("error").takeIf { it.isNotBlank() }?.let { " · $it" }.orEmpty() + if (note.isNotBlank()) " · $note" else "",
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(top = 8.dp))
                if (update.optBoolean("can_update")) Row(verticalAlignment = Alignment.CenterVertically) {
                    TextButton(enabled = !busy, onClick = { busy = true; note = "Checking GitHub …"; Sensors.updateCatalog { note = it; busy = false } }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Check for updates") }
                    Checkbox(update.optBoolean("auto", true), { Sensors.setCatalogAuto(it) }); Text("automatically", style = MaterialTheme.typography.bodySmall)
                } else Text("This program cannot check for newer lists.", style = MaterialTheme.typography.bodySmall)
            }
            androidx.compose.foundation.layout.Spacer(Modifier.height(16.dp))
        }
    }
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
    FullPage("Your own I²C sensor", onDismiss) {
            Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
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
        Row(Modifier.fillMaxWidth().padding(16.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            OutlinedButton(onClick = onDismiss, Modifier.weight(1f).height(48.dp)) { Text("Cancel") }
            Button(onClick = {
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
            }, Modifier.weight(1f).height(48.dp)) { Text(if (src == null) "Add" else "Save") }
        }
    }
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
                Chip(when (w.getString("type")) { "every" -> "every …"; "compare" -> "a value …"; "change" -> "a value changes by …"; else -> "a pin …" }, 3) { dialog = "trigger" }
                when (w.getString("type")) {
                    "every" -> Chip("${w.getInt("secs") / 60} minutes", 1) { dialog = "secs" }
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
                Kw("Back to normal")
                Chip(tr("back to normal at {0}", fmt(w.getDouble("clear")) + " " + unitOf(setup, w.getInt("chan"))), 1) { dialog = "clear" }
            }
            val acts = r.getJSONArray("then")
            for (i in 0 until acts.length()) {
                val a = acts.getJSONObject(i)
                Kw(if (i == 0) "Then" else "and")
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    when (a.getString("type")) {
                        "report" -> Chip("send readings", 2) {}
                        "alarm" -> { Chip("send alarm", 5) {}; Chip("“" + a.optString("text") + "”", 5) { actionIdx = i; dialog = "text" } }
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
        FlowRow(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            for ((kind, text) in listOf(0 to "what to watch", 1 to "the condition", 2 to "what to do", 5 to "an alarm")) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    val dark = MaterialTheme.colorScheme.background.luminance() < 0.5f
                    val dot = when (kind) { 0 -> SensorColors.teal; 1 -> if (dark) Color(0xFF8AB4FF) else Color(0xFF173F86); 2 -> MaterialTheme.colorScheme.primary; else -> MaterialTheme.colorScheme.error }
                    Box(Modifier.size(10.dp).clip(RoundedCornerShape(99.dp)).background(dot))
                    Text(text, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
        OutlinedButton(onClick = { set { val n = JSONArray(); for (j in 0 until rules.length()) if (j != index) n.put(rules.get(j)); setup.put("rules", n) }; onBack() }, Modifier.heightIn(min = 48.dp)) {
            Text("Remove this rule", color = MaterialTheme.colorScheme.error)
        }
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

/**
 * Test and send: pretend the sensor sees something, check that it fits, and see how the sending goes.
 * It is step 3 of the designer and also a page of its own.
 */
@Composable
private fun TestAndSendBody(d: SensorDesign, result: String, @Suppress("UNUSED_PARAMETER") rev: Int) {
    val sent by Sensors.status.collectAsState()
    val k = d.compiled
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
            Text("${plural(s.getInt("alarms"), "alarm")} · ${plural(s.getInt("reports"), "report")}. Runs the same code as the sensor.", style = MaterialTheme.typography.bodySmall)
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
        Text("Sending over radio", fontWeight = FontWeight.SemiBold, color = MaterialTheme.colorScheme.onPrimaryContainer)
        Text(sent ?: "Waiting at a post office. $who fetches it after its next reading, within ${d.setup.optInt("interval_s", 900) / 60} minutes.", color = MaterialTheme.colorScheme.onPrimaryContainer)
        prog?.let { (n, tot) ->
            LinearProgressIndicator(progress = { if (tot == 0) 0f else n.toFloat() / tot }, Modifier.fillMaxWidth().height(8.dp).clip(RoundedCornerShape(99.dp)))
            Text("$n of $tot packets sent", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onPrimaryContainer)
        }
        if (prog == null && result.isNotBlank()) Text(result, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onPrimaryContainer)
    }
    var saved by remember { mutableStateOf(false) }
    OutlinedButton(onClick = { Sensors.saveSetup(); saved = true }, Modifier.fillMaxWidth().height(48.dp), enabled = d.node != null) { Text(if (saved) "Saved on this host" else "Save on this host") }
    Text("First setup of a new board needs the PC. Changes can be sent from here.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

/** Test and send as a page of its own (the designer shows the same as its third step). */
@Composable
fun SensorSendScreen() {
    val rev = Sensors.rev.collectAsState().value
    val d = Sensors.design ?: return
    var result by remember { mutableStateOf("") }
    val k = d.compiled
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        TestAndSendBody(d, result, rev)
        Button(onClick = { result = Sensors.sendRadio() }, Modifier.fillMaxWidth().height(52.dp), enabled = k?.optBoolean("ok") == true) { Text("Send to ${d.setup.optString("name").ifBlank { "the sensor" }}") }
        androidx.compose.foundation.layout.Spacer(Modifier.height(24.dp))
    }
}

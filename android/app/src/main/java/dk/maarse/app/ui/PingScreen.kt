package dk.maarse.app.ui

import android.Manifest
import android.content.Intent
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import dk.maarse.app.Node
import dk.maarse.app.PingTest
import dk.maarse.app.tr

private fun snr(v: Int?) = if (v == null) "–" else (if (v > 0) "+" else "") + v + " dB"

/** Experimental > Ping test: repeated trace routes with loss, round trip, hops and SNR, optionally with the phone's position. */
@Composable
fun PingPage() {
    val ctx = LocalContext.current
    val run by PingTest.run.collectAsState()
    val nodes by Node.nodes.collectAsState()
    var dest by remember { mutableStateOf("") }
    var count by remember { mutableStateOf("10") }
    var interval by remember { mutableStateOf("10") }
    var gps by remember { mutableStateOf(false) }
    var menu by remember { mutableStateOf(false) }
    val permission = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok -> gps = ok }
    val busy = run?.finished == false

    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
        Card2(Modifier.padding(top = 4.dp)) {
            Text(
                tr("Sends a trace route again and again and counts the answers: loss, round trip, hops and the worst SNR on the way. " +
                    "Use it on a walk or a drive (switch on the position) to find how far you reach, and to check a route over time. " +
                    "A ping costs airtime, so keep the interval at 10 seconds or more."),
                style = MaterialTheme.typography.bodyMedium,
            )
        }
        GroupTitle("Target")
        Card2 {
            val known = nodes.values.sortedBy { Node.label(it.addr).lowercase() }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedButton(onClick = { menu = true }, enabled = !busy) {
                    Text(if (dest.isEmpty()) tr("Choose a node") else Node.label(dest))
                }
                DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                    for (n in known) DropdownMenuItem(text = { Text(Node.label(n.addr)) }, onClick = { dest = n.addr; menu = false })
                }
            }
            OutlinedTextField(
                value = dest, onValueChange = { dest = it.trim().lowercase() }, enabled = !busy, singleLine = true,
                label = { Text(tr("or an address (32 hex characters)")) }, modifier = Modifier.fillMaxWidth().padding(top = 8.dp),
            )
        }
        GroupTitle("Test")
        Card2 {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(
                    value = count, onValueChange = { count = it.filter(Char::isDigit).take(3) }, enabled = !busy, singleLine = true,
                    label = { Text(tr("Pings")) }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number), modifier = Modifier.weight(1f),
                )
                OutlinedTextField(
                    value = interval, onValueChange = { interval = it.filter(Char::isDigit).take(3) }, enabled = !busy, singleLine = true,
                    label = { Text(tr("Seconds between")) }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number), modifier = Modifier.weight(1f),
                )
            }
            SwitchRow(
                "Record my position with each ping", "For a walk or a drive. Asks for the location permission.", gps, last = true,
            ) { on ->
                if (!on) gps = false
                else if (Node.hasLocation(ctx)) gps = true
                else permission.launch(Manifest.permission.ACCESS_FINE_LOCATION)
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 8.dp)) {
                Button(onClick = {
                    if (busy) PingTest.stop()
                    else if (Regex("[0-9a-f]{32}").matches(dest)) {
                        PingTest.start(ctx, dest, count.toIntOrNull() ?: 10, interval.toIntOrNull() ?: 10, gps)
                    } else Node.lastError.value = "Choose a node or enter an address"
                }) { Text(if (busy) tr("Stop") else tr("Start")) }
                OutlinedButton(enabled = run?.pings?.isNotEmpty() == true, onClick = {
                    val r = run ?: return@OutlinedButton
                    val i = Intent(Intent.ACTION_SEND).apply {
                        type = "text/csv"
                        putExtra(Intent.EXTRA_SUBJECT, "ping-test.csv")
                        putExtra(Intent.EXTRA_TEXT, PingTest.csv(r))
                    }
                    runCatching { ctx.startActivity(Intent.createChooser(i, "Export CSV")) }
                }) { Text(tr("Export CSV")) }
            }
        }
        val r = run
        if (r != null) {
            val e = nodes[r.dest]
            val s = PingTest.summary(r, if (e?.lat != null && e.lon != null && (e.lat != 0.0 || e.lon != 0.0)) e.lat!! to e.lon!! else null)
            GroupTitle("Result")
            Card2 {
                Text("${Node.label(r.dest)}: ${s.answered} of ${s.sent} answered, ${s.lossPct} % lost", fontWeight = FontWeight.SemiBold)
                if (s.avgMs != null) {
                    Text("Round trip ${s.minMs} / ${s.avgMs} / ${s.maxMs} ms (fastest / average / slowest)", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                if (s.farthestM != null) {
                    Text(
                        "Farthest answered ping: ${s.farthestM} m from ${if (s.baseIsTarget) "the node" else "the first position"}",
                        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            GroupTitle("Pings")
            Card2 {
                for (p in r.pings) {
                    val what = when (p.state) {
                        PingTest.State.OK -> "${p.rttMs} ms · ${p.hops} ${if (p.hops == 1) "hop" else "hops"} · out ${snr(p.outMin)} · back ${snr(p.backMin)}"
                        PingTest.State.WAITING -> "waiting…"
                        PingTest.State.ERROR -> "error: ${p.text}"
                        PingTest.State.LOST -> "no answer"
                    }
                    Text("#${p.seq}  $what", style = MaterialTheme.typography.bodyMedium)
                    if (p.lat != null && p.lon != null) {
                        Text("%.5f, %.5f (±%d m)".format(p.lat, p.lon, p.accM ?: 0), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
                Track(r)
            }
        }
    }
}

/** Where the pings went out (green: answered, red: no answer). */
@Composable
private fun Track(r: PingTest.Run) {
    val pts = r.pings.filter { it.lat != null && it.lon != null && it.state != PingTest.State.WAITING }
    if (pts.size < 2) return
    val lat0 = pts.map { it.lat!! }.average()
    val k = Math.cos(lat0 * Math.PI / 180)
    val xs = pts.map { it.lon!! * k }
    val ys = pts.map { -it.lat!! }
    val minX = xs.min(); val maxX = xs.max(); val minY = ys.min(); val maxY = ys.max()
    val span = maxOf(maxX - minX, maxY - minY, 1e-6)
    val ok = Color(0xFF2E9D58); val bad = Color(0xFFD1453B); val line = MaterialTheme.colorScheme.outlineVariant
    Canvas(Modifier.fillMaxWidth().height(200.dp).padding(top = 8.dp)) {
        val pad = 14.dp.toPx()
        val w = size.width - 2 * pad; val h = size.height - 2 * pad
        fun at(i: Int) = Offset(pad + ((xs[i] - minX) / span * w).toFloat(), pad + ((ys[i] - minY) / span * h).toFloat())
        for (i in 1 until pts.size) drawLine(line, at(i - 1), at(i), strokeWidth = 2f)
        for (i in pts.indices) drawCircle(if (pts[i].state == PingTest.State.OK) ok else bad, radius = 5.dp.toPx(), center = at(i))
    }
}

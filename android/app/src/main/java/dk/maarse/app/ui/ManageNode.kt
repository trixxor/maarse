package dk.maarse.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dk.maarse.app.Neighbor
import dk.maarse.app.Node
import dk.maarse.app.RadioCfg
import dk.maarse.app.RemoteAdmin
import dk.maarse.app.StatusSource
import kotlinx.coroutines.delay

/** What a stat key is called, and how its value reads. The same words as the PC client. */
private val STAT_NAMES: Map<Int, Pair<String, (Int) -> String>> = mapOf(
    1 to ("Uptime" to ::duration), 7 to ("Role" to { v -> dk.maarse.app.Names.ROLES.getOrNull(v) ?: "$v" }),
    10 to ("Packets received" to Int::toString), 11 to ("Invalid packets" to Int::toString), 12 to ("Duplicates" to Int::toString),
    13 to ("Forwarded for others" to Int::toString), 14 to ("Originated" to Int::toString),
    32 to ("Radio packets sent" to Int::toString), 33 to ("Transmit errors" to Int::toString),
    40 to ("Total airtime" to { v -> if (v < 10000) "$v ms" else "%.1f s".format(v / 1000f) }),
    41 to ("Duty cycle used (last hour)" to { v -> "%.1f %%".format(v / 10f) }),
    44 to ("Transmit queue" to Int::toString), 34 to ("Latest RSSI" to { v -> "$v dBm" }), 35 to ("Latest SNR" to { v -> "$v dB" }),
    80 to ("Noise floor (estimated)" to { v -> "$v dBm" }), 49 to ("Battery" to { v -> "$v mV" }), 85 to ("Battery (approx.)" to { v -> "$v %" }),
    86 to ("Chip temperature" to { v -> "%.1f °C".format(v / 10f) }), 90 to ("Crashes" to Int::toString),
    82 to ("USB power" to { v -> if (v < 0) "unknown" else if (v != 0) "yes" else "no" }),
    118 to ("Radio change on trial" to { v -> if (v > 0) "${v / 60} min ${v % 60} s left to confirm" else "None" }),
)

private fun duration(v: Int): String { val h = v / 3600; val m = v % 3600 / 60; return if (h > 0) "$h h $m min" else "$m min ${v % 60} s" }

private val PRESETS = listOf(
    "Må:rse default (EU, 869.525 MHz)" to RadioCfg(869_525_000, 125_000, 9, 5, 22, 100, 16, 0),
    "Long range (EU, SF11)" to RadioCfg(869_525_000, 125_000, 11, 5, 22, 100, 16, 0),
    "Fast (EU, SF7, 250 kHz)" to RadioCfg(869_525_000, 250_000, 7, 5, 22, 100, 16, 0),
)

/**
 * Manage another node you are an administrator of: status (short or full), neighbours and radio settings. The same
 * page as Nodes > Manage on the PC client.
 */
@Composable
fun ManageNodeScreen(addr: String, onBack: () -> Unit) {
    LaunchedEffect(addr) { RemoteAdmin.open(addr) }
    val stats by RemoteAdmin.stats.collectAsState()
    val full by RemoteAdmin.full.collectAsState()
    val neighbors by RemoteAdmin.neighbors.collectAsState()
    val message by RemoteAdmin.message.collectAsState()
    var details by remember(addr) { mutableStateOf(false) }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            Text("Manage " + Node.label(addr), style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold)
        }
        Hint("You must be an administrator of this node. Over LoRa every byte costs airtime, so the short status asks for a few numbers in one reply.")
        if (message.isNotEmpty()) Hint(message)

        GroupTitle("Status")
        if (stats.isNotEmpty()) {
            BatteryCard(stats, StatusSource.REMOTE)
            InShortCard(stats)
        }
        Card2 {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                (if (!full) Button(onClick = { RemoteAdmin.setFull(false) }) { Text("Short status") }
                else OutlinedButton(onClick = { RemoteAdmin.setFull(false) }) { Text("Short status") })
                (if (full) Button(onClick = { RemoteAdmin.setFull(true) }) { Text("Full status") }
                else OutlinedButton(onClick = { RemoteAdmin.setFull(true) }) { Text("Full status") })
            }
            if (stats.isEmpty()) Text("Waiting for a reply…", color = MaterialTheme.colorScheme.onSurfaceVariant)
            else {
                // Everything technical is folded away.
                TextButton(onClick = { details = !details }) { Text(if (details) "Hide details" else "Details") }
                if (details) stats.toSortedMap().forEach { (k, v) ->
                    val (name, show) = STAT_NAMES[k] ?: ("Key $k" to Int::toString)
                    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                        Text(name, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium)
                        Text(show(v), fontWeight = FontWeight.Medium, style = MaterialTheme.typography.bodyMedium)
                    }
                }
            }
            OutlinedButton(onClick = { RemoteAdmin.fetch() }, Modifier.heightIn(min = 48.dp)) { Text("Fetch again") }
        }

        GroupTitle("Neighbours")
        Card2 {
            Text("Who this node hears, with signal strength (one reply, the 8 heard most recently).", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            neighbors?.let { list ->
                if (list.isEmpty()) Text("None heard yet.")
                list.forEach { n -> Text(neighbourLine(n)) }
            }
            OutlinedButton(onClick = { RemoteAdmin.askNeighbors() }, Modifier.heightIn(min = 48.dp)) { Text("Ask for the neighbours") }
        }

        GroupTitle("Radio")
        RadioPart(addr)

        val blePin by RemoteAdmin.blePin.collectAsState()
        BleCodeCard(blePin) { RemoteAdmin.setBlePin(it) }
    }
}

private fun neighbourLine(n: Neighbor): String {
    val ago = when { n.ageS < 60 -> "${n.ageS} s ago"; n.ageS < 3600 -> "${n.ageS / 60} min ago"; else -> "${n.ageS / 3600} h ago" }
    val who = Node.nodes.value.entries.firstOrNull { it.key.startsWith(n.prefix) }?.key?.let { Node.label(it) } ?: n.prefix
    return "$who · SNR ${if (n.snr > 0) "+" else ""}${n.snr} dB" + (if (n.rssi != 0) " · ${n.rssi} dBm" else "") + " · $ago · ${n.count} packets"
}

@Composable
private fun RadioPart(addr: String) {
    val radio by RemoteAdmin.radio.collectAsState()
    val trial by RemoteAdmin.trial.collectAsState()
    val c = radio
    var preset by remember { mutableStateOf<RadioCfg?>(null) }
    var freq by remember(c) { mutableStateOf(c?.let { "%.3f".format(it.freq / 1e6) } ?: "") }
    var sf by remember(c) { mutableStateOf(c?.sf?.toString() ?: "") }
    var bw by remember(c) { mutableStateOf(c?.let { (it.bw / 1000.0).toString().removeSuffix(".0") } ?: "") }
    var power by remember(c) { mutableStateOf(c?.power?.toString() ?: "") }
    var ask by remember { mutableStateOf<RadioCfg?>(null) }
    val t = trial?.takeIf { it.target == addr }

    if (t != null) TrialCard(t)
    Card2 {
        if (c == null) { Text("Waiting for the radio settings…", color = MaterialTheme.colorScheme.onSurfaceVariant); return@Card2 }
        Text("All Må:rse radios in a network must use the same frequency, bandwidth and spreading factor, or they cannot hear each other. " +
            "A change here is only a trial until you confirm it.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        PRESETS.forEach { (name, p) ->
            TextButton(onClick = { freq = "%.3f".format(p.freq / 1e6); sf = p.sf.toString(); bw = (p.bw / 1000).toString(); power = p.power.toString() }) { Text(name) }
        }
        OutlinedTextField(freq, { freq = it }, Modifier.fillMaxWidth(), label = { Text("Frequency (MHz)") }, singleLine = true)
        OutlinedTextField(sf, { sf = it }, Modifier.fillMaxWidth(), label = { Text("Spreading factor (5-12)") }, singleLine = true)
        OutlinedTextField(bw, { bw = it }, Modifier.fillMaxWidth(), label = { Text("Bandwidth (kHz)") }, singleLine = true)
        OutlinedTextField(power, { power = it }, Modifier.fillMaxWidth(), label = { Text("Transmit power (dBm)") }, singleLine = true)
        Button(onClick = {
            val f = freq.toDoubleOrNull(); val s = sf.toIntOrNull(); val b = bw.toDoubleOrNull(); val p = power.toIntOrNull()
            if (f != null && s != null && b != null && p != null) ask = c.copy(freq = Math.round(f * 1e6), sf = s, bw = Math.round(b * 1000), power = p)
        }, Modifier.heightIn(min = 48.dp)) { Text("Try on the node") }
    }
    ask?.let { cfg ->
        AlertDialog(onDismissRequest = { ask = null }, title = { Text("Try these settings?") },
            text = { Text("The node tries the new radio settings for ${RemoteAdmin.TRIAL_MINUTES} minutes. It saves them only if you confirm in time " +
                "(your own radio must hear it on the new settings); otherwise it goes back.") },
            confirmButton = { TextButton(onClick = { ask = null; RemoteAdmin.tryRadio(cfg) }) { Text("Try") } },
            dismissButton = { TextButton(onClick = { ask = null }) { Text("Cancel") } })
    }
}

/** A radio change on trial: countdown, "switch my radio too" and the confirmation. */
@Composable
private fun TrialCard(t: RemoteAdmin.Trial) {
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(t.phase) { while (true) { now = System.currentTimeMillis(); delay(1000) } }
    var mine by remember { mutableStateOf(false) }
    val left = ((t.until - now) / 1000).coerceAtLeast(0)
    Card2(Modifier.padding(bottom = 8.dp)) {
        when (t.phase) {
            "saved" -> Text("Saved. The node confirmed the new radio settings.", fontWeight = FontWeight.SemiBold)
            "expired" -> Text("Not confirmed. The node goes back to its old settings (or already has). Fetch the status again to see which it uses.", fontWeight = FontWeight.SemiBold)
            else -> {
                Text("On trial. The node uses the new settings but has not saved them. Time left: ${left / 60}:${"%02d".format(left % 60)}" +
                    (if (t.phase == "confirming") ". Confirming, trying every 15 seconds…" else "."), fontWeight = FontWeight.SemiBold)
                Text("Make sure your own radio hears it on the new settings, then confirm.", style = MaterialTheme.typography.bodySmall)
                FilledTonalButton(onClick = { mine = true }, Modifier.heightIn(min = 48.dp)) { Text("Switch my radio to these settings too") }
                Button(onClick = { RemoteAdmin.confirm() }, Modifier.heightIn(min = 48.dp)) { Text(if (t.phase == "confirming") "Confirm again" else "Confirm and save") }
            }
        }
    }
    if (mine) AlertDialog(onDismissRequest = { mine = false }, title = { Text("Switch your radio too?") },
        text = { Text("This saves the same radio settings on your own radio, so it can hear the node.") },
        confirmButton = { TextButton(onClick = { mine = false; Node.send(dk.maarse.app.Cmd.setRadio(t.cfg)) }) { Text("Switch") } },
        dismissButton = { TextButton(onClick = { mine = false }) { Text("Cancel") } })
}

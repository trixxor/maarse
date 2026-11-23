package dk.maarse.app.ui

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dk.maarse.app.Node
import dk.maarse.app.NodeEntry

/** Small helpers shared by Nodes, Mesh, Map and Sensors (Kit.kt belongs to the shell work). */

/** "1 link", "2 links". */
fun plural(n: Int, one: String, many: String = one + "s") = "$n " + if (n == 1) one else many

/** "3 hops", "1 hop", "Nearby". */
fun hopsText(h: Int?): String? = h?.let { if (it == 0) "Nearby" else plural(it, "hop") }

/** The sensor colour: teal. Red is alarm, amber is quiet, green is fine. */
object SensorColors {
    private val light = Color(0xFF1A7F86)
    private val dark = Color(0xFF5CC8D0)
    val teal: Color @Composable get() = if (androidx.compose.foundation.isSystemInDarkTheme()) dark else light
    val tealContainer: Color @Composable get() = if (androidx.compose.foundation.isSystemInDarkTheme()) Color(0xFF16363A) else Color(0xFFDDF0F1)
}

/** A row of filter chips that scrolls sideways instead of wrapping. `violet` marks the MeshCore chip. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ChipRow(items: List<String>, selected: Int, onSelect: (Int) -> Unit, violet: Int = -1, modifier: Modifier = Modifier) {
    Row(modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        items.forEachIndexed { i, label ->
            val mc = i == violet
            FilterChip(
                selected = i == selected, onClick = { onSelect(i) }, label = { Text(label, maxLines = 1) },
                colors = if (mc) FilterChipDefaults.filterChipColors(selectedContainerColor = Mc.container, selectedLabelColor = Mc.onContainer) else FilterChipDefaults.filterChipColors(),
            )
        }
    }
}

/** "5 min ago", "2 h ago" from a time in epoch seconds (MeshCore) or milliseconds. */
fun agoText(ms: Long): String {
    if (ms <= 0) return "not heard yet"
    val s = (System.currentTimeMillis() - ms) / 1000
    return when {
        s < 60 -> "just now"
        s < 3600 -> "${s / 60} min ago"
        s < 86400 -> "${s / 3600} h ago"
        else -> "${s / 86400} d ago"
    }
}

/**
 * What a Må:rse node is. The announce carries no role yet, so a node with a name
 * (a person's phone or PC, or one the user named) is a person and one without is a repeater.
 */
fun isPerson(n: NodeEntry): Boolean = n.name != null || Node.nicknames.value.containsKey(n.addr)

/** Signal in dB to a node we hear directly, from the neighbour list (null when not a neighbour). */
fun snrOf(addr: String): Int? = Node.neighbors.value.firstOrNull { addr.startsWith(it.prefix) }?.snr

/** The MeshCore actions of a header: Advert and Scan now, shown when a MeshCore radio is connected. */
@Composable
fun MeshCoreActions(modifier: Modifier = Modifier) {
    val mc by Node.meshcoreAccess.collectAsState()
    var advert by remember { mutableStateOf(false) }
    val border = BorderStroke(1.dp, Mc.color)
    Row(modifier, horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
        OutlinedButton(onClick = { advert = true }, border = border, modifier = Modifier.heightIn(min = 48.dp), shape = RoundedCornerShape(24.dp)) {
            Text("Advert", color = Mc.color, fontWeight = FontWeight.SemiBold, maxLines = 1)
        }
        OutlinedButton(
            onClick = { McNav.scanAskedAt = System.currentTimeMillis(); Node.mcAsk("scan"); Node.mcAsk("nodes") },
            enabled = !mc.scan.waiting && !mc.scan.active, border = border, modifier = Modifier.heightIn(min = 48.dp), shape = RoundedCornerShape(24.dp),
        ) { Text("Scan now", color = if (mc.scan.waiting || mc.scan.active) MaterialTheme.colorScheme.onSurfaceVariant else Mc.color, fontWeight = FontWeight.SemiBold, maxLines = 1) }
    }
    if (advert) AlertDialog(
        onDismissRequest = { advert = false },
        title = { Text("Send an advert") },
        text = {
            Text("An advert tells other nodes that you exist. A zero-hop advert is heard only by nodes in direct radio range. A flood advert is passed on by repeaters, so far more nodes hear it, but it uses more airtime.")
        },
        confirmButton = {
            Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.End) {
                TextButton(onClick = { advert = false; Node.mcAsk("announce-flood"); McNav.advertNote = "Flood advert sent. Repeaters pass it on." }) { Text("Flood advert", color = Mc.color) }
                TextButton(onClick = { advert = false; Node.mcAsk("announce"); McNav.advertNote = "Zero-hop advert sent. Nodes in direct range hear it." }) { Text("Zero-hop advert", color = Mc.color) }
                TextButton(onClick = { advert = false }) { Text("Cancel") }
            }
        },
    )
}

/** A rounded surface for a sub-section with an even inner padding. */
@Composable
fun Panel(modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    Column(
        modifier.padding(horizontal = 16.dp).fillMaxWidth()
            .background(MaterialTheme.colorScheme.surfaceContainer, RoundedCornerShape(16.dp)).padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) { content() }
}

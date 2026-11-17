package dk.maarse.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Tab
import androidx.compose.material3.TabRow
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dk.maarse.app.Cmd
import dk.maarse.app.Mode
import dk.maarse.app.Names
import dk.maarse.app.Node
import dk.maarse.app.STATS
import dk.maarse.app.STAT_GROUPS
import dk.maarse.app.StatKey
import dk.maarse.app.formatStat

@Composable
fun NetworkScreen() {
    var tab by remember { mutableIntStateOf(0) }
    DisposableEffect(Unit) {
        Node.fastPoll = true
        Node.send(Cmd.getStats()); Node.send(Cmd.getNeighbors())
        onDispose { Node.fastPoll = false }
    }
    Column(Modifier.fillMaxSize()) {
        TabRow(selectedTabIndex = tab) {
            listOf("Overview", "All numbers", "Alerts", "Log").forEachIndexed { i, t -> Tab(tab == i, { tab = i }, text = { Text(t) }) }
        }
        when (tab) {
            0 -> Overview()
            1 -> AllStats()
            2 -> Alerts()
            3 -> LogView()
        }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Overview() {
    val s by Node.stats.collectAsState()
    val me by Node.me.collectAsState()
    val nb by Node.neighbors.collectAsState()
    val conn by Node.conn.collectAsState()
    Column(Modifier.verticalScroll(rememberScrollState())) {
        if (conn.mode == Mode.SERVER) { ServerPanel(); return@Column }
        if (s.isEmpty()) { Hint(if (me == null) "Not connected." else "Fetching…"); return@Column }
        FlowRow(Modifier.padding(8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            s[7]?.let { Tile("Role", Names.ROLES.getOrElse(it) { "$it" }, if ((s[98] ?: 0) != 0) "mobile mode: passing repeaters quickly" else null) }
            s[1]?.let { Tile("Uptime", formatStat(1, it)) }
            s[83]?.let { Tile("Neighbours", "$it", "heard directly") }
            s[2]?.let { Tile("Known nodes", "$it", s[97]?.let { p -> "$p pinned (kept in flash)" }) }
            s[41]?.let { Tile("Airtime", formatStat(41, it), "of the 10 % allowed per hour") }
            s[96]?.let { Tile("Carrying", "$it", "messages carried for others (store-carry-forward)") }
            s[61]?.let { Tile("Post office", "$it stored", s[60]?.let { r -> "for $r recipients" }) }
            s[69]?.let { Tile("Deposited", "$it", "put in a post office for offline recipients") }
            s[70]?.let { Tile("From post office", "$it", "delivered when the recipient came back") }
            s[101]?.let { Tile("Outbox", "$it", s[100]?.takeIf { w -> w > 0 }?.let { w -> "$w waiting for the recipient's key" }) }
            s[5]?.let { Tile("Backbone links", "$it", s[72]?.let { d -> "$d DHT contacts" }) }
            s[85]?.let { Tile("Battery", "$it %", s[82]?.let { u -> if (u > 0) "on USB power" else null }) }
            s[90]?.takeIf { it > 0 }?.let { Tile("Crashes", "$it", "since the counter was reset") }
        }
        if (nb.isNotEmpty()) {
            SectionTitle("Neighbours (heard directly)")
            nb.sortedByDescending { it.snr }.forEach { n ->
                Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 2.dp)) {
                    Text(Node.nodes.value.keys.firstOrNull { it.startsWith(n.prefix) }?.let { Node.label(it) } ?: n.prefix, Modifier.weight(1f))
                    Text("SNR ${n.snr} dB · ${n.rssi} dBm · ${n.count}×", style = MaterialTheme.typography.bodySmall)
                }
            }
        }
    }
}

@Composable
private fun Tile(title: String, value: String, sub: String? = null) {
    Card(Modifier.width(170.dp)) {
        Column(Modifier.padding(12.dp)) {
            Text(title, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text(value, style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold)
            sub?.let { Text(it, style = MaterialTheme.typography.labelSmall) }
        }
    }
}

@Composable
private fun AllStats() {
    val s by Node.stats.collectAsState()
    Column(Modifier.verticalScroll(rememberScrollState()).padding(bottom = 16.dp)) {
        if (s.isEmpty()) Hint("No numbers yet.")
        val groups = s.keys.groupBy { (STATS[it] ?: StatKey("Other", "Key $it")).group }
        (STAT_GROUPS + "Other").forEach { g ->
            val keys = groups[g] ?: return@forEach
            SectionTitle(g)
            keys.sorted().forEach { k ->
                Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 2.dp)) {
                    Text(STATS[k]?.label ?: "Key $k", Modifier.weight(1f))
                    Text(formatStat(k, s[k] ?: 0))
                }
            }
        }
    }
}

@Composable
private fun Alerts() {
    val list by Node.alerts.collectAsState()
    Column(Modifier.verticalScroll(rememberScrollState())) {
        Hint("Alerts are signed by trusted issuers and checked by maarse-host. A radio over Bluetooth does not check them yet, so they only appear here with the phone as node or a host.")
        if (list.isEmpty()) Hint("No alerts received.")
        list.reversed().forEach { AlertCard(it) }
    }
}

@Composable
private fun LogView() {
    val l by Node.log.collectAsState()
    Column(Modifier.verticalScroll(rememberScrollState()).padding(8.dp)) {
        l.reversed().forEach { Text(it, style = MaterialTheme.typography.bodySmall) }
    }
}

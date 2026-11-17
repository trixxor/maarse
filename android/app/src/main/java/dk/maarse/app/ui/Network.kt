package dk.maarse.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dk.maarse.app.Cmd
import dk.maarse.app.Conn
import dk.maarse.app.Mode
import dk.maarse.app.Names
import dk.maarse.app.Node
import dk.maarse.app.STATS
import dk.maarse.app.STAT_GROUPS
import dk.maarse.app.StatKey
import dk.maarse.app.formatStat

/** Pages behind the Network tab's "More". */
enum class NetPage(val title: String) { ALERTS("Alerts"), NEIGHBOURS("Neighbours and routes"), NUMBERS("All numbers"), LOG("Log") }

/** The network in plain words first, a few numbers, then the details. */
@Composable
fun NetworkScreen(onPage: (NetPage) -> Unit) {
    val s by Node.stats.collectAsState()
    val conn by Node.conn.collectAsState()
    val alerts by Node.alerts.collectAsState()
    DisposableEffect(Unit) {
        Node.fastPoll = true
        Node.send(Cmd.getStats()); Node.send(Cmd.getNeighbors())
        onDispose { Node.fastPoll = false }
    }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 16.dp)) {
        if (conn.mode == Mode.SERVER) { ServerPanel(); return@Column }
        val online = conn.state == Conn.State.ONLINE
        val near = s[83] ?: 0
        val links = s[5] ?: 0
        val (headline, color) = when {
            !online -> "Not connected" to MaterialTheme.colorScheme.onSurfaceVariant
            near == 0 && links == 0 -> "Connected, but nobody in reach yet" to Amber
            else -> "All good" to MaterialTheme.colorScheme.primary
        }
        val story = when {
            !online -> "Tap the line at the top of Chats to choose how this phone connects."
            else -> buildList {
                if (links > 0) add("You reach others over the internet ($links link${if (links == 1) "" else "s"})")
                if (near > 0) add("$near radio${if (near == 1) "" else "s"} heard nearby")
                if (links == 0 && near == 0) add("No radio heard and no internet link yet. Messages wait and go when someone is in reach")
                s[7]?.let { add("this node works as ${Names.ROLES.getOrElse(it) { "?" }.lowercase()}") }
            }.joinToString(". ") + "."
        }
        Card2 {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.size(10.dp).clip(CircleShape).background(color))
                Spacer(Modifier.width(8.dp))
                Text(headline, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
            }
            Text(story, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        if (online && s.isNotEmpty()) {
            Column(Modifier.padding(horizontal = 16.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Tile(Modifier.weight(1f), "Radio airtime", s[41]?.let { formatStat(41, it) } ?: "–", "of the 10 % allowed")
                    Tile(Modifier.weight(1f), "Post office", "${s[61] ?: 0} kept", s[60]?.takeIf { it > 0 }?.let { "for $it people" } ?: "mail for people away")
                }
                Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Tile(Modifier.weight(1f), "Carrying", "${s[96] ?: 0}", "messages for others")
                    Tile(Modifier.weight(1f), "Backbone", "$links links", s[72]?.let { "$it DHT contacts" } ?: "internet links")
                }
            }
        }
        GroupTitle("More")
        Group {
            GroupRow("Alerts", if (alerts.isEmpty()) "None received" else "${alerts.size} received") { onPage(NetPage.ALERTS) }
            GroupRow("Neighbours and routes", "Who you hear, and how well") { onPage(NetPage.NEIGHBOURS) }
            GroupRow("All numbers", "Every counter the node keeps") { onPage(NetPage.NUMBERS) }
            GroupRow("Log", "What happened, live", last = true) { onPage(NetPage.LOG) }
        }
    }
}

@Composable
private fun Tile(modifier: Modifier, title: String, value: String, sub: String) {
    Column(
        modifier.clip(RoundedCornerShape(16.dp)).background(MaterialTheme.colorScheme.surfaceContainer).padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        Text(title, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(value, style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold)
        Text(sub, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
fun NetPageContent(p: NetPage) {
    when (p) {
        NetPage.ALERTS -> AlertsPage()
        NetPage.NEIGHBOURS -> NeighboursPage()
        NetPage.NUMBERS -> AllStats()
        NetPage.LOG -> LogView()
    }
}

@Composable
private fun NeighboursPage() {
    val nb by Node.neighbors.collectAsState()
    DisposableEffect(Unit) {
        Node.fastPoll = true
        Node.send(Cmd.getNeighbors())
        onDispose { Node.fastPoll = false }
    }
    Column(Modifier.verticalScroll(rememberScrollState())) {
        if (nb.isEmpty()) Hint("No radios heard directly yet.")
        else Group {
            nb.sortedByDescending { it.snr }.forEachIndexed { i, n ->
                val addr = Node.nodes.value.keys.firstOrNull { it.startsWith(n.prefix) }
                val title = addr?.let { Node.label(it) } ?: n.prefix
                Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    Avatar(title, addr ?: n.prefix, 36.dp)
                    Column(Modifier.weight(1f)) {
                        Text(title, fontWeight = FontWeight.Medium)
                        Text("Signal ${n.snr} dB · ${n.rssi} dBm · heard ${n.count}×", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
                if (i < nb.size - 1) androidx.compose.material3.HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
            }
        }
        Hint("Trace a route from a contact's page.")
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
            GroupTitle(g)
            Group {
                keys.sorted().forEach { k ->
                    Row(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 8.dp)) {
                        Text(STATS[k]?.label ?: "Key $k", Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium)
                        Text(formatStat(k, s[k] ?: 0), style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Medium)
                    }
                }
            }
        }
    }
}

@Composable
private fun AlertsPage() {
    val list by Node.alerts.collectAsState()
    Column(Modifier.verticalScroll(rememberScrollState())) {
        Hint("Alerts are signed by trusted issuers and checked by maarse-host (the phone node or your computer). A radio alone does not check them yet.")
        if (list.isEmpty()) Hint("No alerts received.")
        list.reversed().forEach { AlertCard(it) }
    }
}

@Composable
private fun LogView() {
    val l by Node.log.collectAsState()
    Column(Modifier.verticalScroll(rememberScrollState()).padding(16.dp)) {
        l.reversed().forEach { Text(it, style = MaterialTheme.typography.bodySmall, modifier = Modifier.padding(vertical = 1.dp)) }
    }
}

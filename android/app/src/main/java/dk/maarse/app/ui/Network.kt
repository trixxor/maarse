package dk.maarse.app.ui

import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.TextButton
import androidx.compose.material3.OutlinedButton

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
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

/** Pages behind the Mesh place's Details list and backbones. */
enum class NetPage(val title: String) { BACKBONE("Backbone"), ALERTS("Alerts"), NEIGHBOURS("Neighbours and routes"), NUMBERS("All numbers"), LOG("Log"), PAGES("Pages"), ANNOUNCES("Announces") }

/**
 * Mesh: one honest sentence first, then only the tiles that apply, the airtime bar when a radio is in use,
 * backbones, the MeshCore status card and the details. `onConnect` opens how this phone connects (optional).
 */
@Composable
fun NetworkScreen(onPage: (NetPage) -> Unit, onConnect: (() -> Unit)? = null) {
    val s by Node.stats.collectAsState()
    val conn by Node.conn.collectAsState()
    val alerts by Node.alerts.collectAsState()
    DisposableEffect(Unit) {
        Node.fastPoll = true
        Node.send(Cmd.getStats()); Node.send(Cmd.getNeighbors())
        if (dk.maarse.app.Calls.possible()) Node.send(Cmd.getBackbones())
        onDispose { Node.fastPoll = false }
    }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(top = 4.dp, bottom = 24.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        if (conn.mode == Mode.SERVER) { ServerPanel(); return@Column }
        val person by Node.person.collectAsState()
        val online = conn.state == Conn.State.ONLINE
        // Radios heard: only with a radio (over Bluetooth, or as the phone's modem).
        val hasRadio = conn.mode == Mode.BLE || (conn.mode == Mode.PHONE && dk.maarse.app.ModemBridge.enabled(androidx.compose.ui.platform.LocalContext.current))
        val near = if (hasRadio) s[83] ?: 0 else 0
        val off by Node.offline.collectAsState()
        val links = s[5] ?: 0
        val role = s[7]?.let { Names.ROLES.getOrElse(it) { null } }?.lowercase()
        // The dot, the headline and the sentence always agree.
        val (headline, color) = when {
            !online -> "Not connected" to MaterialTheme.colorScheme.onSurfaceVariant
            off -> "No connection" to Amber
            near == 0 && links == 0 -> "Connected, nobody in reach yet" to Amber
            links > 0 && !hasRadio -> "Online, no radio in use" to MaterialTheme.colorScheme.primary
            links > 0 && near == 0 -> "Online, no radio heard" to MaterialTheme.colorScheme.primary
            links > 0 -> "Online, radios nearby" to MaterialTheme.colorScheme.primary
            else -> "Radios nearby, no internet link" to MaterialTheme.colorScheme.primary
        }
        val story = when {
            !online -> "Choose how this phone connects."
            off -> "The phone has no internet" + (if (hasRadio) "" else " and no radio") + ". Messages you write wait and go out when you are back online."
            else -> buildList {
                if (links > 0) add("You reach others over the internet (${plural(links, "link")})")
                if (near > 0) add("${plural(near, "radio")} heard nearby over LoRa")
                else if (hasRadio) add("No radio heard nearby yet")
                if (!hasRadio) add("LoRa is not used. Add a radio to reach people nearby without internet")
                if (links == 0 && near == 0) add("No internet link and no radio heard yet. Messages wait and go when someone is in reach")
                role?.let { add("this node works as $it") }
            }.joinToString(". ") + "."
        }
        Card2 {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.size(10.dp).clip(CircleShape).background(color))
                Spacer(Modifier.width(8.dp))
                Text(headline, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
            }
            Text(story, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Row(Modifier.padding(top = 6.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                if (online) FilledTonalButton(onClick = { Node.send(Cmd.announce()) }, Modifier.heightIn(min = 48.dp)) { Text("Announce now", maxLines = 1) }
                if (!online || !hasRadio) onConnect?.let { OutlinedButton(onClick = it, Modifier.heightIn(min = 48.dp)) { Text(if (online) "Add a radio" else "Connection", maxLines = 1) } }
            }
        }
        if (online && hasRadio && s.isNotEmpty()) {
            val permille = s[41] ?: 0
            Card2 {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("Airtime, last hour", Modifier.weight(1f), fontWeight = FontWeight.SemiBold)
                    Text(formatStat(41, permille) + " of 10 %", style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                LinearProgressIndicator(
                    progress = { (permille / 100f).coerceIn(0f, 1f) }, Modifier.fillMaxWidth().height(8.dp).clip(RoundedCornerShape(99.dp)),
                    color = if (permille > 80) Amber else MaterialTheme.colorScheme.primary, trackColor = MaterialTheme.colorScheme.surfaceContainerHigh,
                )
                Text("The legal limit is 10 %. Nothing is sent when the limit is reached.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
        if (online && s.isNotEmpty()) {
            // Only the tiles that apply.
            val tiles = buildList<@Composable (Modifier) -> Unit> {
                add { m -> Tile(m, "Post office", "${s[61] ?: 0} kept", s[60]?.takeIf { it > 0 }?.let { "for ${plural(it, "person", "people")}" } ?: "mail for people away") }
                if (links > 0) add { m -> Tile(m, "Backbone", plural(links, "link"), s[72]?.let { "$it DHT contacts" } ?: "internet links") }
                if ((s[96] ?: 0) > 0 || role == "relay") add { m -> Tile(m, "Carrying", "${s[96] ?: 0}", "messages for others") }
                if (hasRadio) add { m -> Tile(m, "Radio", if (near > 0) plural(near, "radio") else "none heard", "nearby over LoRa") }
            }
            Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                tiles.chunked(2).forEach { row ->
                    Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                        row.forEach { it(Modifier.weight(1f)) }
                        if (row.size == 1) Spacer(Modifier.weight(1f))
                    }
                }
                if (person?.state == dk.maarse.app.PersonState.MEMBER) TextButton(onClick = { Node.personSetup(dk.maarse.app.PersonOp.SYNC) }, Modifier.heightIn(min = 48.dp)) { Text("Sync now") }
            }
        }
        MeshCoreCard()
        val bbs by Node.backbones.collectAsState()
        if (bbs.isNotEmpty()) {
            GroupTitle("Backbones")
            Group {
                bbs.forEachIndexed { i, b ->
                    GroupRow(backboneName(b), backboneSummary(b), last = i == bbs.lastIndex) { selectedBackbone = b.link; onPage(NetPage.BACKBONE) }
                }
            }
        }
        GroupTitle("Details")
        Group {
            GroupRow("Neighbours and routes", "Who you hear, and how well") { onPage(NetPage.NEIGHBOURS) }
            GroupRow("Alerts", if (alerts.isEmpty()) "None received" else "${alerts.size} received") { onPage(NetPage.ALERTS) }
            GroupRow("All numbers", "Every counter the node keeps") { onPage(NetPage.NUMBERS) }
            GroupRow("Log", "What happened, live", last = true) { onPage(NetPage.LOG) }
        }
    }
}

/** The MeshCore status card: shown once a MeshCore radio is set up. A marker, not a theme. */
@Composable
private fun MeshCoreCard() {
    val link by Node.mcLink.collectAsState()
    val mc by Node.meshcoreAccess.collectAsState()
    if (link.state == 0 || !mc.allowed) return
    val ready = link.state == 2
    Card2 {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("MeshCore", color = Mc.color, fontWeight = FontWeight.SemiBold, style = MaterialTheme.typography.titleMedium)
            NetworkTag("separate network")
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Box(Modifier.size(10.dp).clip(CircleShape).background(if (ready) MaterialTheme.colorScheme.primary else if (link.state == 1) Amber else MaterialTheme.colorScheme.onSurfaceVariant))
            Spacer(Modifier.width(8.dp))
            Text(if (ready) "Radio connected" else if (link.state == 1) "Connecting to the MeshCore radio" else "MeshCore radio not available", fontWeight = FontWeight.SemiBold)
        }
        if (ready) {
            val repeaters = mc.nodes.count { it.type == 2 }
            Text("Hears ${plural(mc.nodes.size, "node")}, ${plural(repeaters, "repeater")} of them. Messages go out as floods until a path is learned.",
                style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            MeshCoreActions(Modifier.padding(top = 4.dp))
        } else if (link.note.isNotBlank()) Text(link.note, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
private fun Tile(modifier: Modifier, title: String, value: String, sub: String) {
    Column(
        modifier.clip(RoundedCornerShape(16.dp)).background(MaterialTheme.colorScheme.surfaceContainer).padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        Text(title, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(value, style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold, maxLines = 1)
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
        NetPage.BACKBONE -> BackbonePage()
        NetPage.PAGES -> PagesPage()
        NetPage.ANNOUNCES -> AnnouncesPage()
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

/** The backbone opened from the Network screen (its link id). */
private var selectedBackbone = 0

private fun backboneName(b: dk.maarse.app.Ev.Backbone) =
    b.name.removePrefix("tcp out ").removePrefix("tcp in ").substringBeforeLast(":")

private fun upFor(s: Long) = when {
    s < 120 -> "$s s"
    s < 7200 -> "${s / 60} min"
    s < 172800 -> "${s / 3600} h"
    else -> "${s / 86400} days"
}

private fun backboneSummary(b: dk.maarse.app.Ev.Backbone) = buildList {
    add(if (b.flags and 1 != 0) "server" else "host")
    if (b.flags and 4 != 0) add("post office")
    if (b.flags and 8 != 0) add("relay")
    add("up " + upFor(b.upSecs))
}.joinToString(" · ")

/** One backbone: what it is, how well we reach it, and tests. */
@Composable
private fun BackbonePage() {
    val bbs by Node.backbones.collectAsState()
    val rtt by Node.traceRtt.collectAsState()
    val traces by Node.traces.collectAsState()
    val speed by dk.maarse.app.Calls.speed.collectAsState()
    DisposableEffect(Unit) { Node.send(Cmd.getBackbones()); onDispose { } }
    val b = bbs.firstOrNull { it.link == selectedBackbone } ?: run {
        Text("The link is down.", Modifier.padding(24.dp)); return
    }
    val known = b.addr.isNotEmpty() && b.addr.trim('0').isNotEmpty()
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
        Card2 {
            Text(backboneName(b), style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
            Text(b.name, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        GroupTitle("Info")
        Card2 {
            val roles = buildList {
                add(if (b.flags and 1 != 0) "Server (dedicated backbone)" else "Host")
                if (b.flags and 2 != 0) add("has a radio")
                if (b.flags and 4 != 0) add("post office (keeps mail for people away)")
                if (b.flags and 8 != 0) add("relay (for hosts behind a router)")
            }
            Text("Version ${b.version.ifEmpty { "unknown" }}")
            Text(roles.joinToString(", "))
            Text("Connected for ${upFor(b.upSecs)}")
            Text("Regions: ${if (b.regions == 0) "none set" else b.regions}")
            if (known) Text("Address ${b.addr.take(8)}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        if (!known) { Text("Its address is not heard yet; tests come when it has announced itself.", Modifier.padding(16.dp)); return@Column }
        GroupTitle("Route and response time")
        Card2 {
            val t = traces.firstOrNull { it.dest == b.addr }
            Text(rtt[b.addr]?.let { "Round trip ${it} ms" + (t?.let { tr -> " · ${tr.path.size} hop${if (tr.path.size == 1) "" else "s"}" } ?: "") } ?: "Not measured yet")
            TextButton(onClick = { Node.trace(b.addr) }) { Text("Measure") }
        }
        if (b.flags and 16 != 0) {
            GroupTitle("Call test")
            Card2 {
                Text("Hear how calls sound through this backbone. The call screen shows lost packets.", style = MaterialTheme.typography.bodySmall)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    FilledTonalButton(onClick = { dk.maarse.app.Calls.startTest(b.addr, 1) }, Modifier.weight(1f)) { Text("Music") }
                    FilledTonalButton(onClick = { dk.maarse.app.Calls.startTest(b.addr, 2) }, Modifier.weight(1f)) { Text("Echo") }
                }
                Text("Echo: say something; it comes back after one second.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            GroupTitle("Speed test")
            Card2 {
                val kbs = if (speed.ms > 0) speed.bytes * 1000.0 / speed.ms / 1024 else 0.0
                val loss = if (speed.got + speed.lost > 0) 100.0 * speed.lost / (speed.got + speed.lost) else 0.0
                Text(when {
                    speed.running && speed.got == 0 -> speed.note
                    speed.got > 0 -> "%.1f KB/s · %d packets · %.1f %% lost%s".format(kbs, speed.got, loss, if (speed.running) " (measuring…)" else "")
                    speed.note.isNotEmpty() -> speed.note
                    else -> "Five seconds of data from the backbone, as fast as calls may send."
                })
                if (!speed.running && speed.got > 0) Text(verdict(loss), fontWeight = FontWeight.SemiBold)
                TextButton(onClick = { dk.maarse.app.Calls.startTest(b.addr, 3) }, enabled = !speed.running) { Text("Start") }
            }
        }
    }
}

private fun verdict(lossPct: Double) = when {
    lossPct < 1 -> "Good connection"
    lossPct < 5 -> "Fair: calls may crackle now and then"
    else -> "Poor: calls will break up"
}

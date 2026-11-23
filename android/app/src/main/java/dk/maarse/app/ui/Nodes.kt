package dk.maarse.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.Chat
import androidx.compose.material.icons.filled.Call
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Route
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
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
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dk.maarse.app.tr
import dk.maarse.app.Node
import dk.maarse.app.NodeEntry
import dk.maarse.app.Sensors
import dk.maarse.app.shortAddr
import kotlinx.coroutines.delay

private fun reach(n: NodeEntry): String = buildList {
    hopsText(n.hops)?.let { add(it) }
    if (n.lastHeard > 0) add("heard " + time(n.lastHeard))
    n.mv?.let { add("%.1f V".format(it / 1000.0)) }
}.joinToString(" · ")

/** Which part of the Nodes place is open: Nodes, Sensors or Pages. Kept when you go into something and back. */
object NodesNav {
    var seg by mutableIntStateOf(0)
    fun showSensors() { seg = 1 }
    /** Opens the Pages segment (for the Announces list and links that open a page). */
    fun showPages() { seg = 2 }
}

/** The Nodes place: a segment control Nodes | Sensors | Pages. */
@Composable
fun ContactsScreen(onContact: (String) -> Unit, onPages: () -> Unit = {}, onAnnounces: () -> Unit = {}, onMeshcoreChat: () -> Unit = {},
                   onSensor: (String) -> Unit = {}, onSensorNew: () -> Unit = {}) {
    // A MeshCore node opened from the list shows its own page, with its own back.
    if (McNav.node != null) { MeshCoreNodes(onMeshcoreChat); return }
    Column(Modifier.fillMaxSize()) {
        SegmentedTabs(listOf("Nodes", "Sensors", "Pages"), NodesNav.seg, { NodesNav.seg = it; if (it == 1) Sensors.refresh() })
        when (NodesNav.seg) {
            1 -> SensorsPanel(onSensor, onSensorNew)
            2 -> PagesPage(onExit = { dk.maarse.app.Pages.home() })
            else -> NodesList(onContact, onAnnounces)
        }
    }
}

private val FILTERS = listOf("All", "People", "Repeaters", "MeshCore")

/** People and nodes heard, then what the MeshCore radio heard (violet tag). */
@Composable
private fun NodesList(onContact: (String) -> Unit, onAnnounces: () -> Unit) {
    val mcState by Node.meshcoreAccess.collectAsState()
    val link by Node.mcLink.collectAsState()
    val nodes by Node.nodes.collectAsState()
    Node.nicknames.collectAsState().value
    Node.neighbors.collectAsState().value
    val ready = link.state == 2 && mcState.allowed
    val showMc = link.state != 0 && mcState.allowed
    var filter by remember { mutableIntStateOf(0) }
    var query by remember { mutableStateOf("") }
    if (!showMc && filter == 3) filter = 0
    // Ask the MeshCore radio for its nodes while this list is open.
    LaunchedEffect(ready) {
        while (ready) {
            Node.mcAsk("nodes")
            val s = Node.meshcoreAccess.value.scan
            val recent = McNav.scanAskedAt > 0 && System.currentTimeMillis() - McNav.scanAskedAt < 25_000
            delay(if (s.waiting || s.active || recent) 2_000 else 15_000)
        }
    }
    val q = query.trim()
    val sensorNodes = (Sensors.list.collectAsState().value ?: emptyList()).map { it.node }.toSet()
    val marse = nodes.values.filter { it.addr !in sensorNodes }
        .filter { q.isBlank() || Node.label(it.addr).contains(q, true) || it.addr.startsWith(q.lowercase()) }
        .filter { filter == 0 || (filter == 1 && isPerson(it)) || (filter == 2 && kindOf(it) == NodeKind.REPEATER) }
        .sortedWith(compareByDescending<NodeEntry> { Node.nicknames.value.containsKey(it.addr) }.thenByDescending { it.lastHeard })
    val mc = mcState.nodes
        .filter { q.isBlank() || it.name.contains(q, true) || it.key.startsWith(q.lowercase()) }
        .filter { filter == 0 || filter == 3 || (filter == 2 && it.type == 2) || (filter == 1 && it.type == 1) }
        .sortedWith(compareByDescending<Node.McNode> { it.favorite }.thenByDescending { it.seen })
    val saved = marse.filter { Node.nicknames.value.containsKey(it.addr) }
    val showMarse = filter != 3
    LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 24.dp)) {
        item {
            OutlinedTextField(
                query, { query = it }, Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp), singleLine = true,
                placeholder = { Text("Search by name or address") }, leadingIcon = { Icon(Icons.Default.Search, null) },
                shape = RoundedCornerShape(24.dp),
            )
        }
        item { ChipRow(if (showMc) FILTERS else FILTERS.take(3), filter, { filter = it }, violet = if (showMc) 3 else -1) }
        if (ready) item {
            Column {
                Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 2.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text("MeshCore radio", Modifier.weight(1f), style = MaterialTheme.typography.labelMedium, color = Mc.color, fontWeight = FontWeight.SemiBold)
                    MeshCoreActions()
                }
                if (McNav.advertNote.isNotEmpty()) Text(McNav.advertNote, Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
        if (showMarse && q.isBlank()) item {
            Group { GroupRow("Announces", "Nodes and repeaters announcing themselves, live", last = true) { onAnnounces() } }
        }
        if (showMarse && marse.isEmpty() && (mc.isEmpty() || filter != 0)) item {
            Text(if (q.isNotBlank() || filter != 0) "Nobody matches." else "Nobody heard yet. People show up when they announce themselves or write to you. Give someone a name to save them.",
                Modifier.padding(24.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        if (showMarse && saved.isNotEmpty()) item { SectionLabel("Saved (${saved.size})") }
        if (showMarse) items(saved, key = { "s" + it.addr }) { n -> ContactRow(n, onContact) }
        if (showMarse && marse.size > saved.size) item { SectionLabel("Heard recently") }
        if (showMarse) items(marse - saved.toSet(), key = { it.addr }) { n -> ContactRow(n, onContact) }
        if (showMc && (mc.isNotEmpty() || filter == 3)) {
            item { SectionLabel("MeshCore", meshcore = true) }
            if (mc.isEmpty()) item {
                Text(if (mcState.nodes.isEmpty()) "No MeshCore nodes heard yet. Choose Scan now, or wait for adverts." else "Nobody matches.",
                    Modifier.padding(horizontal = 24.dp, vertical = 8.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            items(mc, key = { "mc" + it.key }) { n -> McRow(n) }
        }
    }
}

private val MC_KINDS = listOf("Node", "Chat", "Repeater", "Room", "Sensor")

@Composable
private fun McRow(n: Node.McNode) {
    val name = n.name.ifEmpty { tr("Unnamed") + " · " + n.key.take(4) }
    Row(
        Modifier.fillMaxWidth().heightIn(min = 64.dp).clickable { McNav.node = n.key }.padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Avatar(name, n.key, 44.dp)
        Column(Modifier.weight(1f)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(name, Modifier.weight(1f, fill = false), fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                NetworkTag()
            }
            Text(listOfNotNull(MC_KINDS.getOrElse(n.type) { "Node" }, hopsText(n.hops), if (n.seen > 0) tr("heard {0}", agoText(n.seen * 1000)) else null).joinToString(" · "),
                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (n.seen > 0) SignalBars(Math.round(n.snr).toInt())
    }
}

@Composable
private fun ContactRow(n: NodeEntry, onContact: (String) -> Unit) {
    val title = friendlyName(Node.label(n.addr))
    Row(
        Modifier.fillMaxWidth().heightIn(min = 64.dp).clickable { onContact(n.addr) }.padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Avatar(title, n.addr, 44.dp)
        Column(Modifier.weight(1f)) {
            Text(title, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(listOfNotNull(kindLabel(n), hopsText(n.hops), if (n.lastHeard > 0) tr("heard {0}", time(n.lastHeard)) else null).joinToString(" · "),
                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        snrOf(n.addr)?.let { SignalBars(it) }
    }
}

/**
 * The safety number with a contact (as the web client's "Verify key"):
 * compare it with theirs in person or on a call; then mark it verified.
 */
@Composable
private fun SafetyNumber(addr: String) {
    Node.keys.collectAsState().value
    Node.verified.collectAsState().value
    val mine by Node.myKey.collectAsState()
    val num = remember(addr, mine, Node.keys.value[addr]) { Node.safetyNumber(addr) }
    GroupTitle("Safety number")
    Card2 {
        when {
            mine == null -> Text("The node has not reported its own key (older firmware?).", style = MaterialTheme.typography.bodySmall)
            num == null -> Text("The contact's key is not known yet. Wait for an announce from it.", style = MaterialTheme.typography.bodySmall)
            else -> {
                if (Node.keyChanged(addr)) Text("⚠ The contact's key has changed since you verified it. Compare the number again before you trust it.",
                    color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                Text("Compare these numbers with the ones on their screen, in person or on a call where you know their voice. If they are the same, nobody is in between.",
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                // 12 groups of 5 digits, 4 per row.
                num.chunked(5).chunked(4).forEach { row ->
                    Row(Modifier.padding(vertical = 2.dp), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                        row.forEach { Text(it, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.SemiBold, style = MaterialTheme.typography.titleMedium) }
                    }
                }
                val ok = Node.isVerified(addr)
                if (ok) Text("✓ Verified", color = MaterialTheme.colorScheme.primary, fontWeight = FontWeight.SemiBold)
                androidx.compose.material3.TextButton(onClick = { Node.setVerified(addr, !ok) }) {
                    Text(if (ok) "Remove verification" else "Mark as verified")
                }
            }
        }
    }
}

@Composable
private fun Action(icon: androidx.compose.ui.graphics.vector.ImageVector, label: String, modifier: Modifier, onClick: () -> Unit) {
    androidx.compose.material3.Surface(onClick = onClick, modifier = modifier, shape = RoundedCornerShape(16.dp),
        color = MaterialTheme.colorScheme.secondaryContainer) {
        Column(Modifier.padding(vertical = 12.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            Icon(icon, null)
            Text(label, style = MaterialTheme.typography.labelLarge, maxLines = 1, modifier = Modifier.padding(top = 4.dp))
        }
    }
}

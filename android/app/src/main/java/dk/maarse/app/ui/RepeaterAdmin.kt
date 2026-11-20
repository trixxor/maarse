package dk.maarse.app.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.AccountTree
import androidx.compose.material.icons.filled.BarChart
import androidx.compose.material.icons.filled.Campaign
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.Hub
import androidx.compose.material.icons.filled.Info
import androidx.compose.material.icons.filled.Lock
import androidx.compose.material.icons.filled.LocationOn
import androidx.compose.material.icons.filled.Person
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material.icons.filled.RestartAlt
import androidx.compose.material.icons.filled.Schedule
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Shield
import androidx.compose.material.icons.filled.Terminal
import androidx.compose.material.icons.filled.Tune
import androidx.compose.material.icons.filled.VpnKey
import androidx.compose.material.icons.filled.Router
import androidx.compose.material.icons.filled.Sync
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dk.maarse.app.Node
import dk.maarse.app.hex
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/** Admin screen for a logged-in repeater, room or sensor: Status, Command Line and Settings, as in the MeshCore app. */
private val RAmber = Color(0xFFE9B25A)
private val RAmberDark = Color(0xFF3A2C10)
private val RAmberText = Color(0xFF2A1B00)
private val RGreen = Color(0xFF5CC8A1)
private val RDanger = Color(0xFFF08A7A)
private val RLight = Color(0xFFE8F1EC)

/** Talks to the repeater through the backbone: one command or request at a time, each waiting for its own answer. */
object Rep {
    private val lock = Mutex()
    private val pending = listOf("Waiting for repeater", "Request queued.")
    var tab by mutableStateOf(0)
    var tool by mutableStateOf<String?>(null)
    val status = mutableStateMapOf<String, String>()
    val telemetry = mutableStateListOf<String>()
    val acl = mutableStateListOf<String>()
    var statusNote by mutableStateOf("")
    var telemetryNote by mutableStateOf("")
    var clockText by mutableStateOf("")
    var busy by mutableStateOf(0)

    fun reset() { tab = 0; tool = null; status.clear(); telemetry.clear(); acl.clear(); statusNote = ""; telemetryNote = ""; clockText = "" }

    /** Sends a CLI command and returns the repeater's text answer, or an "ERROR ..." text. */
    suspend fun command(key: String, cmd: String): String = lock.withLock {
        busy++
        try {
            val bytes = cmd.toByteArray()
            if (bytes.size > 60) return@withLock "ERROR The command is too long (60 bytes at most)."
            val before = Node.mcCall("repeater-status:$key")?.repeaterSeq ?: 0L
            var sent = Node.mcCall("repeater-command:$key:${bytes.hex()}")
            var tries = 0
            while (sent != null && sent.text.contains("already queued") && tries++ < 6) { delay(700); sent = Node.mcCall("repeater-command:$key:${bytes.hex()}") }
            if (sent == null) return@withLock "ERROR No connection to the backbone."
            repeat(22) {
                delay(1500)
                val st = Node.mcCall("repeater-status:$key") ?: return@repeat
                val text = st.repeaterStatus
                if (st.repeaterSeq > before && pending.none { text.startsWith(it) }) return@withLock text
            }
            "ERROR No answer from the repeater. Try again."
        } finally { busy-- }
    }

    /** Binary request: "status", "telemetry" or "acl". */
    suspend fun request(key: String, kind: String): String = lock.withLock {
        busy++
        try {
            val payload = "repeater-request:$key:${kind.toByteArray().hex()}"
            var sent = Node.mcCall(payload)
            var tries = 0
            while (sent != null && sent.text.contains("already queued") && tries++ < 6) { delay(700); sent = Node.mcCall(payload) }
            if (sent == null) return@withLock "ERROR No connection to the backbone."
            repeat(18) {
                delay(1500)
                val d = Node.mcCall("repeater-data:$key:$kind") ?: return@repeat
                if (d.repeaterKind == kind && d.repeaterData.isNotBlank()) return@withLock d.repeaterData
            }
            "ERROR No answer from the node. Try again."
        } finally { busy-- }
    }
}

private fun reply(text: String) = text.removePrefix("> ").trim()
private fun failed(text: String) = text.startsWith("ERROR") || text.startsWith("Error") || text.startsWith("Administrator login required") ||
    text.startsWith("Login required")

private fun ago(seconds: Long): String = when {
    seconds < 90 -> "${seconds} s ago"
    seconds < 5400 -> "${(seconds + 30) / 60} min ago"
    seconds < 129600 -> "${seconds / 3600} h ago"
    else -> "${seconds / 86400} d ago"
}

private fun duration(seconds: Long): String {
    val d = seconds / 86400; val h = seconds % 86400 / 3600; val m = seconds % 3600 / 60; val s = seconds % 60
    return "$d days ${h}h ${m}m ${s}s"
}

// ------------------------------------------------------------------ The screen

@Composable
fun RepeaterAdminScreen(n: dk.maarse.app.Node.McNode, name: String, mc: Node.MeshCoreAccess, admin: Boolean, onLogout: () -> Unit) {
    val title = when (n.type) { 3 -> "Room Admin"; 4 -> "Sensor"; else -> "Repeater Admin" }
    val tool = Rep.tool
    BackHandler { if (tool != null) Rep.tool = null else onLogout() }
    LaunchedEffect(n.key) { Rep.reset() }
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().padding(start = 8.dp, end = 12.dp, top = 8.dp, bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = { if (tool != null) Rep.tool = null else onLogout() }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            Column(Modifier.weight(1f)) {
                Text(title, fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
                Text(name, fontSize = 13.sp, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            OutlinedButton(onClick = onLogout, shape = RoundedCornerShape(18.dp)) { Text("Log out") }
        }
        Box(Modifier.fillMaxWidth().background(if (admin) Color(0xFF1F5A37) else Color(0xFF3A4A1F)).padding(horizontal = 16.dp, vertical = 8.dp)) {
            Text(if (admin) "You are logged in as an Admin" else "Logged in as a guest: status and sensor readings only", fontSize = 13.sp, color = Color.White)
        }
        if (Rep.busy > 0) LinearProgressIndicator(Modifier.fillMaxWidth(), color = RAmber, trackColor = RAmberDark)
        Box(Modifier.weight(1f).fillMaxWidth()) {
            when {
                tool != null -> ToolScreen(tool, n, name, mc)
                Rep.tab == 0 -> StatusTab(n.key, admin)
                Rep.tab == 1 -> TerminalTab(n.key)
                else -> SettingsTab(n.key, mc)
            }
        }
        if (tool == null) NavigationBar(containerColor = MaterialTheme.colorScheme.surface) {
            val tabs = buildList {
                add(Triple(0, "Status", Icons.Default.Info))
                if (admin) add(Triple(1, "Command Line", Icons.Default.Terminal))
                if (admin) add(Triple(2, "Settings", Icons.Default.Settings))
                if (!admin) add(Triple(3, "Telemetry", Icons.Default.BarChart))
            }
            for ((index, label, icon) in tabs) NavigationBarItem(
                selected = Rep.tab == index || (index == 3 && Rep.tab == 3), onClick = { Rep.tab = index },
                icon = { Icon(icon, label) }, label = { Text(label, fontSize = 11.sp) },
                colors = NavigationBarItemDefaults.colors(selectedIconColor = RAmberText, indicatorColor = RAmber),
            )
        }
    }
    if (Rep.tab == 3 && tool == null) { Rep.tool = "telemetry"; Rep.tab = 0 }
}

// ------------------------------------------------------------------ Status

@Composable
private fun StatusTab(key: String, admin: Boolean) {
    val scope = rememberCoroutineScope()
    fun refresh() = scope.launch {
        Rep.statusNote = ""
        val text = Rep.request(key, "status")
        if (failed(text)) { Rep.statusNote = text.removePrefix("ERROR ").trim(); return@launch }
        Rep.status.clear()
        for (line in text.lines()) line.split(' ', limit = 2).takeIf { it.size == 2 }?.let { Rep.status[it[0]] = it[1] }
        if (admin) { val clock = Rep.command(key, "clock"); if (!failed(clock)) Rep.clockText = reply(clock) }
    }
    LaunchedEffect(key) { refresh() }
    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 12.dp), verticalArrangement = Arrangement.spacedBy(18.dp)) {
            if (Rep.statusNote.isNotEmpty()) Text(Rep.statusNote, color = RDanger)
            val s = Rep.status
            if (s.isEmpty() && Rep.statusNote.isEmpty()) Text("Asking the node for its status…", color = MaterialTheme.colorScheme.onSurfaceVariant)
            s["battery_mv"]?.toIntOrNull()?.let { mv ->
                val pct = Math.round((mv - 3000) / 12.0).toInt().coerceIn(0, 100)
                Item("Battery", "$pct% / ${"%.2f".format(mv / 1000.0)}v")
            }
            if (Rep.clockText.isNotEmpty()) Item("Clock", Rep.clockText)
            s["uptime"]?.toLongOrNull()?.let { Item("Uptime", duration(it)) }
            if (s["tx_airtime"] != null) Item("Total Airtime", "TX: ${duration(s["tx_airtime"]!!.toLongOrNull() ?: 0)}\nRX: ${duration(s["rx_airtime"]?.toLongOrNull() ?: 0)}")
            s["last_rssi"]?.let { Item("Last RSSI", it) }
            s["last_snr"]?.let { Item("Last SNR", it) }
            s["noise_floor"]?.let { Item("Noise Floor", "${it}dB") }
            if (s["sent"] != null) Item("Packets Sent", "${s["sent"]}  (flood ${s["sent_flood"]}, direct ${s["sent_direct"]})")
            if (s["recv"] != null) Item("Packets Received", "${s["recv"]}  (flood ${s["recv_flood"]}, direct ${s["recv_direct"]})")
            if (s["dups_flood"] != null) Item("Duplicates", "flood ${s["dups_flood"]}, direct ${s["dups_direct"]}")
            if (s["errors"] != null) Item("Errors", "events ${s["errors"]}, receive errors ${s["recv_errors"]}  ·  queue ${s["queue"]}")
        }
        Box(
            Modifier.align(Alignment.BottomEnd).padding(20.dp).size(56.dp).clip(RoundedCornerShape(16.dp)).background(MaterialTheme.colorScheme.surfaceVariant)
                .clickable { refresh() }, contentAlignment = Alignment.Center,
        ) { Icon(Icons.Default.Refresh, "Refresh") }
    }
}

@Composable
private fun Item(label: String, value: String) {
    Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
        Text(label, fontSize = 20.sp)
        Text(value, fontSize = 17.sp, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

// ------------------------------------------------------------------ Command line

@Composable
private fun TerminalTab(key: String) {
    val scope = rememberCoroutineScope()
    var command by remember { mutableStateOf("") }
    val listState = rememberLazyListState()
    LaunchedEffect(McNav.log.size) { if (McNav.log.isNotEmpty()) listState.scrollToItem(McNav.log.size - 1) }
    fun run(text: String) {
        if (text.isBlank()) return
        McNav.log.add(true to text.trim())
        scope.launch { McNav.log.add(false to reply(Rep.command(key, text.trim()))) }
    }
    Column(Modifier.fillMaxSize().padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            for ((label, cmd) in listOf("Version" to "ver", "Clock" to "clock", "Neighbours" to "neighbors", "Advert" to "advert", "Stats" to "stats-core"))
                OutlinedButton(onClick = { run(cmd) }, shape = RoundedCornerShape(18.dp)) { Text(label) }
        }
        LazyColumn(
            Modifier.weight(1f).fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(Color(0xFF0A100D)).padding(12.dp),
            state = listState, verticalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            if (McNav.log.isEmpty()) item { Text("Type a command. Replies take a few seconds over the radio.", fontFamily = FontFamily.Monospace, fontSize = 12.5.sp, color = MaterialTheme.colorScheme.onSurfaceVariant) }
            items(McNav.log.size) { i ->
                val (typed, line) = McNav.log[i]
                Text(if (typed) "> $line" else line, fontFamily = FontFamily.Monospace, fontSize = 12.5.sp, color = if (typed) MaterialTheme.colorScheme.onSurfaceVariant else RLight)
            }
        }
        Row(Modifier.fillMaxWidth().padding(bottom = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            OutlinedTextField(command, { command = it }, Modifier.weight(1f), singleLine = true, placeholder = { Text("Command") }, shape = RoundedCornerShape(24.dp))
            Box(
                Modifier.size(48.dp).clip(CircleShape).background(if (command.isBlank()) RAmberDark else RAmber).clickable(enabled = command.isNotBlank()) { run(command); command = "" },
                contentAlignment = Alignment.Center,
            ) { Icon(Icons.AutoMirrored.Filled.Send, "Send command", tint = RAmberText) }
        }
    }
}

// ------------------------------------------------------------------ Settings

private enum class RKind { TEXT, NUMBER, ONOFF, LOOP, PASSWORD }

private class RField(val label: String, val get: String?, val set: (String) -> String, val kind: RKind = RKind.TEXT, val note: String = "")
private class RAction(val label: String, val command: String, val confirm: String? = null, val danger: Boolean = false)
private class RToolDef(val id: String, val title: String, val subtitle: String, val icon: ImageVector,
                      val fields: List<RField> = emptyList(), val actions: List<RAction> = emptyList())

private val tools = listOf(
    RToolDef("owner", "Owner Info", "Add public contact details.", Icons.Default.Person,
        fields = listOf(RField("Owner info", "get owner.info", { "set owner.info $it" }, note = "Shown to anyone who asks the repeater. Up to about 45 characters."))),
    RToolDef("advert", "Advert", "Repeater will send a flood advert.", Icons.Default.Campaign,
        actions = listOf(RAction("Send flood advert", "advert"), RAction("Send zero-hop advert", "advert.zerohop"))),
    RToolDef("intervals", "Advert Intervals", "Set auto advert intervals.", Icons.Default.Schedule,
        fields = listOf(
            RField("Zero-hop advert interval (minutes, 0 = off)", "get advert.interval", { "set advert.interval $it" }, RKind.NUMBER),
            RField("Flood advert interval (hours, 0 = off)", "get flood.advert.interval", { "set flood.advert.interval $it" }, RKind.NUMBER),
        )),
    RToolDef("position", "Position", "Set repeater position on the map.", Icons.Default.LocationOn,
        fields = listOf(RField("Latitude", "get lat", { "set lat $it" }, RKind.NUMBER), RField("Longitude", "get lon", { "set lon $it" }, RKind.NUMBER))),
    RToolDef("clock", "Sync Clock", "Updates the repeater clock.", Icons.Default.Sync,
        actions = listOf(RAction("Sync clock to now", "clock sync"), RAction("Show repeater clock", "clock"))),
    RToolDef("access", "Access Control", "Add or remove users.", Icons.Default.Shield),
    RToolDef("adminpw", "Admin Password", "Set remote management password.", Icons.Default.Lock,
        fields = listOf(RField("New admin password (15 characters at most)", null, { "password $it" }, RKind.PASSWORD, "You are logged in with the old one until the repeater restarts."))),
    RToolDef("guestpw", "Guest Password", "Set repeater status password.", Icons.Default.Lock,
        fields = listOf(RField("Guest password", "get guest.password", { "set guest.password $it" }, RKind.PASSWORD))),
    RToolDef("identity", "Change Identity Key", "Allows choosing a new prefix.", Icons.Default.VpnKey),
    RToolDef("regions", "Manage Regions", "Configure regions to repeat.", Icons.Default.AccountTree),
    RToolDef("neighbours", "Neighbours", "View zero hop neighbours.", Icons.Default.Hub),
    RToolDef("network", "Network Settings", "Configure Mesh Network", Icons.Default.Tune,
        fields = listOf(
            RField("Maximum flood hops", "get flood.max", { "set flood.max $it" }, RKind.NUMBER),
            RField("Maximum flood hops for adverts", "get flood.max.advert", { "set flood.max.advert $it" }, RKind.NUMBER),
            RField("Loop detection", "get loop.detect", { "set loop.detect $it" }, RKind.LOOP),
            RField("Allow read-only logins", "get allow.read.only", { "set allow.read.only $it" }, RKind.ONOFF),
        )),
    RToolDef("repeat", "Repeat Settings", "Turn repeat mode on or off.", Icons.Default.Router,
        fields = listOf(RField("Repeat other nodes' packets", "get repeat", { "set repeat $it" }, RKind.ONOFF))),
    RToolDef("telemetry", "Telemetry", "Fetch and view telemetry.", Icons.Default.BarChart),
    RToolDef("reboot", "Reboot", "Reboot the repeater.", Icons.Default.RestartAlt,
        actions = listOf(RAction("Reboot now", "reboot", "The repeater is off the air for a moment and you are logged out.", danger = true))),
    RToolDef("version", "Version", "Get repeater firmware version.", Icons.Default.Info,
        actions = listOf(RAction("Show firmware version", "ver"))),
)

@Composable
private fun SettingsTab(key: String, mc: Node.MeshCoreAccess) {
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 16.dp)) {
        Section("Public Info")
        Card { FieldRow(key, RField("Name", "get name", { "set name $it" })) }
        Card {
            Text("Public key", fontSize = 13.sp, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text(mc.repeaterKey.ifEmpty { key }, fontFamily = FontFamily.Monospace, fontSize = 12.sp)
        }
        Section("Radio Settings")
        RadioCard(key)
        Section("Extra Tools")
        for (t in tools) Row(
            Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 5.dp).clip(RoundedCornerShape(16.dp)).background(MaterialTheme.colorScheme.surfaceVariant)
                .clickable { Rep.tool = t.id }.padding(horizontal = 16.dp, vertical = 16.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Icon(t.icon, null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
            Column(Modifier.weight(1f)) { Text(t.title, fontSize = 17.sp); Text(t.subtitle, fontSize = 13.sp, color = MaterialTheme.colorScheme.onSurfaceVariant) }
            Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@Composable
private fun Section(title: String) {
    Text(title, Modifier.padding(horizontal = 20.dp, vertical = 14.dp), fontSize = 18.sp)
}

@Composable
private fun Card(content: @Composable () -> Unit) {
    Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp).clip(RoundedCornerShape(14.dp)).background(MaterialTheme.colorScheme.surfaceVariant).padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp)) { content() }
}

/** One setting: read it with the refresh icon, change it and apply with the check icon. */
@Composable
private fun FieldRow(key: String, f: RField) {
    val scope = rememberCoroutineScope()
    var value by remember(key, f.label) { mutableStateOf("") }
    var message by remember(key, f.label) { mutableStateOf("") }
    var busy by remember(key, f.label) { mutableStateOf(false) }
    fun load() { if (f.get == null) return; scope.launch { busy = true; val r = Rep.command(key, f.get); busy = false; if (failed(r)) message = r.removePrefix("ERROR ") else { value = reply(r); message = "" } } }
    fun apply() = scope.launch {
        busy = true
        val r = Rep.command(key, f.set(value.trim())); busy = false
        message = if (failed(r)) r.removePrefix("ERROR ") else reply(r).ifEmpty { "OK" }
    }
    LaunchedEffect(key, f.label) { load() }
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(f.label, Modifier.weight(1f), fontSize = 13.sp, color = MaterialTheme.colorScheme.onSurfaceVariant)
            if (f.get != null) IconButton(onClick = { load() }, enabled = !busy) { Icon(Icons.Default.Refresh, "Read from the repeater") }
            IconButton(onClick = { apply() }, enabled = !busy && value.isNotBlank()) { Icon(Icons.Default.Check, "Apply") }
        }
        when (f.kind) {
            RKind.ONOFF -> Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                for (o in listOf("on", "off")) FilterChip(selected = value == o, onClick = { value = o }, label = { Text(o) },
                    colors = FilterChipDefaults.filterChipColors(selectedContainerColor = RAmber, selectedLabelColor = RAmberText))
            }
            RKind.LOOP -> Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                for (o in listOf("off", "minimal", "moderate", "strict")) FilterChip(selected = value == o, onClick = { value = o }, label = { Text(o) },
                    colors = FilterChipDefaults.filterChipColors(selectedContainerColor = RAmber, selectedLabelColor = RAmberText))
            }
            else -> OutlinedTextField(
                value, { value = it }, Modifier.fillMaxWidth(), singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = if (f.kind == RKind.NUMBER) KeyboardType.Decimal else KeyboardType.Text),
                visualTransformation = if (f.kind == RKind.PASSWORD && f.get == null) PasswordVisualTransformation() else androidx.compose.ui.text.input.VisualTransformation.None,
            )
        }
        if (f.note.isNotEmpty()) Text(f.note, fontSize = 12.sp, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (message.isNotEmpty()) Text(message, fontSize = 12.sp, color = if (message.startsWith("OK") || !message.contains("rror")) RGreen else RDanger)
    }
}

/** Frequency, bandwidth, spreading factor and coding rate are one setting on the repeater ("radio f,bw,sf,cr"); power is separate. */
@Composable
private fun RadioCard(key: String) {
    val scope = rememberCoroutineScope()
    var freq by remember(key) { mutableStateOf("") }
    var bw by remember(key) { mutableStateOf("") }
    var sf by remember(key) { mutableStateOf("") }
    var cr by remember(key) { mutableStateOf("") }
    var power by remember(key) { mutableStateOf("") }
    var message by remember(key) { mutableStateOf("") }
    var confirm by remember(key) { mutableStateOf(false) }
    fun load() = scope.launch {
        val r = reply(Rep.command(key, "get radio")).split(',')
        if (r.size >= 4) { freq = r[0]; bw = r[1]; sf = r[2]; cr = r[3]; message = "" } else message = "Could not read the radio settings."
        val p = Rep.command(key, "get tx"); if (!failed(p)) power = reply(p)
    }
    LaunchedEffect(key) { load() }
    Card {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("The radio settings must match the rest of the network.", Modifier.weight(1f), fontSize = 12.sp, color = MaterialTheme.colorScheme.onSurfaceVariant)
            IconButton(onClick = { load() }) { Icon(Icons.Default.Refresh, "Read from the repeater") }
            IconButton(onClick = { confirm = true }, enabled = freq.isNotBlank()) { Icon(Icons.Default.Check, "Apply") }
        }
        OutlinedTextField(freq, { freq = it }, Modifier.fillMaxWidth(), singleLine = true, label = { Text("Frequency (MHz)") }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal))
        OutlinedTextField(bw, { bw = it }, Modifier.fillMaxWidth(), singleLine = true, label = { Text("Bandwidth (kHz)") }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal))
        OutlinedTextField(sf, { sf = it }, Modifier.fillMaxWidth(), singleLine = true, label = { Text("Spreading factor") }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number))
        OutlinedTextField(cr, { cr = it }, Modifier.fillMaxWidth(), singleLine = true, label = { Text("Coding rate") }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number))
        OutlinedTextField(power, { power = it }, Modifier.fillMaxWidth(), singleLine = true, label = { Text("Transmit power (dBm)") }, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number))
        if (message.isNotEmpty()) Text(message, fontSize = 12.sp, color = if (message.contains("rror") || message.startsWith("Could")) RDanger else RGreen)
    }
    if (confirm) AlertDialog(
        onDismissRequest = { confirm = false },
        title = { Text("Change the radio settings?") },
        text = { Text("If these do not match the rest of your network, you can no longer reach this repeater by radio. The change takes effect after a reboot.") },
        confirmButton = {
            TextButton(onClick = {
                confirm = false
                scope.launch {
                    val r = Rep.command(key, "set radio ${freq.trim()},${bw.trim()},${sf.trim()},${cr.trim()}")
                    var out = if (failed(r)) r.removePrefix("ERROR ") else reply(r).ifEmpty { "OK" }
                    if (power.isNotBlank()) out += " · " + reply(Rep.command(key, "set tx ${power.trim()}")).ifEmpty { "OK" }
                    message = out
                }
            }) { Text("Apply", color = RAmber) }
        },
        dismissButton = { TextButton(onClick = { confirm = false }) { Text("Cancel") } },
    )
}

// ------------------------------------------------------------------ One tool

@Composable
private fun ToolScreen(id: String, n: dk.maarse.app.Node.McNode, name: String, mc: Node.MeshCoreAccess) {
    val def = tools.firstOrNull { it.id == id }
    val key = n.key
    val scope = rememberCoroutineScope()
    var result by remember(id) { mutableStateOf("") }
    var confirm by remember(id) { mutableStateOf<RAction?>(null) }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 12.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(def?.title ?: "Telemetry", Modifier.padding(horizontal = 8.dp), fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
        if (def != null) Text(def.subtitle, Modifier.padding(horizontal = 8.dp), fontSize = 13.sp, color = MaterialTheme.colorScheme.onSurfaceVariant)
        def?.fields?.forEach { f -> Card { FieldRow(key, f) } }
        def?.actions?.forEach { a ->
            Button(
                onClick = { if (a.confirm != null) confirm = a else scope.launch { result = reply(Rep.command(key, a.command)).removePrefix("ERROR ") } },
                Modifier.fillMaxWidth().height(48.dp), shape = RoundedCornerShape(24.dp),
                colors = ButtonDefaults.buttonColors(containerColor = if (a.danger) Color(0xFF6B3A33) else RAmber, contentColor = if (a.danger) RDanger else RAmberText),
            ) { Text(a.label, fontWeight = FontWeight.SemiBold) }
        }
        if (result.isNotEmpty()) Text(result, fontFamily = FontFamily.Monospace, fontSize = 13.sp, modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(Color(0xFF0A100D)).padding(12.dp))
        when (id) {
            "identity" -> Text("A new identity key can only be set from the repeater's own USB serial port, not over the radio. Use the MeshCore flasher or a serial terminal for that.", Modifier.padding(8.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
            "telemetry" -> TelemetryPanel(key)
            "access" -> AccessPanel(key, mc)
            "regions" -> RegionsPanel(key)
            "neighbours" -> NeighboursPanel(key, mc)
        }
    }
    confirm?.let { a ->
        AlertDialog(
            onDismissRequest = { confirm = null },
            title = { Text("${a.label}?") }, text = { Text(a.confirm.orEmpty()) },
            confirmButton = { TextButton(onClick = { confirm = null; scope.launch { result = reply(Rep.command(key, a.command)).removePrefix("ERROR ") } }) { Text(a.label, color = RDanger) } },
            dismissButton = { TextButton(onClick = { confirm = null }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun TelemetryPanel(key: String) {
    val scope = rememberCoroutineScope()
    fun load() = scope.launch {
        Rep.telemetryNote = ""
        val text = Rep.request(key, "telemetry")
        if (failed(text)) { Rep.telemetryNote = text.removePrefix("ERROR ").trim(); return@launch }
        Rep.telemetry.clear()
        if (text.trim() == "EMPTY") Rep.telemetryNote = "The node reports no sensor readings." else Rep.telemetry.addAll(text.lines().filter { it.isNotBlank() })
    }
    LaunchedEffect(key) { load() }
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        OutlinedButton(onClick = { load() }, shape = RoundedCornerShape(18.dp)) { Icon(Icons.Default.Refresh, null); Text("  Refresh") }
        if (Rep.telemetryNote.isNotEmpty()) Text(Rep.telemetryNote, color = MaterialTheme.colorScheme.onSurfaceVariant)
        for (line in Rep.telemetry) {
            val parts = line.split(' ', limit = 3)
            if (parts.size == 3) Card {
                Text("${parts[1]}  ·  channel ${parts[0]}", fontSize = 13.sp, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Text(parts[2], fontSize = 22.sp, fontWeight = FontWeight.SemiBold)
            }
        }
    }
}

private fun nameOf(prefix: String, mc: Node.MeshCoreAccess): String {
    val p = prefix.lowercase()
    return mc.nodes.firstOrNull { it.key.startsWith(p) }?.name?.ifEmpty { null } ?: (p.take(8) + "…")
}

@Composable
private fun AccessPanel(key: String, mc: Node.MeshCoreAccess) {
    val scope = rememberCoroutineScope()
    var note by remember { mutableStateOf("") }
    var newKey by remember { mutableStateOf("") }
    var perm by remember { mutableStateOf("1") }
    fun load() = scope.launch {
        val text = Rep.request(key, "acl")
        Rep.acl.clear()
        if (failed(text)) note = text.removePrefix("ERROR ").trim() else { note = ""; if (text.trim() != "EMPTY") Rep.acl.addAll(text.lines().filter { it.isNotBlank() }) }
    }
    LaunchedEffect(key) { load() }
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        OutlinedButton(onClick = { load() }, shape = RoundedCornerShape(18.dp)) { Icon(Icons.Default.Refresh, null); Text("  Refresh") }
        if (note.isNotEmpty()) Text(note, color = RDanger)
        if (Rep.acl.isEmpty() && note.isEmpty()) Text("No users in the access list.", color = MaterialTheme.colorScheme.onSurfaceVariant)
        for (line in Rep.acl) {
            val (prefix, p) = line.split(' ').let { it[0] to (it.getOrNull(1)?.toIntOrNull() ?: 0) }
            val full = mc.nodes.firstOrNull { it.key.startsWith(prefix.lowercase()) }?.key
            Card {
                Text(nameOf(prefix, mc), fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                Text("$prefix  ·  ${when (p and 3) { 3 -> "Admin"; 2 -> "Read / write"; 1 -> "Guest"; else -> "No access" }}", fontSize = 13.sp, color = MaterialTheme.colorScheme.onSurfaceVariant)
                if (full != null) TextButton(onClick = { scope.launch { note = reply(Rep.command(key, "setperm $full 0")); load() } }) { Text("Remove", color = RDanger) }
            }
        }
        Card {
            Text("Add a user", fontWeight = FontWeight.SemiBold)
            OutlinedTextField(newKey, { newKey = it.trim().lowercase() }, Modifier.fillMaxWidth(), singleLine = true, label = { Text("Public key (64 characters)") })
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                for ((v, label) in listOf("1" to "Guest", "2" to "Read / write", "3" to "Admin"))
                    FilterChip(selected = perm == v, onClick = { perm = v }, label = { Text(label) },
                        colors = FilterChipDefaults.filterChipColors(selectedContainerColor = RAmber, selectedLabelColor = RAmberText))
            }
            Button(
                onClick = { scope.launch { note = reply(Rep.command(key, "setperm $newKey $perm")); newKey = ""; load() } }, enabled = Regex("[0-9a-f]{64}").matches(newKey),
                colors = ButtonDefaults.buttonColors(containerColor = RAmber, contentColor = RAmberText),
            ) { Text("Add") }
        }
    }
}

@Composable
private fun RegionsPanel(key: String) {
    val scope = rememberCoroutineScope()
    var text by remember { mutableStateOf("") }
    var name by remember { mutableStateOf("") }
    fun run(cmd: String) = scope.launch { text = reply(Rep.command(key, cmd)).removePrefix("ERROR ") }
    LaunchedEffect(key) { run("region") }
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        OutlinedButton(onClick = { run("region") }, shape = RoundedCornerShape(18.dp)) { Icon(Icons.Default.Refresh, null); Text("  Refresh") }
        if (text.isNotEmpty()) Text(text, fontFamily = FontFamily.Monospace, fontSize = 13.sp, modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(Color(0xFF0A100D)).padding(12.dp))
        Card {
            OutlinedTextField(name, { name = it.trim() }, Modifier.fillMaxWidth(), singleLine = true, label = { Text("Region name") })
            Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                for ((label, cmd) in listOf("Add" to "region put", "Remove" to "region remove", "Allow flood" to "region allowf", "Deny flood" to "region denyf"))
                    OutlinedButton(onClick = { if (name.isNotBlank()) run("$cmd $name") }, shape = RoundedCornerShape(18.dp)) { Text(label) }
            }
            Button(onClick = { run("region save") }, colors = ButtonDefaults.buttonColors(containerColor = RAmber, contentColor = RAmberText)) { Text("Save regions") }
        }
    }
}

@Composable
private fun NeighboursPanel(key: String, mc: Node.MeshCoreAccess) {
    val scope = rememberCoroutineScope()
    var text by remember { mutableStateOf("") }
    fun load() = scope.launch { text = reply(Rep.command(key, "neighbors")).removePrefix("ERROR ") }
    LaunchedEffect(key) { load() }
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        OutlinedButton(onClick = { load() }, shape = RoundedCornerShape(18.dp)) { Icon(Icons.Default.Refresh, null); Text("  Refresh") }
        val rows = text.lines().mapNotNull { line -> line.split(':').takeIf { it.size >= 3 && it[0].length >= 4 } }
        if (rows.isEmpty() && text.isNotEmpty()) Text(text, fontFamily = FontFamily.Monospace, fontSize = 13.sp)
        for (r in rows) Card {
            Text(nameOf(r[0], mc), fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
            Text("${r[0]}  ·  heard ${ago(r[1].toLongOrNull() ?: 0)}  ·  SNR ${"%.1f".format((r[2].toIntOrNull() ?: 0) / 4.0)}", fontSize = 13.sp, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

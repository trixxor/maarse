package dk.maarse.app.ui

import android.annotation.SuppressLint
import android.bluetooth.BluetoothManager
import android.bluetooth.le.ScanCallback
import android.bluetooth.le.ScanFilter
import android.bluetooth.le.ScanResult
import android.bluetooth.le.ScanSettings
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.ParcelUuid
import android.os.PowerManager
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import dk.maarse.app.Cmd
import dk.maarse.app.Conn
import dk.maarse.app.HostFiles
import dk.maarse.app.Mode
import dk.maarse.app.Names
import dk.maarse.app.Node
import dk.maarse.app.Nus
import dk.maarse.app.PRECISION

@Composable
fun SettingsScreen() {
    val conn by Node.conn.collectAsState()
    val me by Node.me.collectAsState()
    Column(Modifier.verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
        ConnectionSection()
        // Linked devices live in maarse-host: the phone node or a host elsewhere.
        if (me != null && (conn.mode == Mode.PHONE || conn.mode == Mode.REMOTE)) AccountSection()
        if (me != null && conn.mode != Mode.SERVER) NodeSection()
        if (conn.mode == Mode.PHONE) HostSection(server = false)
        if (conn.mode == Mode.SERVER) HostSection(server = true)
        AppSection()
    }
}

// ---------- connection ----------

@Composable
internal fun ConnectionSection() {
    val ctx = LocalContext.current
    val conn by Node.conn.collectAsState()
    var mode by remember { mutableStateOf(Node.prefs.mode) }
    var picking by remember { mutableStateOf(false) }
    var bleName by remember { mutableStateOf(Node.prefs.bleName) }
    var remote by remember { mutableStateOf(Node.prefs.remoteAddr) }
    var key by remember { mutableStateOf(Node.prefs.remoteKey) }
    var refusal by remember { mutableStateOf<String?>(null) }
    SectionTitle("Connection")
    Mode.entries.forEach { m ->
        Row(Modifier.fillMaxWidth().clickable { mode = m }.padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            RadioButton(mode == m, { mode = m })
            Column { Text(m.title); Text(m.help, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
        }
    }
    when (mode) {
        Mode.BLE -> Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("Radio: " + (bleName ?: "none chosen"), Modifier.weight(1f))
            OutlinedButton(onClick = { picking = true }) { Text("Choose radio") }
        }
        Mode.REMOTE -> Column(Modifier.padding(horizontal = 16.dp)) {
            OutlinedTextField(remote, { remote = it; Node.prefs.remoteAddr = it }, Modifier.fillMaxWidth(), label = { Text("Address, e.g. 10.0.0.2:8080") }, singleLine = true)
            OutlinedTextField(key, { key = it; Node.prefs.remoteKey = it }, Modifier.fillMaxWidth(), label = { Text("Access key (access-key.txt on the host)") }, singleLine = true)
        }
        Mode.PHONE -> {
            Hint("No radio needed: the phone reaches others over the internet. Messages for people who are offline are kept by post offices on the backbone.")
            var modem by remember { mutableStateOf(dk.maarse.app.ModemBridge.enabled(ctx)) }
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text("Also use my radio as LoRa modem")
                    Text("The phone node sends and receives over LoRa through the radio (" + (bleName ?: "choose one under Radio over Bluetooth") + "). The radio only carries packets; the phone does the routing and the account.",
                        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                Switch(modem, { v -> modem = v; Node.setModemRadio(v) }, enabled = Node.prefs.bleAddress != null)
            }
            if (Node.prefs.bleAddress == null) OutlinedButton(onClick = { picking = true }, modifier = Modifier.padding(horizontal = 16.dp)) { Text("Choose radio") }
        }
        Mode.SERVER -> Hint("Needs Wi-Fi and a charger, a port opened in the router and a public address (below). Admin dashboard on port 8080.")
    }
    refusal?.let { Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(horizontal = 16.dp)) }
    Row(Modifier.padding(horizontal = 16.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        val running = conn.state != Conn.State.OFF
        Button(onClick = {
            refusal = null
            if (mode == Mode.SERVER && !HostFiles.serverConditionsMet(ctx)) { refusal = "Server mode needs Wi-Fi and a charger."; return@Button }
            Node.start(mode)
        }) { Text(if (running && mode == conn.mode) "Reconnect" else "Connect") }
        if (running) OutlinedButton(onClick = { Node.stop() }) { Text("Disconnect") }
    }
    if (picking) BlePicker(onDone = { addr, name ->
        picking = false
        if (addr != null) {
            Node.prefs.bleAddress = addr; Node.prefs.bleName = name; bleName = name
            // Chosen as the phone node's modem: stay a phone node.
            if (mode != Mode.PHONE) { mode = Mode.BLE; Node.start(Mode.BLE) }
        }
    })
}

private fun blePerms() = if (Build.VERSION.SDK_INT >= 31)
    arrayOf(android.Manifest.permission.BLUETOOTH_SCAN, android.Manifest.permission.BLUETOOTH_CONNECT)
else arrayOf(android.Manifest.permission.ACCESS_FINE_LOCATION)

private fun hasPerms(ctx: Context) = blePerms().all { ctx.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED }

@SuppressLint("MissingPermission")
@Composable
internal fun BlePicker(onDone: (String?, String?) -> Unit) {
    val ctx = LocalContext.current
    var granted by remember { mutableStateOf(hasPerms(ctx)) }
    var error by remember { mutableStateOf<String?>(null) }
    val found = remember { mutableStateListOf<Triple<String, String, Int>>() }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { r ->
        granted = r.values.all { it }
        if (!granted) error = "Bluetooth permission was denied"
    }
    DisposableEffect(granted) {
        if (!granted) { ask.launch(blePerms()); return@DisposableEffect onDispose {} }
        val adapter = ctx.getSystemService(BluetoothManager::class.java)?.adapter
        val sc = adapter?.bluetoothLeScanner
        if (adapter == null || !adapter.isEnabled || sc == null) { error = "Turn on Bluetooth and try again"; return@DisposableEffect onDispose {} }
        val cb = object : ScanCallback() {
            override fun onScanResult(type: Int, r: ScanResult) {
                val a = r.device.address
                if (found.any { it.first == a }) return
                val n = r.device.name ?: r.scanRecord?.deviceName ?: "Unknown node"
                found.add(Triple(a, n, r.rssi))
            }
        }
        sc.startScan(listOf(ScanFilter.Builder().setServiceUuid(ParcelUuid(Nus.SERVICE)).build()),
            ScanSettings.Builder().setScanMode(ScanSettings.SCAN_MODE_LOW_LATENCY).build(), cb)
        onDispose { runCatching { sc.stopScan(cb) } }
    }
    AlertDialog(
        onDismissRequest = { onDone(null, null) },
        title = { Text("Choose a Må:rse radio") },
        text = {
            Column(Modifier.heightIn(max = 360.dp).verticalScroll(rememberScrollState())) {
                error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
                if (error == null && found.isEmpty()) Text("Searching… The radio must be switched on and close by.")
                found.forEach { (a, n, rssi) ->
                    ListItem(headlineContent = { Text(n) }, supportingContent = { Text("$a · $rssi dBm") }, modifier = Modifier.clickable { onDone(a, n) })
                }
            }
        },
        confirmButton = {},
        dismissButton = { TextButton(onClick = { onDone(null, null) }) { Text("Cancel") } },
    )
}

// ---------- the node (radio, phone host or remote host) ----------

@Composable
internal fun NodeSection() {
    val me by Node.me.collectAsState()
    val stats by Node.stats.collectAsState()
    val regions by Node.regions.collectAsState()
    val sharing by Node.sharing.collectAsState()
    val radio by Node.radio.collectAsState()
    val conn by Node.conn.collectAsState()
    SectionTitle("This node")
    Hint("Address: " + (me?.addr ?: ""))
    var name by remember(me?.name) { mutableStateOf(me?.name ?: "") }
    Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
        OutlinedTextField(name, { name = it }, Modifier.weight(1f), label = { Text("Name others see (~name)") }, singleLine = true)
        TextButton(onClick = { Node.send(Cmd.setName(name.trim())); Node.send(Cmd.getName()) }) { Text("Save") }
    }
    // Roles only mean something on a radio node; the host is an anchor by default.
    if (conn.mode == Mode.BLE) {
        val role = stats[7]
        Text("Role", Modifier.padding(start = 16.dp, top = 8.dp), style = MaterialTheme.typography.labelLarge)
        Names.ROLES.forEachIndexed { i, r ->
            Row(Modifier.fillMaxWidth().clickable { Node.send(Cmd.setRole(i)); Node.send(Cmd.getStats()) }.padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                RadioButton(role == i, { Node.send(Cmd.setRole(i)); Node.send(Cmd.getStats()) })
                Column { Text(r); Text(Names.ROLE_HELP[i], style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
            }
        }
    }
    var reg by remember(regions) { mutableStateOf(regions?.names ?: "") }
    Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
        OutlinedTextField(reg, { reg = it }, Modifier.weight(1f), label = { Text("Regions (comma separated, e.g. dk, nordjylland)") }, singleLine = true)
        TextButton(onClick = { Node.send(Cmd.setRegions(regions?.strict ?: false, reg.trim())); Node.send(Cmd.getRegions()) }) { Text("Save") }
    }
    sharing?.let { sh ->
        Text("Share my position in announces", Modifier.padding(start = 16.dp, top = 8.dp), style = MaterialTheme.typography.labelLarge)
        Row(Modifier.padding(horizontal = 8.dp)) {
            PRECISION.forEachIndexed { i, p ->
                Row(verticalAlignment = Alignment.CenterVertically) {
                    RadioButton(sh.prec == i, {
                        Node.send(Cmd.setSharing(i, (sh.lat * 1e7).toInt(), (sh.lon * 1e7).toInt(), sh.telemetry)); Node.send(Cmd.getSharing())
                    })
                    Text(p, style = MaterialTheme.typography.bodySmall)
                }
            }
        }
    }
    radio?.let { r ->
        Hint("Radio: %.3f MHz · SF%d · %.1f kHz · %d dBm. All Må:rse nodes use the same radio settings.".format(r.freq / 1e6, r.sf, r.bw / 1e3, r.power))
    }
    Row(Modifier.padding(horizontal = 16.dp)) {
        TextButton(onClick = { Node.send(Cmd.announce()) }) { Text("Announce now") }
    }
}

// ---------- the phone's own maarse-host ----------

private enum class Kind { BOOL, BOOL_OFF, TEXT, LIST }
private data class Field(val flag: String, val kind: Kind, val label: String, val help: String = "")

private val HOST_FIELDS = listOf(
    Field("--public", Kind.BOOL, "Join the open backbone", "Connects to the public servers (nj01 …)."),
    Field("--peer", Kind.LIST, "Also connect to", "host:port, one per line, e.g. a friend's host or your home PC."),
    Field("--dht", Kind.LIST, "DHT seeds", "host:port, one per line."),
    Field("--listen", Kind.TEXT, "Accept connections on", "off, or e.g. 0.0.0.0:4242 (only useful on Wi-Fi; mobile networks cannot be reached)."),
    Field("--no-post-office", Kind.BOOL_OFF, "Keep mail for others (post office)", "Copies of mail for offline people, as one of three replicas."),
    Field("--mail-quota-mb", Kind.TEXT, "Post office size (MB)", "0 = no limit."),
    Field("--mailbox", Kind.TEXT, "My mailbox", "Address of a node that stores mail for me (32 hex characters)."),
    Field("--no-relay", Kind.BOOL_OFF, "Relay for hosts behind NAT"),
    Field("--no-bordercast", Kind.BOOL_OFF, "Bordercast", "Only put path requests from the backbone on the radio when needed."),
    Field("--region", Kind.LIST, "Regions", "E.g. dk or nordjylland, one per line."),
    Field("--history-days", Kind.TEXT, "History (days)", "Default 90."),
    Field("--bulletin-key", Kind.LIST, "Trusted bulletin keys", "One per line (128 hex characters)."),
    Field("--alert-root", Kind.LIST, "Alert root keys", "Alerts signed under these roots are shown as verified."),
)

private val SERVER_FIELDS = listOf(
    Field("--public-addr", Kind.TEXT, "My public address", "host:port others reach this phone at (router port forward)."),
    Field("--public", Kind.BOOL, "Join the open backbone"),
    Field("--peer", Kind.LIST, "Also connect to", "host:port, one per line."),
    Field("--no-post-office", Kind.BOOL_OFF, "Post office"),
    Field("--mail-quota-mb", Kind.TEXT, "Post office size (MB)", "0 = no limit."),
    Field("--min-peers", Kind.TEXT, "Minimum backbone links", "Default 6."),
    Field("--alert-root", Kind.LIST, "Alert root keys"),
)

/** settings.txt as (flag, value) lines, in order; unknown lines are kept. */
private fun parseSettings(t: String): List<Pair<String, String?>> = t.lines().map { it.trim() }.filter { it.isNotEmpty() && !it.startsWith("#") }.map { l ->
    val i = l.indexOfFirst { it.isWhitespace() }
    if (i < 0) l to null else l.substring(0, i) to l.substring(i).trim()
}

@Composable
internal fun HostSection(server: Boolean) {
    val ctx = LocalContext.current
    val fields = if (server) SERVER_FIELDS else HOST_FIELDS
    var raw by remember { mutableStateOf(HostFiles.readSettings(ctx, server)) }
    var advanced by remember { mutableStateOf(false) }
    var saved by remember { mutableStateOf<String?>(null) }
    val lines = parseSettings(raw)
    fun set(flag: String, values: List<String?>) {
        val others = lines.filter { it.first != flag }
        raw = (others + values.map { flag to it }).joinToString("\n") { (f, v) -> if (v.isNullOrEmpty()) f else "$f $v" }
        saved = null
    }
    SectionTitle(if (server) "Server on this phone" else "Phone node (maarse-host)")
    if (server) {
        val pw = HostFiles.adminPassword(ctx)
        Hint("Dashboard: http://<this phone's address>:8080 — log in with the admin password" + (pw?.let { ": $it" } ?: " (shown after the first start)."))
    } else Hint("Settings for the host running on this phone. Saved in its settings.txt; the node restarts.")
    if (!advanced) fields.forEach { f ->
        when (f.kind) {
            Kind.BOOL, Kind.BOOL_OFF -> {
                val present = lines.any { it.first == f.flag }
                val on = if (f.kind == Kind.BOOL) present else !present
                Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(f.label)
                        if (f.help.isNotEmpty()) Text(f.help, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    Switch(on, { v -> set(f.flag, if (v == (f.kind == Kind.BOOL)) listOf(null) else emptyList()) })
                }
            }
            Kind.TEXT -> OutlinedTextField(
                lines.lastOrNull { it.first == f.flag }?.second ?: "", { v -> set(f.flag, if (v.isBlank()) emptyList() else listOf(v.trim())) },
                Modifier.fillMaxWidth().padding(horizontal = 16.dp), label = { Text(f.label) }, singleLine = true,
                supportingText = if (f.help.isNotEmpty()) ({ Text(f.help) }) else null,
            )
            Kind.LIST -> OutlinedTextField(
                lines.filter { it.first == f.flag }.joinToString("\n") { it.second ?: "" },
                { v -> set(f.flag, v.lines().map { it.trim() }.filter { it.isNotEmpty() }) },
                Modifier.fillMaxWidth().padding(horizontal = 16.dp), label = { Text(f.label) },
                supportingText = if (f.help.isNotEmpty()) ({ Text(f.help) }) else null,
            )
        }
    } else OutlinedTextField(raw, { raw = it; saved = null }, Modifier.fillMaxWidth().padding(horizontal = 16.dp), label = { Text("settings.txt (one option per line)") })
    saved?.let { Hint(it) }
    Row(Modifier.padding(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Button(onClick = {
            HostFiles.writeSettings(ctx, server, raw)
            saved = "Saved. Restarting the node…"
            Node.restartHost()
        }) { Text("Save and restart") }
        TextButton(onClick = { advanced = !advanced }) { Text(if (advanced) "Simple" else "All options") }
    }
}

// ---------- the app ----------

@Composable
internal fun AppSection() {
    val ctx = LocalContext.current
    var msgs by remember { mutableStateOf(Node.prefs.notifyMessages) }
    var chans by remember { mutableStateOf(Node.prefs.notifyChannels) }
    SectionTitle("App")
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
        Text("Notify about direct messages", Modifier.weight(1f)); Switch(msgs, { msgs = it; Node.prefs.notifyMessages = it })
    }
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
        Text("Notify about channel messages", Modifier.weight(1f)); Switch(chans, { chans = it; Node.prefs.notifyChannels = it })
    }
    Hint("Alerts always notify. Severe, actual alerts use their own loud channel; change it in Android's notification settings.")
    val pm = ctx.getSystemService(PowerManager::class.java)
    if (!pm.isIgnoringBatteryOptimizations(ctx.packageName)) {
        Hint("Android may stop the connection when the screen is off. Allow Må:rse to run in the background:")
        @SuppressLint("BatteryLife")
        val go = { ctx.startActivity(Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:" + ctx.packageName))) }
        OutlinedButton(onClick = go, modifier = Modifier.padding(horizontal = 16.dp)) { Text("Allow background use") }
    }
    HorizontalDivider(Modifier.padding(top = 16.dp))
    Hint("Må:rse " + ctx.packageManager.getPackageInfo(ctx.packageName, 0).versionName)
}

@Composable
fun ServerPanel() {
    val ctx = LocalContext.current
    Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text("Server running", style = MaterialTheme.typography.titleMedium)
        Hint("This phone is a backbone node and post office. It stops by itself if it leaves Wi-Fi or the charger.")
        Hint("Dashboard with telemetry, connections and settings: http://<this phone's address>:8080")
        HostFiles.adminPassword(ctx)?.let { Hint("Admin password (first start): $it") }
    }
}

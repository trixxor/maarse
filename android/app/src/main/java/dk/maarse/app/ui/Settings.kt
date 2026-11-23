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
import androidx.compose.foundation.layout.Box
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Text as RawText
import dk.maarse.app.I18n
import dk.maarse.app.tr
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Switch
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
/** `radioPart = false`: name and position (Me). `radioPart = true`: role, regions and the radio (Radio). */
internal fun NodeSection(radioPart: Boolean = false) {
    val ctx = LocalContext.current
    val me by Node.me.collectAsState()
    val stats by Node.stats.collectAsState()
    val regions by Node.regions.collectAsState()
    val sharing by Node.sharing.collectAsState()
    val radio by Node.radio.collectAsState()
    val conn by Node.conn.collectAsState()

    if (!radioPart) {
    GroupTitle("Your name")
    Card2 {
        var name by remember(me?.name) { mutableStateOf(me?.name ?: "") }
        Text("Shown to others as ~name next to your address. Anyone can pick any name, so it is a hint, not a proof.",
            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Row(verticalAlignment = Alignment.CenterVertically) {
            OutlinedTextField(name, { name = it.take(24) }, Modifier.weight(1f), label = { Text("Name") }, singleLine = true)
            TextButton(onClick = { Node.send(Cmd.setName(name.trim())); Node.send(Cmd.getName()); Node.send(Cmd.announce()) }) { Text("Save") }
        }
    }

    }

    if (radioPart) {
    GroupTitle("Role — how this node helps others")
    Card2 {
        when (conn.mode) {
            Mode.BLE -> {
                val role = stats[7]
                Text("Your radio's role. It decides whether the radio passes on messages for others.",
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Names.ROLES.forEachIndexed { i, r ->
                    Row(Modifier.fillMaxWidth().clickable { Node.send(Cmd.setRole(i)); Node.send(Cmd.getStats()) }.heightIn(min = 48.dp).padding(vertical = 4.dp), verticalAlignment = Alignment.Top) {
                        RadioButton(role == i, { Node.send(Cmd.setRole(i)); Node.send(Cmd.getStats()) })
                        Column(Modifier.padding(top = 10.dp)) {
                            Text(r, fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold)
                            Text(Names.ROLE_HELP[i], style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    }
                }
                Text("Tip: a radio you carry around → Auto. A radio on a roof or mast → Repeater. In a forest or at sea with no repeaters → Relay.",
                    style = MaterialTheme.typography.bodySmall)
            }
            Mode.PHONE, Mode.SERVER -> {
                val server = conn.mode == Mode.SERVER
                val current = hostRole(ctx, server)
                var asked by remember { mutableStateOf<String?>(null) }
                Text(if (server) "This node is a server. Choose how it routes:" else "The phone comes and goes, so it is a client until you choose otherwise:",
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                listOf(
                    "client" to ((if (server) "Client" else "Client (default)") to "Sends and receives your own messages and forwards nothing for others. Nobody can depend on it, so it suits a phone."),
                    "repeater" to ("Repeater" to "Passes on messages for others, but does not split paths. Choose it for a node that is on most of the time."),
                    "anchor" to ((if (server) "Anchor (default)" else "Anchor") to "Passes on messages, splits long paths and answers path requests on behalf of others. Choose it for a node that is always on the internet."))
                    .forEach { (role, t) ->
                        Row(Modifier.fillMaxWidth().clickable { if (role != current) asked = role }.heightIn(min = 48.dp).padding(vertical = 4.dp), verticalAlignment = Alignment.Top) {
                            RadioButton(current == role, { if (role != current) asked = role })
                            Column(Modifier.padding(top = 10.dp)) {
                                Text(t.first, fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold)
                                Text(t.second, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                            }
                        }
                    }
                asked?.let { role ->
                    val name = role.replaceFirstChar { it.uppercase() }
                    AlertDialog(
                        onDismissRequest = { asked = null },
                        title = { Text("Switch to $name?") },
                        text = { Text("The node restarts to apply the new role. Messages may be delayed for a moment.") },
                        confirmButton = { TextButton(onClick = { setHostRole(ctx, server, role); asked = null }) { Text("Switch") } },
                        dismissButton = { TextButton(onClick = { asked = null }) { Text("Cancel") } },
                    )
                }
                Text("Relay and Auto are roles for a Må:rse radio. To set your radio's role, choose \"Through my radio only\" under How this phone connects, set it, and switch back. When the radio is the phone's LoRa modem, the phone does the routing.",
                    style = MaterialTheme.typography.bodySmall)
            }
            Mode.REMOTE -> Text("You follow a host on another computer. Its role is set on that computer (Host settings there); a radio's role is set when you connect to the radio over Bluetooth.",
                style = MaterialTheme.typography.bodySmall)
        }
    }

    }

    // Site is for a fixed node on a roof or mast: a repeater or an anchor. A client (a phone) has none.
    val fixedNode = when (conn.mode) {
        Mode.PHONE, Mode.SERVER -> hostRole(ctx, conn.mode == Mode.SERVER) != "client"
        Mode.BLE -> stats[7] == 1 || stats[7] == 3
        Mode.REMOTE -> true
    }
    if (radioPart && fixedNode) {
    GroupTitle("Site — for a fixed node")
    Card2 { SiteCard() }
    }

    if (!radioPart) {
    GroupTitle("Share my position")
    Card2 {
        val sh = sharing
        val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { r ->
            if (r.values.any { it }) Node.updatePosition(ctx)
        }
        Text("Shares the phone's location in announces. A server that shares its map can show your node on the public Denmark map, including over TCP. Choose a sharing precision to enable it.",
            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (sh == null) Text("Fetching…") else {
            PRECISION.forEachIndexed { i, p ->
                Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable {
                    Node.setPrecision(i)
                    if (i > 0 && !Node.hasLocation(ctx)) ask.launch(arrayOf(android.Manifest.permission.ACCESS_FINE_LOCATION, android.Manifest.permission.ACCESS_COARSE_LOCATION))
                }, verticalAlignment = Alignment.CenterVertically) {
                    RadioButton(sh.prec == i, null)
                    Text(p, Modifier.padding(start = 12.dp))
                }
            }
            if (sh.prec > 0) Text(
                if (sh.lat == 0.0 && sh.lon == 0.0) "No position yet" + (if (!Node.hasLocation(ctx)) " — allow location for Må:rse" else " — waiting for the phone's location")
                else "Shared: %.3f, %.3f (updated every 10 minutes)".format(sh.lat, sh.lon),
                style = MaterialTheme.typography.bodySmall,
            )
            Row(Modifier.fillMaxWidth().heightIn(min = 56.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text("Share battery and uptime")
                    Text("Helps others see if a node is healthy.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                Switch(sh.telemetry, { Node.send(Cmd.setSharing(sh.prec, (sh.lat * 1e7).toInt(), (sh.lon * 1e7).toInt(), it)); Node.send(Cmd.getSharing()) })
            }
        }
    }

    }

    if (radioPart) {
    GroupTitle("Regions")
    Card2 {
        var reg by remember(regions) { mutableStateOf(regions?.names ?: "") }
        Text("Channel messages marked with a region (e.g. #help@nordjylland) only travel where nodes have that region. Leave empty to get everything.",
            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Row(verticalAlignment = Alignment.CenterVertically) {
            OutlinedTextField(reg, { reg = it }, Modifier.weight(1f), label = { Text("e.g. dk, nordjylland") }, singleLine = true)
            TextButton(onClick = { Node.send(Cmd.setRegions(regions?.strict ?: false, reg.trim())); Node.send(Cmd.getRegions()) }) { Text("Save") }
        }
    }

    radio?.let { r ->
        GroupTitle("Radio")
        Card2 {
            Text("%.3f MHz · SF%d · %.1f kHz · %d dBm".format(r.freq / 1e6, r.sf, r.bw / 1e3, r.power), fontWeight = androidx.compose.ui.text.font.FontWeight.Medium)
            Text("All Må:rse radios use the same settings, so everyone hears everyone.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
    }

    if (!radioPart) {
        Card2(Modifier.padding(top = 8.dp)) {
            Text("Tell others who you are right away, instead of waiting for the next automatic announce.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            OutlinedButton(onClick = { Node.send(Cmd.announce()) }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Announce now") }
        }
        Hint("Address: " + (me?.addr ?: ""))
    }
}

/** The node's routing role in a few words, for the Settings list. */
@Composable
internal fun roleSummary(): String {
    val ctx = LocalContext.current
    val conn by Node.conn.collectAsState()
    val stats by Node.stats.collectAsState()
    return when (conn.mode) {
        Mode.PHONE, Mode.SERVER -> when (hostRole(ctx, conn.mode == Mode.SERVER)) {
            "client" -> "Client · forwards nothing for others"
            "repeater" -> "Repeater · passes messages on for others"
            else -> "Anchor · passes messages on and splits long paths"
        }
        Mode.BLE -> stats[7]?.let { Names.ROLES.getOrNull(it) }?.let { "$it · your radio's role" } ?: "How your radio helps others"
        Mode.REMOTE -> "Set on the computer you follow"
    }
}

/** The routing role of the phone's own node: what its settings say, else the default for its kind
 *  (a phone node is a client, a server an anchor). Settings: `--role client|repeater|anchor`. */
internal fun hostRole(ctx: Context, server: Boolean): String {
    val lines = HostFiles.readSettings(ctx, server).lines().map { it.trim() }
    lines.firstOrNull { it.startsWith("--role ") }?.substringAfter(' ')?.trim()?.lowercase()
        ?.takeIf { it in listOf("client", "repeater", "anchor") }?.let { return it }
    if (lines.any { it == "--repeater-role" }) return "repeater"
    return if (server) "anchor" else "client"
}

/** Saves the role in the node's settings, then restarts the node. */
private fun setHostRole(ctx: Context, server: Boolean, role: String) {
    val lines = HostFiles.readSettings(ctx, server).lines()
        .filter { it.trim().isNotEmpty() && it.trim() != "--repeater-role" && !it.trim().startsWith("--role ") }
    HostFiles.writeSettings(ctx, server, (lines + "--role $role").joinToString("\n"))
    Node.restartHost()
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
    Field("--public-map", Kind.BOOL, "Share the public map"),
    Field("--public-traffic", Kind.BOOL, "Share live traffic", "Requires Share the public map. Publishes recent packet counts and observations of verified announces."),
    Field("--share-open-channels", Kind.BOOL, "Share open channels", "Requires Share the public map. Automatically publishes activity and new text from open channels this server can identify."),
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
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) { fields.forEach { f ->
        when (f.kind) {
            Kind.BOOL, Kind.BOOL_OFF -> {
                val present = lines.any { it.first == f.flag }
                val on = if (f.kind == Kind.BOOL) present else !present
                Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
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
    } }
    saved?.let { Hint(it) }
    Row(Modifier.padding(horizontal = 16.dp, vertical = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Button(modifier = Modifier.heightIn(min = 48.dp), onClick = {
            HostFiles.writeSettings(ctx, server, raw)
            saved = "Saved. Restarting the node…"
            Node.restartHost()
        }) { Text("Save and restart") }
    }
}

// ---------- the app ----------

@Composable
internal fun AppSection() {
    val ctx = LocalContext.current
    var msgs by remember { mutableStateOf(Node.prefs.notifyMessages) }
    var chans by remember { mutableStateOf(Node.prefs.notifyChannels) }
    Column(verticalArrangement = Arrangement.spacedBy(0.dp)) {
        GroupTitle("Notifications")
        Group {
            SwitchRow("Direct messages", "Notify when someone writes to you", msgs) { msgs = it; Node.prefs.notifyMessages = it }
            SwitchRow("Channel messages", "Notify about new messages in your channels", chans, last = true) { chans = it; Node.prefs.notifyChannels = it }
        }
        Hint("Alerts always notify. Severe, actual alerts use their own loud channel; change it in Android's notification settings.")
        SoundPickers()
        GroupTitle("Language")
        Group { LanguageRow(last = true) }
        GroupTitle("Background")
        Group { BackgroundRow(last = true) }
        Hint("Må:rse " + ctx.packageManager.getPackageInfo(ctx.packageName, 0).versionName)
    }
}

/** Keep the connection alive with the screen off: Android's battery optimisation exemption, shown as a switch. */
@SuppressLint("BatteryLife")
@Composable
internal fun BackgroundRow(last: Boolean = false) {
    val ctx = LocalContext.current
    val pm = ctx.getSystemService(PowerManager::class.java)
    var allowed by remember { mutableStateOf(pm.isIgnoringBatteryOptimizations(ctx.packageName)) }
    // Android decides on its own screen: read the answer again when the person comes back.
    val owner = androidx.lifecycle.compose.LocalLifecycleOwner.current
    DisposableEffect(owner) {
        val obs = androidx.lifecycle.LifecycleEventObserver { _, e ->
            if (e == androidx.lifecycle.Lifecycle.Event.ON_RESUME) allowed = pm.isIgnoringBatteryOptimizations(ctx.packageName)
        }
        owner.lifecycle.addObserver(obs)
        onDispose { owner.lifecycle.removeObserver(obs) }
    }
    SwitchRow("Keep running in the background", if (allowed) "Allowed. Messages arrive with the screen off." else "Android may stop the connection when the screen is off.", allowed, last) {
        ctx.startActivity(
            if (it) Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS, Uri.parse("package:" + ctx.packageName))
            else Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS),
        )
    }
}

/** The names of the choices in the Language row: Automatic (the device's language), English, or a language file. */
private fun languageOptions() = listOf(I18n.AUTO to tr("Automatic (device language)"), "en" to "English") + I18n.languages.map { it.code to it.name }

/** Language as a row of the Me screen: the current choice on the right, a menu to change it. */
@Composable
internal fun LanguageRow(last: Boolean = false, leading: (@Composable () -> Unit)? = null) {
    val ctx = LocalContext.current
    var open by remember { mutableStateOf(false) }
    val options = languageOptions()
    val current = options.firstOrNull { it.first == I18n.choice }?.second ?: options[0].second
    Box {
        ValueRow("Language", current, last, leading) { open = true }
        DropdownMenu(open, { open = false }) {
            for ((code, name) in options) DropdownMenuItem(text = { RawText(name) }, onClick = { open = false; I18n.setChoice(ctx, code) }, modifier = Modifier.heightIn(min = 48.dp))
        }
    }
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


/** Site of a fixed node (repeater, anchor, host on a roof or mast): where it
 *  stands, how high the antenna is, its gain and who owns it. Sent in the
 *  node's announces, so maps and coverage pictures can use the real numbers. */
@SuppressLint("MissingPermission")
@Composable
private fun SiteCard() {
    val ctx = LocalContext.current
    val site by Node.site.collectAsState()
    val sharing by Node.sharing.collectAsState()
    val s = site
    var height by remember(s) { mutableStateOf(if ((s?.heightDm ?: 0) > 0) "%.1f".format(s!!.heightDm / 10.0) else "") }
    var gain by remember(s) { mutableStateOf(if (s != null && s.heightDm > 0) "%.1f".format(s.gainHdb / 2.0) else "") }
    var owner by remember(s) { mutableStateOf(s?.owner ?: "") }
    var lat by remember(sharing) { mutableStateOf(sharing?.takeIf { it.prec > 0 && it.lat != 0.0 }?.let { "%.5f".format(java.util.Locale.ROOT, it.lat) } ?: "") }
    var lon by remember(sharing) { mutableStateOf(sharing?.takeIf { it.prec > 0 && it.lon != 0.0 }?.let { "%.5f".format(java.util.Locale.ROOT, it.lon) } ?: "") }
    Text("For a radio on a roof or mast. The node sends this in its announces, so maps and coverage pictures use the real antenna height, gain and power. Everything here is public: anyone can read it.",
        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    if (s == null) { Text("Fetching…"); return }
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedTextField(height, { height = it.filter { c -> c.isDigit() || c == '.' || c == ',' }.take(6) }, Modifier.weight(1f), label = { Text("Antenna height, m") }, singleLine = true)
        OutlinedTextField(gain, { gain = it.filter { c -> c.isDigit() || c == '.' || c == ',' || c == '-' }.take(5) }, Modifier.weight(1f), label = { Text("Antenna gain, dBi") }, singleLine = true)
    }
    OutlinedTextField(owner, { owner = it.take(40) }, Modifier.fillMaxWidth(), label = { Text("Owner: name or call sign (and a contact, if you want it public)") }, singleLine = true)
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
        OutlinedTextField(lat, { lat = it.take(12) }, Modifier.weight(1f), label = { Text("Latitude") }, singleLine = true)
        OutlinedTextField(lon, { lon = it.take(12) }, Modifier.weight(1f), label = { Text("Longitude") }, singleLine = true)
    }
    TextButton(onClick = {
        if (!Node.hasLocation(ctx)) return@TextButton
        val lm = ctx.getSystemService(android.location.LocationManager::class.java)
        lm.getProviders(true).mapNotNull { runCatching { lm.getLastKnownLocation(it) }.getOrNull() }.maxByOrNull { it.time }?.let {
            lat = "%.5f".format(java.util.Locale.ROOT, it.latitude); lon = "%.5f".format(java.util.Locale.ROOT, it.longitude)
        }
    }) { Text("Use this phone's position (stand at the mast)") }
    if ((s.txDbm) != 0 && s.heightDm > 0) Text("Transmit power ${s.txDbm} dBm (from the radio's settings).", style = MaterialTheme.typography.bodySmall)
    Row {
        TextButton(onClick = {
            val h = height.replace(',', '.').toDoubleOrNull() ?: 0.0
            val g = gain.replace(',', '.').toDoubleOrNull() ?: 0.0
            Node.send(Cmd.setSite((h * 10).toInt().coerceIn(0, 65535), (g * 2).toInt().coerceIn(-128, 127), owner.trim()))
            val la = lat.replace(',', '.').toDoubleOrNull(); val lo = lon.replace(',', '.').toDoubleOrNull()
            if (h > 0 && la != null && lo != null) {
                // A site's position is shared precisely: it is a mast, not a person.
                Node.send(Cmd.setSharing(3, (la * 1e7).toInt(), (lo * 1e7).toInt(), sharing?.telemetry ?: true))
                Node.send(Cmd.getSharing())
            }
            Node.send(Cmd.getSite()); Node.send(Cmd.announce())
        }) { Text("Save") }
        if (s.heightDm > 0) TextButton(onClick = { Node.send(Cmd.setSite(0, 0, "")); Node.send(Cmd.getSite()) }) { Text("Not a fixed node") }
    }
}

package dk.maarse.app.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.automirrored.outlined.Chat
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.outlined.Computer
import androidx.compose.material.icons.outlined.Dns
import androidx.compose.material.icons.outlined.Memory
import androidx.compose.material.icons.outlined.Notifications
import androidx.compose.material.icons.outlined.People
import androidx.compose.material.icons.outlined.Person
import androidx.compose.material.icons.outlined.PinDrop
import androidx.compose.material.icons.outlined.Public
import androidx.compose.material.icons.outlined.Router
import androidx.compose.material.icons.outlined.SettingsInputAntenna
import androidx.compose.material.icons.outlined.Smartphone
import androidx.compose.material.icons.outlined.Tune
import androidx.compose.material.icons.outlined.Wifi
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dk.maarse.app.Cmd
import dk.maarse.app.Conn
import dk.maarse.app.HostFiles
import dk.maarse.app.Mode
import dk.maarse.app.ModemBridge
import dk.maarse.app.Node
import dk.maarse.app.PRECISION
import dk.maarse.app.PersonState
import dk.maarse.app.R
import dk.maarse.app.Sounds
import dk.maarse.app.hex
import dk.maarse.app.tr
import kotlinx.coroutines.DelicateCoroutinesApi
import kotlinx.coroutines.GlobalScope
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

/** The "Me" side of the app: connection choices, the Settings pages, and the first-start screens. */

// ---------- switching mode ----------

private data class ModeChoice(val mode: Mode, val icon: ImageVector, val title: String, val text: String)

private val CHOICES = listOf(
    ModeChoice(Mode.PHONE, Icons.Outlined.Smartphone, "This phone is the node", "Recommended. Works over the internet; add your radio for LoRa. Your own address and account."),
    ModeChoice(Mode.BLE, Icons.Outlined.Router, "Through my radio only", "The radio over Bluetooth is the node. No internet needed; fewer features."),
    ModeChoice(Mode.REMOTE, Icons.Outlined.Computer, "My home computer", "Follow the maarse-host on your PC (on Wi-Fi or over VPN) with its access key."),
)

private val SERVER_CHOICE = ModeChoice(Mode.SERVER, Icons.Outlined.Dns, "Server on this phone",
    "An old phone on Wi-Fi and a charger as a backbone node and post office. Stops when Wi-Fi or power goes.")

@Composable
private fun ModeCard(c: ModeChoice, on: Boolean, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(16.dp))
            .border(BorderStroke(if (on) 2.dp else 1.dp, if (on) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outlineVariant), RoundedCornerShape(16.dp))
            .background(if (on) MaterialTheme.colorScheme.secondaryContainer else MaterialTheme.colorScheme.surfaceContainer)
            .selectable(selected = on, role = Role.RadioButton, onClick = onClick).padding(14.dp),
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Box(Modifier.size(40.dp).clip(RoundedCornerShape(12.dp)).background(MaterialTheme.colorScheme.primaryContainer), contentAlignment = Alignment.Center) {
            Icon(c.icon, null, tint = MaterialTheme.colorScheme.onPrimaryContainer)
        }
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Text(c.title, fontWeight = FontWeight.SemiBold)
            Text(c.text, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        if (on) Icon(Icons.Default.Check, "Chosen", tint = MaterialTheme.colorScheme.primary)
    }
}

/** The connection choices, used by the sheet and the first-start guide. `advanced` adds the server mode. */
@Composable
fun ModeChooser(advanced: Boolean = false, onConnected: () -> Unit) {
    val ctx = LocalContext.current
    val conn by Node.conn.collectAsState()
    // First start: the recommended choice; later: what is in use.
    var mode by remember { mutableStateOf(if (!Node.prefs.onboarded || (conn.mode == Mode.SERVER && !advanced)) Mode.PHONE else conn.mode) }
    var refusal by remember { mutableStateOf<String?>(null) }
    var picking by remember { mutableStateOf(false) }
    var radio by remember { mutableStateOf(Node.prefs.bleName) }
    var remote by remember { mutableStateOf(Node.prefs.remoteAddr) }
    var key by remember { mutableStateOf(Node.prefs.remoteKey) }
    var modem by remember { mutableStateOf(ModemBridge.enabled(ctx)) }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        (if (advanced) CHOICES + SERVER_CHOICE else CHOICES).forEach { c -> ModeCard(c, mode == c.mode) { mode = c.mode } }
        when (mode) {
            // The radio as a LoRa modem: one switch, or a way to choose the radio first.
            Mode.PHONE -> Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Column(Modifier.weight(1f)) {
                    Text("Use my radio as LoRa modem")
                    Text(radio?.let { "$it · over Bluetooth" } ?: "Choose a radio first", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                if (radio == null) TextButton(onClick = { picking = true }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Choose") }
                else Switch(modem, { modem = it })
            }
            Mode.BLE -> Row(Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(radio?.let { "Radio: $it" } ?: "No radio chosen yet", Modifier.weight(1f))
                TextButton(onClick = { picking = true }, modifier = Modifier.heightIn(min = 48.dp)) { Text(if (radio == null) "Choose radio" else "Change") }
            }
            Mode.REMOTE -> Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(remote, { remote = it; Node.prefs.remoteAddr = it }, Modifier.fillMaxWidth(), singleLine = true, label = { Text("Address, e.g. 192.168.1.20:8080") })
                OutlinedTextField(key, { key = it; Node.prefs.remoteKey = it }, Modifier.fillMaxWidth(), singleLine = true, label = { Text("Access key (shown by the host)") })
            }
            Mode.SERVER -> Text("Needs Wi-Fi and a charger, a port opened in the router and a public address (under Internet link and post office). Admin dashboard on port 8080.",
                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        val ready = when (mode) { Mode.BLE -> radio != null; Mode.REMOTE -> remote.isNotBlank(); else -> true }
        Button(
            onClick = {
                refusal = null
                if (mode == Mode.SERVER && !HostFiles.serverConditionsMet(ctx)) { refusal = "Server mode needs Wi-Fi and a charger."; return@Button }
                // The host reads the modem setting at start: set it first.
                val changed = mode == Mode.PHONE && radio != null && ModemBridge.enabled(ctx) != modem
                if (changed) ModemBridge.configure(ctx, modem)
                Node.start(mode)
                if (changed) Node.restartHost()
                onConnected()
            },
            enabled = ready, modifier = Modifier.fillMaxWidth().height(48.dp),
        ) { Text(if (conn.state != Conn.State.OFF && conn.mode == mode) "Reconnect" else "Connect") }
        refusal?.let { Text(it, color = MaterialTheme.colorScheme.error) }
        if (conn.state != Conn.State.OFF) TextButton(onClick = { Node.stop(); onConnected() }, modifier = Modifier.align(Alignment.CenterHorizontally).heightIn(min = 48.dp)) { Text("Disconnect") }
    }
    if (picking) BlePicker { addr, name ->
        picking = false
        if (addr != null) { Node.prefs.bleAddress = addr; Node.prefs.bleName = name; radio = name }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ConnectionSheet(onDismiss: () -> Unit) {
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), containerColor = MaterialTheme.colorScheme.surface) {
        // imePadding first, so the keyboard never covers the address and key fields of the home computer.
        Column(Modifier.imePadding().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp).padding(bottom = 24.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text("How this phone connects", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold)
            Text("You can change this any time. Your chats and your account stay.", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.height(6.dp))
            ModeChooser(advanced = true, onConnected = onDismiss)
        }
    }
}

// ---------- you ----------

@Composable
fun MeScreen(nav: Nav) {
    val ctx = LocalContext.current
    val meshcore by Node.meshcoreAccess.collectAsState()
    val me by Node.me.collectAsState()
    val p by Node.person.collectAsState()
    val conn by Node.conn.collectAsState()
    val sharing by Node.sharing.collectAsState()
    val host = conn.mode == Mode.PHONE || conn.mode == Mode.REMOTE
    val hostPage = conn.mode == Mode.PHONE || conn.mode == Mode.SERVER
    // The sheets belong to this screen only, so leaving Me and coming back never shows an old one.
    var sending by remember { mutableStateOf(false) }
    var repeaters by remember { mutableStateOf(false) }

    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
        // Profile first: who you are and how you can be reached.
        Row(
            Modifier.padding(horizontal = 16.dp, vertical = 4.dp).fillMaxWidth().clip(RoundedCornerShape(18.dp))
                .background(MaterialTheme.colorScheme.surfaceContainer).clickable { nav.go(Screen.Page(MePage.NODE)) }.padding(16.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            MeAvatar(56.dp) { nav.go(Screen.Page(MePage.NODE)) }
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(me?.name?.takeIf { it.isNotBlank() } ?: "No name yet", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(
                    when {
                        p?.state == PersonState.MEMBER -> tr("Account {0}…", p!!.person.take(8))
                        me != null -> tr("Address {0}…", me!!.addr.take(8))
                        else -> "Not connected"
                    },
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis,
                )
                if (p?.state == PersonState.MEMBER) {
                    val n = p!!.devices.count { !it.second }
                    Text(if (n == 1) tr("1 device") else tr("{0} devices", n), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
            Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, null, tint = MaterialTheme.colorScheme.outline)
        }

        GroupTitle("Må:rse")
        Group {
            if (host) GroupRow("Account and devices", when (p?.state) {
                PersonState.MEMBER -> if (p!!.master) "This is the master device · backup, add devices" else "Linked to your account"
                PersonState.AWAITING -> "Waiting to be linked"
                else -> "One address for your phone, PC and more"
            }, leading = { RowIcon(Icons.Outlined.Person) }) { nav.go(Screen.Page(MePage.ACCOUNT)) }
            GroupRow("How this phone connects", connectionSummary(), leading = { RowIcon(Icons.Outlined.Wifi) }) { nav.sheet = true }
            GroupRow("Name and position", when {
                sharing == null -> "What your announces share"
                sharing!!.prec == 0 -> "Position not shared"
                else -> tr("Position shared: {0}", tr(PRECISION.getOrElse(sharing!!.prec) { "" }).lowercase())
            }, leading = { RowIcon(Icons.Outlined.PinDrop) }) { nav.go(Screen.Page(MePage.NODE)) }
            if (hostPage) GroupRow("Internet link and post office", "Backbone, peers, mail for others, keys", leading = { RowIcon(Icons.Outlined.Dns) }) { nav.go(Screen.Page(MePage.HOST)) }
            GroupRow("Role and regions", roleSummary(), leading = { RowIcon(Icons.Outlined.Tune) }) { nav.go(Screen.Page(MePage.RADIO)) }
            GroupRow("Firmware", "Update your radios over USB, Bluetooth or Wi-Fi", last = true, leading = { RowIcon(Icons.Outlined.Memory) }) { nav.go(Screen.Page(MePage.FIRMWARE)) }
        }

        // MeshCore is a separate network with its own identity: its settings are grouped, never mixed in above.
        McGroupTitle("MeshCore", "separate identity and radio network")
        Group {
            GroupRow("MeshCore setup", if (meshcore.key != null) "Access and identity. Messages are in Chats" else "Backbone address and access",
                leading = { RowIcon(Icons.Outlined.SettingsInputAntenna, meshcore = true) }) { nav.go(Screen.Page(MePage.MESHCORE)) }
            GroupRow("Sending settings",
                if (meshcore.key != null) tr("{0} byte path hash", meshcore.radio.hashSize) + (if (meshcore.radio.scope.isEmpty()) " · " + tr("no region") else " · " + meshcore.radio.scope.split(",").joinToString(", ") { "#$it" })
                else "Set up MeshCore first",
                leading = { RowIcon(Icons.Outlined.Tune, meshcore = true) }) { if (meshcore.key != null) sending = true else nav.go(Screen.Page(MePage.MESHCORE)) }
            GroupRow("Repeater admin", "Status and settings of a repeater you can log in to", last = true,
                leading = { RowIcon(Icons.Outlined.Router, meshcore = true) }) { repeaters = true }
        }

        GroupTitle("Notifications")
        Group {
            GroupRow("Notifications and sounds", soundSummary(ctx), last = true, leading = { RowIcon(Icons.Outlined.Notifications) }) { nav.go(Screen.Page(MePage.APP)) }
        }

        GroupTitle("This phone")
        Group {
            LanguageRow(leading = { RowIcon(Icons.Outlined.Public) })
            BackgroundRow(last = true)
        }
        Hint("Må:rse " + ctx.packageManager.getPackageInfo(ctx.packageName, 0).versionName)
    }

    if (sending) SendingSheet(meshcore.radio) { sending = false }
    if (repeaters) RepeaterSheet(nav, meshcore.nodes) { repeaters = false }
}

/** Path hash size and region scope for everything this account sends over MeshCore. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SendingSheet(radio: Node.McRadio, onDismiss: () -> Unit) {
    var size by remember { mutableIntStateOf(radio.hashSize.coerceIn(1, 3)) }
    var region by remember { mutableStateOf(radio.scope) }
    var error by remember { mutableStateOf("") }
    var saving by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), containerColor = MaterialTheme.colorScheme.surface) {
        Column(Modifier.imePadding().verticalScroll(rememberScrollState()).padding(bottom = 24.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Sending settings", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold)
                NetworkTag()
            }
            Text("Path hash size", Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
            SegmentedTabs(listOf("1 byte", "2 bytes", "3 bytes"), size - 1, { size = it + 1 })
            OutlinedTextField(
                region, { region = it.replace(" ", "").replace("#", "") }, Modifier.padding(horizontal = 16.dp).fillMaxWidth(), singleLine = true,
                label = { Text("Regions, e.g. dk,dk-nordjylland") },
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
            )
            Text("2 or 3 bytes need repeaters that support it. A region makes only repeaters with that region pass your messages on. With several regions, each message is sent once per region.",
                Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            if (error.isNotEmpty()) Text(error, Modifier.padding(horizontal = 16.dp), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
            Button(
                onClick = {
                    if (region.isNotEmpty() && (region.split(",").size > 8 || !region.split(",").all { Regex("[A-Za-z0-9-]{1,24}").matches(it) })) {
                        error = "Up to 8 regions, separated by commas. A region uses letters, digits and -, up to 24 characters."
                    } else {
                        error = ""; saving = true
                        scope.launch {
                            val answer = Node.mcCall("set-radio:$size:${region.toByteArray().hex()}")
                            saving = false
                            if (answer?.radio?.hashSize == size && answer.radio.scope == region) { Node.mcAsk("overview"); onDismiss() }
                            else error = "Not saved. The backbone server did not accept it: it probably needs updating to the newest version."
                        }
                    }
                },
                enabled = !saving, modifier = Modifier.padding(horizontal = 16.dp).fillMaxWidth().height(48.dp),
            ) { Text(if (saving) "Saving…" else "Save") }
        }
    }
}

/** The repeaters (and rooms and sensors) your MeshCore radio hears. Opening one shows its page; logging in happens there. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun RepeaterSheet(nav: Nav, nodes: List<Node.McNode>, onDismiss: () -> Unit) {
    val list = nodes.filter { it.type in 2..4 }.sortedWith(compareBy({ it.hops }, { it.name }))
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), containerColor = MaterialTheme.colorScheme.surface) {
        Column(Modifier.verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
            Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Repeater admin", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold)
                NetworkTag()
            }
            Hint("Choose a repeater, room or sensor. You log in on its page with its own password; nothing is sent until you do.")
            if (list.isEmpty()) Hint("None heard yet. Repeaters your MeshCore radio hears show up here.")
            Group {
                list.forEachIndexed { i, n ->
                    GroupRow(n.name.ifBlank { n.key.take(8) },
                        tr(when (n.type) { 3 -> "Room"; 4 -> "Sensor"; else -> "Repeater" }) + " · " + (if (n.hops == 0) tr("direct") else if (n.hops == 1) tr("1 hop") else tr("{0} hops", n.hops)),
                        last = i == list.lastIndex, leading = { RowIcon(Icons.Outlined.Router, meshcore = true) },
                    ) {
                        McNav.node = n.key; McNav.nodeMode = true
                        nav.select(Tab.CONTACTS, reset = false)
                        onDismiss()
                    }
                }
            }
        }
    }
}

@Composable
fun MePageContent(p: MePage) {
    val conn by Node.conn.collectAsState()
    val me by Node.me.collectAsState()
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).imePadding().padding(bottom = 24.dp)) {
        when (p) {
            MePage.ACCOUNT -> if (me != null) AccountSection() else Hint("Connect first.")
            MePage.MESHCORE -> if (me == null) Hint("Connect first.") else {
                val account by Node.person.collectAsState()
                if (account != null && account?.state != PersonState.MEMBER) {
                    // MeshCore proves access with the account: set that up first, right here.
                    Hint(Node.meshcoreAccountNote() ?: "MeshCore needs an account.")
                    AccountSection()
                } else MeshCoreAccessSection(setup = true)
            }
            MePage.NODE -> if (me != null) NodeSection() else Hint("Connect first.")
            MePage.RADIO -> if (me != null) NodeSection(radioPart = true) else Hint("Connect first.")
            MePage.APP -> AppSection()
            MePage.FIRMWARE -> FirmwareSection()
            MePage.HOST -> when (conn.mode) {
                Mode.PHONE -> HostSection(server = false)
                Mode.SERVER -> HostSection(server = true)
                else -> Hint("Host settings are for the phone node and server mode.")
            }
        }
    }
}

// ---------- first start ----------

/** The name typed in the first-start guide, before the node is running. It is sent as soon as the node has an address. */
@OptIn(DelicateCoroutinesApi::class)
private fun sendNameWhenReady(name: String) {
    if (name.isBlank()) return
    GlobalScope.launch {
        withTimeoutOrNull(120_000) { Node.me.first { it != null } } ?: return@launch
        Node.send(Cmd.setName(name.trim())); Node.send(Cmd.getName()); Node.send(Cmd.announce())
    }
}

/** Three short steps: welcome, what to call you, how this phone connects (the recommended choice is chosen). */
@Composable
fun Welcome(nav: Nav) {
    var step by remember { mutableIntStateOf(0) }
    var name by remember { mutableStateOf("") }
    BackHandler(enabled = step > 0) { step-- }
    val title = listOf("Welcome to Må:rse", "What should we call you?", "How will you use Må:rse?")[step]
    val text = listOf(
        "With or without the internet. Må:rse uses the internet when it is there and radio when it is not.",
        "This is what others see next to your address. You can change it later.",
        "Most people choose the first one. You can change it any time.",
    )[step]
    fun finish() { Node.prefs.onboarded = true; nav.home() }
    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().imePadding().verticalScroll(rememberScrollState()).padding(24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        // Progress: three dots, the steps done and the current one filled.
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            for (i in 0..2) Box(Modifier.weight(1f).height(4.dp).clip(CircleShape).background(if (i <= step) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outlineVariant))
        }
        Spacer(Modifier.height(8.dp))
        Box(Modifier.size(64.dp).clip(RoundedCornerShape(16.dp)).background(Green), contentAlignment = Alignment.Center) {
            when (step) {
                0 -> Icon(painterResource(R.drawable.ic_stat), null, Modifier.size(40.dp), tint = androidx.compose.ui.graphics.Color.White)
                1 -> Icon(Icons.Outlined.People, null, Modifier.size(34.dp), tint = androidx.compose.ui.graphics.Color.White)
                else -> Icon(Icons.Outlined.Wifi, null, Modifier.size(34.dp), tint = androidx.compose.ui.graphics.Color.White)
            }
        }
        Text(title, style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold)
        Text(text, style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
        when (step) {
            1 -> {
                OutlinedTextField(
                    name, { name = it.take(24) }, Modifier.fillMaxWidth(), singleLine = true, label = { Text("Your name") },
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Next), keyboardActions = KeyboardActions(onNext = { step = 2 }),
                )
                Hint("Anyone can pick any name, so it is a hint, not a proof.")
            }
            2 -> ModeChooser(onConnected = {
                sendNameWhenReady(name)
                finish()
                if (Node.prefs.mode == Mode.PHONE || Node.prefs.mode == Mode.REMOTE) nav.go(Screen.WelcomeAccount)
            })
        }
        if (step < 2) Button(onClick = { step++ }, modifier = Modifier.fillMaxWidth().height(48.dp)) { Text(if (step == 0) "Get started" else "Continue") }
        TextButton(onClick = { finish() }, modifier = Modifier.align(Alignment.CenterHorizontally).heightIn(min = 48.dp)) { Text("Skip for now") }
    }
}

@Composable
fun WelcomeAccount(nav: Nav) {
    val me by Node.me.collectAsState()
    val account by Node.person.collectAsState()
    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().imePadding().verticalScroll(rememberScrollState()).padding(vertical = 24.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text("Your account", Modifier.padding(horizontal = 24.dp), style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold)
        Text("Create your account, or link this phone to the one on your computer.",
            Modifier.padding(horizontal = 24.dp), style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (me == null) Hint("Starting the node…") else {
            // The name was asked in the first step; ask again only when it is still empty.
            if (me?.name.isNullOrBlank()) {
                var name by remember { mutableStateOf("") }
                GroupTitle("Your name")
                Card2 {
                    Text("Shown to others next to your address.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        OutlinedTextField(name, { name = it.take(24) }, Modifier.weight(1f), singleLine = true, label = { Text("Name") })
                        TextButton(onClick = { Node.send(Cmd.setName(name.trim())); Node.send(Cmd.getName()) }, modifier = Modifier.heightIn(min = 48.dp)) { Text("Save") }
                    }
                }
            }
            AccountSection()
        }
        Spacer(Modifier.height(8.dp))
        Button(onClick = { nav.home() }, modifier = Modifier.padding(horizontal = 24.dp).fillMaxWidth().height(48.dp)) {
            Text(if (account?.state == PersonState.NONE) "Later" else "Done")
        }
        if (account?.state == PersonState.NONE) {
            Text("You can do this later under Settings → Account and devices. MeshCore needs an account.",
                Modifier.padding(horizontal = 24.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

/** One line for the Notifications row in Me: which kinds notify, the tones live on the page behind it. */
private fun soundSummary(ctx: android.content.Context): String {
    val on = listOf(Node.prefs.notifyMessages, Node.prefs.notifyChannels).count { it }
    return if (on == 2) tr("Direct and channels on") else if (on == 0) tr("Messages off") else tr("Some messages off")
}

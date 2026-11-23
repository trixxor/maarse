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
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.outlined.Chat
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.outlined.Computer
import androidx.compose.material.icons.outlined.Dns
import androidx.compose.material.icons.outlined.Hub
import androidx.compose.material.icons.outlined.People
import androidx.compose.material.icons.outlined.Public
import androidx.compose.material.icons.outlined.Settings
import androidx.compose.material.icons.outlined.Router
import androidx.compose.material.icons.outlined.Smartphone
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.TextButton
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import kotlinx.coroutines.delay
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dk.maarse.app.Conn
import dk.maarse.app.HostFiles
import dk.maarse.app.Mode
import dk.maarse.app.ModemBridge
import dk.maarse.app.Node
import dk.maarse.app.Sensors
import dk.maarse.app.PersonState
import dk.maarse.app.R

enum class Tab(val title: String) { CHATS("Chats"), PAGES("Pages"), CONTACTS("Nodes"), NETWORK("Network"), SETTINGS("Settings") }

/** Pages behind "You". */
enum class MePage(val title: String) {
    ACCOUNT("Account and devices"), MESHCORE("MeshCore setup"), NODE("Name and position"), RADIO("Role and regions"), APP("Notifications and background"),
    HOST("Internet link and post office"),
}

sealed interface Screen {
    data object Home : Screen
    data class Chat(val conv: String) : Screen
    data class Contact(val addr: String) : Screen
    data object Me : Screen
    data class Page(val page: MePage) : Screen
    data class Net(val page: NetPage) : Screen
    data object Welcome : Screen
    data object WelcomeAccount : Screen
    /** Sensors (Nodes > Sensors): one sensor, the start page, the designer, a rule and the test-and-send page. */
    data class Sensor(val node: String) : Screen
    data object SensorStart : Screen
    data object SensorDesign : Screen
    data class SensorRule(val index: Int) : Screen
    data object SensorSend : Screen
}

/** Navigation: five places in a bar that never hides; each place has its own
 *  small stack of screens (a chat, a contact, a sub-page) and remembers it.
 *  Back: one step back in the place you are in; with nothing left to go back
 *  to it goes to Chats, and from Chats it leaves the app. A tap on the place
 *  you are already in takes you to its top. */
class Nav(start: Screen) {
    private val stacks = Tab.entries.associateWith { mutableStateListOf<Screen>() }
    var tab by mutableStateOf(Tab.CHATS)
    var sheet by mutableStateOf(false)
    /** The first-start screens (no bar), until they are done. */
    var welcome by mutableStateOf(start.takeIf { it == Screen.Welcome || it == Screen.WelcomeAccount })
    val top: Screen get() = welcome ?: stacks.getValue(tab).lastOrNull() ?: Screen.Home
    val canGoBack get() = stacks.getValue(tab).isNotEmpty() || tab != Tab.CHATS
    fun go(s: Screen) {
        when {
            s == Screen.Welcome || s == Screen.WelcomeAccount -> welcome = s
            s == Screen.Net(NetPage.PAGES) -> select(Tab.PAGES, reset = false)
            else -> stacks.getValue(tab).add(s)
        }
    }
    fun back() {
        val st = stacks.getValue(tab)
        if (st.isNotEmpty()) st.removeAt(st.lastIndex) else if (tab != Tab.CHATS) tab = Tab.CHATS
    }
    /** To a place's top (and leave the first-start screens). */
    fun home(t: Tab? = null) { welcome = null; t?.let { tab = it }; stacks.getValue(tab).clear() }
    fun select(t: Tab, reset: Boolean = true) {
        if (t == tab && reset) { stacks.getValue(t).clear(); if (t == Tab.PAGES) dk.maarse.app.Pages.home() }
        tab = t
    }
}

@Composable
fun AppShell(nav: Nav) {
    LaunchedEffect(Unit) {
        while (true) {
            Node.pollMeshcoreUnread()
            delay(5_000)
        }
    }
    BackHandler(nav.welcome == null && nav.canGoBack) { nav.back() }
    PagesNav.open = { nav.select(Tab.PAGES, reset = false) }
    PagesNav.contact = { nav.go(Screen.Contact(it)) }
    // Edge to edge: lift the whole app above the keyboard, and hide the tab bar while it is open,
    // so the text field of any screen stays visible.
    @OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
    val keyboardOpen = androidx.compose.foundation.layout.WindowInsets.Companion.isImeVisible
    Surface(Modifier.fillMaxSize().imePadding(), color = MaterialTheme.colorScheme.background) {
        val s = nav.top
        if (s == Screen.Welcome) Welcome(nav)
        else if (s == Screen.WelcomeAccount) WelcomeAccount(nav)
        else Column(Modifier.fillMaxSize()) {
            Box(Modifier.weight(1f)) {
                when (s) {
                    Screen.Home -> Home(nav)
                    is Screen.Chat -> Box(Modifier.statusBarsPadding()) {
                        ChatScreen(s.conv, onBack = nav::back, onContact = { nav.go(Screen.Contact(it)) })
                    }
                    is Screen.Contact -> Box(Modifier.statusBarsPadding()) {
                        ContactScreen(s.addr, onBack = nav::back, onChat = { c -> nav.home(Tab.CHATS); nav.go(Screen.Chat(c)) })
                    }
                    Screen.Me -> SubPage("You", nav::back) { MeScreen(nav) }
                    is Screen.Page -> SubPage(s.page.title, nav::back) { MePageContent(s.page) }
                    is Screen.Net -> SubPage(s.page.title, nav::back) { NetPageContent(s.page) }
                    is Screen.Sensor -> SubPage("Sensor", nav::back) {
                        SensorDetailScreen(s.node, onBack = nav::back, onEdit = { Sensors.openDesign(it, "blank"); nav.go(Screen.SensorDesign) })
                    }
                    Screen.SensorStart -> SubPage("Set up a sensor", nav::back) {
                        SensorStartScreen(onPick = { Sensors.openDesign(null, it); nav.go(Screen.SensorDesign) }, onImported = { nav.go(Screen.SensorDesign) })
                    }
                    Screen.SensorDesign -> SubPage("Sensor setup", nav::back) {
                        SensorDesignScreen(onRule = { nav.go(Screen.SensorRule(it)) }, onSend = { nav.go(Screen.SensorSend) })
                    }
                    is Screen.SensorRule -> SubPage("Edit a rule", nav::back) { SensorRuleScreen(s.index, onBack = nav::back) }
                    Screen.SensorSend -> SubPage("Test and send", nav::back) { SensorSendScreen() }
                    else -> {}
                }
            }
            if (!keyboardOpen) BottomBar(nav)
        }
    }
    CallOverlay()
    if (nav.sheet) ConnectionSheet(onDismiss = { nav.sheet = false })
}

/** The five places, on every screen. */
@Composable
private fun BottomBar(nav: Nav) {
    val unread by Node.unread.collectAsState()
    val meshcore by Node.meshcoreAccess.collectAsState()
    NavigationBar(containerColor = MaterialTheme.colorScheme.surfaceContainer) {
        Tab.entries.forEach { t ->
            val icon: ImageVector = when (t) {
                Tab.CHATS -> Icons.AutoMirrored.Outlined.Chat; Tab.PAGES -> Icons.Outlined.Public; Tab.CONTACTS -> Icons.Outlined.People
                Tab.NETWORK -> Icons.Outlined.Hub; Tab.SETTINGS -> Icons.Outlined.Settings
            }
            NavigationBarItem(
                selected = nav.tab == t, onClick = { nav.select(t) }, label = { Text(t.title) },
                colors = NavigationBarItemDefaults.colors(indicatorColor = MaterialTheme.colorScheme.primaryContainer),
                icon = {
                    val n = if (t == Tab.CHATS) unread.values.sum() else 0
                    if (n > 0) BadgedBox(badge = { Badge { Text("$n") } }) { Icon(icon, null) } else Icon(icon, null)
                },
            )
        }
    }
}

@Composable
private fun SubPage(title: String, onBack: () -> Unit, content: @Composable () -> Unit) {
    Column(Modifier.fillMaxSize().statusBarsPadding()) {
        Row(Modifier.fillMaxWidth().padding(4.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            Text(title, style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold)
        }
        Box(Modifier.weight(1f)) { content() }
    }
}

/** My picture: the person's initials, or "?" before a name is set. */
@Composable
private fun MeAvatar(size: androidx.compose.ui.unit.Dp, onClick: () -> Unit) {
    val me by Node.me.collectAsState()
    val name = me?.name?.takeIf { it.isNotBlank() } ?: "?"
    Box(Modifier.clip(CircleShape).clickable(onClick = onClick)) { Avatar(name, me?.addr ?: "me", size) }
}

@Composable
private fun Home(nav: Nav) {
    if (nav.tab == Tab.PAGES) { Box(Modifier.fillMaxSize().statusBarsPadding()) { PagesPage(onExit = { nav.select(Tab.CHATS) }) }; return }
    Column(Modifier.fillMaxSize().statusBarsPadding()) {
        Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 8.dp, top = 12.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            MeAvatar(40.dp) { nav.select(Tab.SETTINGS, reset = false) }
            Text(nav.tab.title, Modifier.weight(1f).padding(start = 12.dp), style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold)
        }
        ConnectionBanner { nav.sheet = true }
        AlertBar()
        Box(Modifier.weight(1f)) {
            when (nav.tab) {
                Tab.CHATS -> ChatsScreen(onOpen = { nav.go(Screen.Chat(it)) }, onAlerts = { nav.go(Screen.Net(NetPage.ALERTS)) })
                Tab.CONTACTS -> ContactsScreen(onContact = { nav.go(Screen.Contact(it)) }, onMeshcoreChat = { nav.select(Tab.CHATS, reset = false) }, onPages = { nav.select(Tab.PAGES, reset = false) }, onAnnounces = { nav.go(Screen.Net(NetPage.ANNOUNCES)) },
                    onSensor = { nav.go(Screen.Sensor(it)) }, onSensorNew = { nav.go(Screen.SensorStart) })
                Tab.NETWORK -> NetworkScreen(onPage = { nav.go(Screen.Net(it)) })
                Tab.SETTINGS -> MeScreen(nav)
                Tab.PAGES -> {}
            }
        }
    }
}

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
            .clickable(onClick = onClick).padding(14.dp),
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
private fun ModeChooser(advanced: Boolean = false, onConnected: () -> Unit) {
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
            Mode.PHONE -> Row(Modifier.fillMaxWidth().padding(4.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text("Use my radio as LoRa modem")
                    Text(radio?.let { "$it · over Bluetooth" } ?: "Choose a radio first", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                if (radio == null) TextButton(onClick = { picking = true }) { Text("Choose") }
                else Switch(modem, { modem = it })
            }
            Mode.BLE -> Row(Modifier.fillMaxWidth().padding(4.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(radio?.let { "Radio: $it" } ?: "No radio chosen yet", Modifier.weight(1f))
                TextButton(onClick = { picking = true }) { Text(if (radio == null) "Choose radio" else "Change") }
            }
            Mode.REMOTE -> Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
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
        if (conn.state != Conn.State.OFF) TextButton(onClick = { Node.stop(); onConnected() }, modifier = Modifier.align(Alignment.CenterHorizontally)) { Text("Disconnect") }
    }
    if (picking) BlePicker { addr, name ->
        picking = false
        if (addr != null) { Node.prefs.bleAddress = addr; Node.prefs.bleName = name; radio = name }
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun ConnectionSheet(onDismiss: () -> Unit) {
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), containerColor = MaterialTheme.colorScheme.surface) {
        Column(Modifier.padding(horizontal = 16.dp).padding(bottom = 24.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text("How this phone connects", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold)
            Text("You can change this any time. Your chats and your account stay.", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.height(6.dp))
            ModeChooser(advanced = true, onConnected = onDismiss)
        }
    }
}

// ---------- you ----------

@Composable
private fun MeScreen(nav: Nav) {
    val meshcore by Node.meshcoreAccess.collectAsState()
    val me by Node.me.collectAsState()
    val p by Node.person.collectAsState()
    val conn by Node.conn.collectAsState()
    val host = conn.mode == Mode.PHONE || conn.mode == Mode.REMOTE
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
        Card2 {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                MeAvatar(56.dp) { nav.go(Screen.Page(MePage.NODE)) }
                Column(Modifier.weight(1f)) {
                    Text(me?.name?.takeIf { it.isNotBlank() } ?: "No name yet", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
                    Text(
                        when {
                            p?.state == PersonState.MEMBER -> "Account ${p!!.person.take(8)} · ${p!!.devices.count { !it.second }} devices"
                            me != null -> "Address ${me!!.addr.take(8)}"
                            else -> "Not connected"
                        },
                        style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
        GroupTitle("Me")
        Group {
            if (host) GroupRow("Account and devices", when (p?.state) {
                PersonState.MEMBER -> if (p!!.master) "This is the master device · backup, add devices" else "Linked to your account"
                PersonState.AWAITING -> "Waiting to be linked"
                else -> "One address for your phone, PC and more"
            }) { nav.go(Screen.Page(MePage.ACCOUNT)) }
            // Setup only: the messages and channels are in Chats, next to the conversations.
            if (me != null) GroupRow("MeshCore setup",
                if (meshcore.key != null) "Access and identity. Messages are in Chats" else "Backbone address and access") {
                nav.go(Screen.Page(MePage.MESHCORE))
            }
            if (p?.state == PersonState.MEMBER) GroupRow("Sync now", "Fetch your mail and copies from your other devices (also done at start-up)") {
                Node.personSetup(dk.maarse.app.PersonOp.SYNC)
            }
            GroupRow("Name and position", "What your announces share", last = true) { nav.go(Screen.Page(MePage.NODE)) }
        }
        GroupTitle("Connection")
        Group {
            val hostPage = conn.mode == Mode.PHONE || conn.mode == Mode.SERVER
            GroupRow("How this phone connects", connectionSummary(), last = !hostPage) { nav.sheet = true }
            if (hostPage) GroupRow("Internet link and post office", "Backbone, peers, mail for others, keys", last = true) { nav.go(Screen.Page(MePage.HOST)) }
        }
        GroupTitle("This node")
        Group {
            GroupRow("Role and regions", roleSummary(), last = true) { nav.go(Screen.Page(MePage.RADIO)) }
        }
        GroupTitle("This phone")
        Group {
            GroupRow("Notifications and background", "Messages, channels, alerts", last = true) { nav.go(Screen.Page(MePage.APP)) }
        }
        Hint("Må:rse " + LocalContext.current.packageManager.getPackageInfo(LocalContext.current.packageName, 0).versionName)
    }
}

@Composable
private fun MePageContent(p: MePage) {
    val conn by Node.conn.collectAsState()
    val me by Node.me.collectAsState()
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
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
            MePage.HOST -> when (conn.mode) {
                Mode.PHONE -> HostSection(server = false)
                Mode.SERVER -> HostSection(server = true)
                else -> Hint("Host settings are for the phone node and server mode.")
            }
        }
    }
}

// ---------- first start ----------

@Composable
private fun Welcome(nav: Nav) {
    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().verticalScroll(rememberScrollState()).padding(24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Box(Modifier.size(64.dp).clip(RoundedCornerShape(16.dp)).background(Green), contentAlignment = Alignment.Center) {
            Icon(painterResource(R.drawable.ic_stat), null, Modifier.size(40.dp), tint = androidx.compose.ui.graphics.Color.White)
        }
        Text("Messages that get through — with or without the internet", style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold)
        Text("Step 1 of 2 · How will you use Må:rse? You can change it later by tapping the line at the top of Chats.",
            style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        ModeChooser(onConnected = {
            Node.prefs.onboarded = true
            nav.home()
            if (Node.prefs.mode == Mode.PHONE || Node.prefs.mode == Mode.REMOTE) nav.go(Screen.WelcomeAccount)
        })
        TextButton(onClick = { Node.prefs.onboarded = true; nav.home() }, modifier = Modifier.align(Alignment.CenterHorizontally)) { Text("Skip for now") }
    }
}

@Composable
private fun WelcomeAccount(nav: Nav) {
    val me by Node.me.collectAsState()
    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().verticalScroll(rememberScrollState()).padding(vertical = 24.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text("Step 2 of 2 · Your account", Modifier.padding(horizontal = 24.dp), style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold)
        Text("Create your account, or link this phone to the one on your computer.",
            Modifier.padding(horizontal = 24.dp), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (me == null) Hint("Starting the node…") else {
            var name by remember { mutableStateOf(me?.name ?: "") }
            GroupTitle("Your name")
            Card2 {
                Text("Shown to others next to your address.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Row(verticalAlignment = Alignment.CenterVertically) {
                    OutlinedTextField(name, { name = it.take(24) }, Modifier.weight(1f), singleLine = true, label = { Text("Name") })
                    TextButton(onClick = { Node.send(dk.maarse.app.Cmd.setName(name.trim())); Node.send(dk.maarse.app.Cmd.getName()) }) { Text("Save") }
                }
            }
            AccountSection()
        }
        val account by Node.person.collectAsState()
        OutlinedButton(onClick = { nav.home() }, modifier = Modifier.align(Alignment.CenterHorizontally)) {
            Text(if (account?.state == PersonState.NONE) "Later" else "Done")
        }
        if (account?.state == PersonState.NONE) {
            Text("You can do this later under Settings → Account and devices. MeshCore needs an account.",
                Modifier.padding(horizontal = 24.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

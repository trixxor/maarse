package dk.maarse.app.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.outlined.Chat
import androidx.compose.material.icons.outlined.Hub
import androidx.compose.material.icons.outlined.Map
import androidx.compose.material.icons.outlined.People
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dk.maarse.app.Node
import dk.maarse.app.Sensors
import kotlinx.coroutines.delay

/** The four places in the bottom bar. Pages is a part of Nodes; Settings opens from the avatar. */
enum class Tab(val title: String) { CHATS("Chats"), MAP("Map"), CONTACTS("Nodes"), NETWORK("Mesh") }

/** Pages behind "Me" (Settings). */
enum class MePage(val title: String) {
    ACCOUNT("Account and devices"), MESHCORE("MeshCore setup"), NODE("Name and position"), RADIO("Role and regions"), APP("Notifications and sounds"),
    HOST("Internet link and post office"), FIRMWARE("Firmware"),
}

sealed interface Screen {
    data object Home : Screen
    data class Chat(val conv: String) : Screen
    data class Contact(val addr: String) : Screen
    /** New chat: join a channel, add by QR code or link, or pick one of your people. */
    data object NewChat : Screen
    /** Settings, opened from the avatar at the top right of every place. */
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

/**
 * Navigation: four places in a bar (Chats, Map, Nodes, Mesh). Each place has a small stack of screens
 * (a chat, a contact, a sub-page, Settings). Sub-pages, chats and Settings have a back arrow and no bar.
 * Back: one step back; with nothing left to go back to it goes to Chats, and from Chats it leaves the app.
 * A tap on a place always shows its top; only an open chat is remembered (it is a screen of its own,
 * so Back from Settings returns to it).
 */
class Nav(start: Screen) {
    private val stacks = Tab.entries.associateWith { mutableStateListOf<Screen>() }
    var tab by mutableStateOf(Tab.CHATS)
    var sheet by mutableStateOf(false)
    /** The first-start screens (no bar), until they are done. */
    var welcome by mutableStateOf(start.takeIf { it == Screen.Welcome || it == Screen.WelcomeAccount })
    val top: Screen get() = welcome ?: stacks.getValue(tab).lastOrNull() ?: Screen.Home
    /** A MeshCore conversation is open on the Chats place: it is a screen without the bar. */
    val mcChatOpen: Boolean get() = welcome == null && tab == Tab.CHATS && stacks.getValue(tab).isEmpty() && McNav.open != null
    val canGoBack get() = mcChatOpen || stacks.getValue(tab).isNotEmpty() || tab != Tab.CHATS
    fun go(s: Screen) {
        when {
            s == Screen.Welcome || s == Screen.WelcomeAccount -> welcome = s
            // Pages is a part of Nodes now.
            s == Screen.Net(NetPage.PAGES) -> { select(Tab.CONTACTS, reset = false); NodesNav.showPages() }
            else -> stacks.getValue(tab).add(s)
        }
    }
    fun back() {
        val st = stacks.getValue(tab)
        if (st.isNotEmpty()) st.removeAt(st.lastIndex)
        else if (mcChatOpen) { McNav.open = null; Node.mcAsk("overview") }
        else if (tab != Tab.CHATS) tab = Tab.CHATS
    }
    /** To a place's top (and leave the first-start screens). */
    fun home(t: Tab? = null) { welcome = null; t?.let { tab = it }; stacks.getValue(tab).clear() }
    fun select(t: Tab, reset: Boolean = true) {
        if (reset) {
            stacks.getValue(t).clear()
            if (t == Tab.CONTACTS) McNav.node = null
        }
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
    PagesNav.open = { nav.select(Tab.CONTACTS, reset = false); NodesNav.showPages() }
    PagesNav.contact = { nav.go(Screen.Contact(it)) }
    // Edge to edge: lift the whole app above the keyboard, and hide the tab bar while it is open,
    // so the text field of any screen stays visible.
    @OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
    val keyboardOpen = androidx.compose.foundation.layout.WindowInsets.Companion.isImeVisible
    Surface(Modifier.fillMaxSize().imePadding(), color = MaterialTheme.colorScheme.background) {
        val s = nav.top
        if (s == Screen.Welcome) Welcome(nav)
        else if (s == Screen.WelcomeAccount) WelcomeAccount(nav)
        else {
            val mcChat = nav.mcChatOpen
            // A MeshCore node page (and the repeater screens) has its own back arrow: no place title, no bar.
            val mcNode = s == Screen.Home && nav.tab == Tab.CONTACTS && McNav.node != null
            val onHome = s == Screen.Home && !mcChat && !mcNode
            Column(Modifier.fillMaxSize()) {
                // Screens without the bar keep clear of the system navigation bar themselves.
                Box(Modifier.weight(1f).then(if (onHome) Modifier else Modifier.navigationBarsPadding())) {
                    if (mcChat) Box(Modifier.statusBarsPadding()) { McChatScreen(McNav.open!!, onBack = nav::back) }
                    else when (s) {
                        Screen.Home -> Home(nav)
                        is Screen.Chat -> Box(Modifier.statusBarsPadding()) {
                            ChatScreen(s.conv, onBack = nav::back, onContact = { nav.go(Screen.Contact(it)) })
                        }
                        is Screen.Contact -> Box(Modifier.statusBarsPadding()) {
                            ContactPage(s.addr, onBack = nav::back, onChat = { c -> nav.home(Tab.CHATS); nav.go(Screen.Chat(c)) })
                        }
                        Screen.NewChat -> SubPage("New chat", nav::back) {
                            NewChatScreen(onOpen = { c -> nav.back(); nav.go(Screen.Chat(c)) }, onDone = { nav.back() })
                        }
                        Screen.Me -> SubPage("Settings", nav::back) { MeScreen(nav) }
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
                if (onHome && !keyboardOpen) BottomBar(nav)
            }
        }
    }
    CallOverlay()
    if (nav.sheet) ConnectionSheet(onDismiss = { nav.sheet = false })
}

/** The four places, on every top screen (not on sub-pages, chats or Settings). */
@Composable
private fun BottomBar(nav: Nav) {
    val unread by Node.unread.collectAsState()
    val mc by Node.meshcoreAccess.collectAsState()
    NavigationBar(containerColor = MaterialTheme.colorScheme.surfaceContainer) {
        Tab.entries.forEach { t ->
            val icon: ImageVector = when (t) {
                Tab.CHATS -> Icons.AutoMirrored.Outlined.Chat; Tab.MAP -> Icons.Outlined.Map
                Tab.CONTACTS -> Icons.Outlined.People; Tab.NETWORK -> Icons.Outlined.Hub
            }
            NavigationBarItem(
                selected = nav.tab == t, onClick = { nav.select(t) },
                // Short labels on one line: a label must never wrap into two lines.
                label = { Text(t.title, maxLines = 1, softWrap = false, overflow = TextOverflow.Clip) },
                colors = NavigationBarItemDefaults.colors(indicatorColor = MaterialTheme.colorScheme.primaryContainer),
                icon = {
                    // Må:rse unread in the primary colour; MeshCore unread as its own small violet badge.
                    val n = if (t == Tab.CHATS) unread.values.sum() else 0
                    val m = if (t == Tab.CHATS) mc.totalUnread else 0
                    if (n > 0 || m > 0) BadgedBox(badge = {
                        Row(horizontalArrangement = Arrangement.spacedBy(2.dp)) {
                            if (n > 0) Badge(Modifier.border(1.5.dp, MaterialTheme.colorScheme.surfaceContainer, CircleShape), containerColor = MaterialTheme.colorScheme.primary, contentColor = MaterialTheme.colorScheme.onPrimary) { Text("$n") }
                            if (m > 0) Badge(Modifier.border(1.5.dp, MaterialTheme.colorScheme.surfaceContainer, CircleShape), containerColor = Mc.color, contentColor = Mc.onColor) { Text("$m") }
                        }
                    }) { Icon(icon, null) } else Icon(icon, null)
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
            Text(title, Modifier.weight(1f), style = if (title.length > 22) MaterialTheme.typography.titleMedium else MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold, maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
        Box(Modifier.weight(1f)) { content() }
    }
}

/** My picture: the person's initials, or "?" before a name is set. */
@Composable
fun MeAvatar(size: androidx.compose.ui.unit.Dp, onClick: () -> Unit) {
    val me by Node.me.collectAsState()
    val name = me?.name?.takeIf { it.isNotBlank() } ?: "?"
    Box(Modifier.clip(CircleShape).clickable(onClick = onClick)) { Avatar(name, me?.addr ?: "me", size) }
}

@Composable
private fun Home(nav: Nav) {
    if (nav.tab == Tab.CONTACTS && McNav.node != null) {
        Box(Modifier.fillMaxSize().statusBarsPadding()) {
            ContactsScreen(onContact = { nav.go(Screen.Contact(it)) }, onMeshcoreChat = { nav.select(Tab.CHATS, reset = false) })
        }
        return
    }
    Column(Modifier.fillMaxSize().statusBarsPadding()) {
        // The title of the place; the avatar opens Settings (and Back returns here).
        Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 8.dp, top = 8.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(nav.tab.title, Modifier.weight(1f), style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold, maxLines = 1)
            Box(Modifier.size(48.dp), contentAlignment = Alignment.Center) { MeAvatar(40.dp) { nav.go(Screen.Me) } }
        }
        ConnectionBanner { nav.sheet = true }
        Box(Modifier.weight(1f)) {
            when (nav.tab) {
                Tab.CHATS -> ChatsScreen(onOpen = { nav.go(Screen.Chat(it)) }, onAlerts = { nav.go(Screen.Net(NetPage.ALERTS)) },
                    onNew = { nav.go(Screen.NewChat) }, onSensor = { nav.go(Screen.Sensor(it)) })
                Tab.MAP -> MapScreen(onNode = { nav.go(Screen.Contact(it)) }, onSensor = { nav.go(Screen.Sensor(it)) }, onMessage = { nav.go(Screen.Chat(it)) })
                Tab.CONTACTS -> ContactsScreen(onContact = { nav.go(Screen.Contact(it)) }, onMeshcoreChat = { nav.select(Tab.CHATS, reset = false) }, onPages = {},
                    onAnnounces = { nav.go(Screen.Net(NetPage.ANNOUNCES)) }, onSensor = { nav.go(Screen.Sensor(it)) }, onSensorNew = { nav.go(Screen.SensorStart) })
                Tab.NETWORK -> NetworkScreen(onPage = { nav.go(Screen.Net(it)) }, onConnect = { nav.sheet = true })
            }
        }
    }
}

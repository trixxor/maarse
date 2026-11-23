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
    HOST("Internet link and post office"), FIRMWARE("Firmware"),
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
fun MeAvatar(size: androidx.compose.ui.unit.Dp, onClick: () -> Unit) {
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

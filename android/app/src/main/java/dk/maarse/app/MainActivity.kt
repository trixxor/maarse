package dk.maarse.app

import android.content.Intent
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Chat
import androidx.compose.material.icons.filled.Hub
import androidx.compose.material.icons.filled.People
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.Icon
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import dk.maarse.app.ui.AlertBar
import dk.maarse.app.ui.ChatScreen
import dk.maarse.app.ui.ChatsScreen
import dk.maarse.app.ui.MaarseTheme
import dk.maarse.app.ui.NetworkScreen
import dk.maarse.app.ui.NodesScreen
import dk.maarse.app.ui.SettingsScreen
import dk.maarse.app.ui.StatusStrip

enum class Tab(val title: String) { CHATS("Chats"), NODES("Nodes"), NETWORK("Network"), SETTINGS("Settings") }

class MainActivity : ComponentActivity() {
    private var tab by mutableStateOf(Tab.CHATS)
    private var conv by mutableStateOf<String?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Node.init(this)
        enableEdgeToEdge()
        if (Build.VERSION.SDK_INT >= 33) requestPermissions(arrayOf(android.Manifest.permission.POST_NOTIFICATIONS), 1)
        // First start: nothing chosen yet, so begin in Settings.
        if (!Node.prefs.running && Node.prefs.bleAddress == null) tab = Tab.SETTINGS
        handle(intent)
        setContent { MaarseTheme { App() } }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    private fun handle(i: Intent?) {
        when (val c = i?.getStringExtra("conv")) {
            null -> {}
            "alerts" -> { tab = Tab.NETWORK; conv = null }
            else -> { tab = Tab.CHATS; conv = c }
        }
    }

    @Composable
    private fun App() {
        val unread by Node.unread.collectAsState()
        BackHandler(conv != null) { conv = null }
        Scaffold(
            topBar = { Column(Modifier.statusBarsPadding()) { StatusStrip(); AlertBar() } },
            bottomBar = {
                if (conv == null) NavigationBar {
                    Tab.entries.forEach { t ->
                        NavigationBarItem(
                            selected = tab == t, onClick = { tab = t }, label = { Text(t.title) },
                            icon = {
                                val icon = when (t) { Tab.CHATS -> Icons.Default.Chat; Tab.NODES -> Icons.Default.People; Tab.NETWORK -> Icons.Default.Hub; Tab.SETTINGS -> Icons.Default.Settings }
                                val n = if (t == Tab.CHATS) unread.values.sum() else 0
                                if (n > 0) BadgedBox(badge = { Badge { Text("$n") } }) { Icon(icon, null) } else Icon(icon, null)
                            },
                        )
                    }
                }
            },
        ) { pad ->
            Column(Modifier.fillMaxSize().padding(pad)) {
                val c = conv
                if (c != null) ChatScreen(c, onBack = { conv = null })
                else when (tab) {
                    Tab.CHATS -> ChatsScreen(onOpen = { conv = it })
                    Tab.NODES -> NodesScreen(onChat = { conv = it; tab = Tab.CHATS })
                    Tab.NETWORK -> NetworkScreen()
                    Tab.SETTINGS -> SettingsScreen()
                }
            }
        }
    }
}

package dk.maarse.app

import android.content.Intent
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import dk.maarse.app.ui.AppShell
import dk.maarse.app.ui.MaarseTheme
import dk.maarse.app.ui.NetPage
import dk.maarse.app.ui.Nav
import dk.maarse.app.ui.Screen
import dk.maarse.app.ui.Tab

class MainActivity : ComponentActivity() {
    private lateinit var nav: Nav

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Node.init(this)
        // Light or dark follows the system (MaarseTheme), edge to edge.
        enableEdgeToEdge()
        if (Build.VERSION.SDK_INT >= 33) requestPermissions(arrayOf(android.Manifest.permission.POST_NOTIFICATIONS), 1)
        // First start: the guide (how to connect, then the account).
        val first = !Node.prefs.onboarded && !Node.prefs.running
        nav = Nav(if (first) Screen.Welcome else Screen.Home)
        handle(intent)
        setContent { MaarseTheme { AppShell(nav) } }
    }

    override fun onStart() {
        super.onStart()
        Node.resume()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    /** A tap on a notification: open that conversation, or the alerts. */
    private fun handle(i: Intent?) {
        when (val c = i?.getStringExtra("conv")) {
            null -> {}
            "alerts" -> { nav.home(Tab.NETWORK); nav.go(Screen.Net(NetPage.ALERTS)) }
            else -> if (c.startsWith("mc:")) {
                // A MeshCore mention: open that MeshCore conversation.
                val (kind, id) = c.removePrefix("mc:").split(":", limit = 2).let { it[0] to it.getOrElse(1) { "" } }
                val row = Node.meshcoreAccess.value.conversations.firstOrNull { it.kind == kind && it.id == id }
                nav.home(Tab.CHATS)
                dk.maarse.app.ui.MeshCoreMode.on = true
                dk.maarse.app.ui.McNav.open = row ?: Node.McConversation(kind, id, if (kind == "public") "Public" else if (kind == "channel") "#$id" else id.take(8) + "…", "", false, 0, 0)
                Notify.cancel(this, c)
            } else { nav.home(Tab.CHATS); nav.go(Screen.Chat(c)) }
        }
    }
}

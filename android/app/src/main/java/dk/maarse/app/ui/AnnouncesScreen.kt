package dk.maarse.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Tab
import androidx.compose.material3.TabRow
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dk.maarse.app.Node
import dk.maarse.app.Pages

/** Who announces themselves, live: Users (every announce, newest first) and Pages (nodes sharing pages). */
@Composable
fun AnnouncesPage() {
    val list by Node.announces.collectAsState()
    val nodes by Node.nodes.collectAsState()
    val v = Pages.version.collectAsState().value
    var tab by remember { mutableIntStateOf(0) }
    Column(Modifier.fillMaxSize()) {
        TabRow(selectedTabIndex = tab) {
            Tab(tab == 0, { tab = 0 }, text = { Text("Users") })
            Tab(tab == 1, { tab = 1 }, text = { Text("Pages") })
        }
        val now = System.currentTimeMillis()
        if (tab == 0) {
            if (list.isEmpty()) Hint("No announces yet. They come in while you are connected; nodes announce themselves every few minutes.")
            LazyColumn {
                items(list.take(100)) { (addr, hops, at) ->
                    val n = nodes[addr]
                    val tags = buildList {
                        add(if (hops == 0) "direct" else "$hops hop${if (hops == 1) "" else "s"}")
                        if (n?.lat != null) add("shares position")
                        if (addr in Pages.sharing) add("has pages")
                    }
                    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        Avatar(Node.label(addr), addr)
                        Column(Modifier.weight(1f)) {
                            Text(Node.label(addr), fontWeight = FontWeight.SemiBold)
                            Text(tags.joinToString(" · "), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        Text(ago(now - at), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            }
        } else {
            val last = remember(list) { list.groupBy { it.first }.mapValues { e -> e.value.maxOf { it.third } } }
            val sharing = remember(v, list) { Pages.sharing.sortedByDescending { last[it] ?: 0L } }
            if (sharing.isEmpty()) Hint("No node sharing pages heard yet. A host with pages/index.mu says so in every announce.")
            LazyColumn {
                items(sharing) { addr ->
                    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        Avatar(Node.label(addr), addr)
                        Column(Modifier.weight(1f)) {
                            Text(Node.label(addr), fontWeight = FontWeight.SemiBold)
                            Text(last[addr]?.let { "announced " + ago(now - it) } ?: "visited earlier",
                                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        Button(onClick = { Pages.open("maarse://$addr/") ; PagesNav.open() }) { Text("Open") }
                    }
                }
            }
        }
    }
}

private fun ago(ms: Long): String {
    val s = ms / 1000
    return when {
        s < 60 -> "${s}s ago"
        s < 3600 -> "${s / 60} min ago"
        s < 86400 -> "${s / 3600} h ago"
        else -> "${s / 86400} d ago"
    }
}

/** Lets the Announces page jump to the Pages screen (set by the shell). */
object PagesNav {
    var open: () -> Unit = {}
}

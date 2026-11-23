package dk.maarse.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Tab
import androidx.compose.material3.TabRow
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dk.maarse.app.Node
import dk.maarse.app.Pages

/**
 * Who announces themselves, live. Users: favourites first, then every
 * announce, newest first. Pages: nodes sharing pages with their front page's
 * title, favourites first. Both can be searched; a star makes a favourite.
 */
@Composable
fun AnnouncesPage(onUser: (String) -> Unit = { PagesNav.contact(it) }) {
    val list by Node.announces.collectAsState()
    val nodes by Node.nodes.collectAsState()
    val v = Pages.version.collectAsState().value
    var tab by remember { mutableIntStateOf(0) }
    var query by remember { mutableStateOf("") }
    val q = query.trim().lowercase()
    fun match(a: String, extra: String = "") = q.isEmpty() || Node.label(a).lowercase().contains(q) || a.startsWith(q) || extra.lowercase().contains(q)
    Column(Modifier.fillMaxSize()) {
        OutlinedTextField(
            query, { query = it }, Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), singleLine = true,
            placeholder = { Text("Search nodes and repeaters by name or address") },
            leadingIcon = { Icon(Icons.Default.Search, null) }, shape = RoundedCornerShape(24.dp),
        )
        val now = System.currentTimeMillis()
        val last = remember(list) { list.groupBy { it.first }.mapValues { e -> e.value.maxOf { it.third } } }
        if (tab == 0) {
            val favs = remember(v, q) { Pages.favUsers.filter { match(it) } }
            val live = list.filter { match(it.first) }.take(100)
            if (favs.isEmpty() && live.isEmpty()) Hint(if (q.isNotEmpty()) "Nobody matches." else "No announces yet. They come in while you are connected; nodes announce themselves every few minutes.")
            LazyColumn {
                if (favs.isNotEmpty()) {
                    item { GroupTitle("Favourites") }
                    items(favs, key = { "f$it" }) { a ->
                        Line(a, last[a]?.let { "announced " + ago(now - it) } ?: "not heard this session", 'u') { onUser(a) }
                    }
                    item { GroupTitle("Latest announces") }
                }
                items(live) { (addr, hops, at) ->
                    val n = nodes[addr]
                    val tags = buildList {
                        add(if (hops == 0) "direct" else "$hops hop${if (hops == 1) "" else "s"}")
                        if (n?.lat != null) add("shares position")
                        if (addr in Pages.sharing) add("has pages")
                        add(ago(now - at))
                    }
                    Line(addr, tags.joinToString(" · "), 'u') { onUser(addr) }
                }
            }
        } else {
            val sites = remember(v, list, q) {
                (Pages.favPages + Pages.sharing).distinct().filter { match(it, Pages.siteTitle(it)) }
                    .sortedWith(compareByDescending<String> { Pages.isFav('p', it) }.thenByDescending { last[it] ?: 0L })
            }
            if (sites.isEmpty()) Hint(if (q.isNotEmpty()) "No page matches." else "No node sharing pages heard yet. A host with pages/index.mu says so in every announce.")
            LazyColumn {
                items(sites, key = { it }) { addr ->
                    val t = Pages.siteTitle(addr)
                    val when_ = last[addr]?.let { "announced " + ago(now - it) } ?: "visited earlier"
                    Line(addr, if (t.isEmpty()) when_ else Node.label(addr) + " · " + when_, 'p', title = t.ifEmpty { null },
                        action = { Button(onClick = { Pages.open("maarse://$addr/"); PagesNav.open() }, Modifier.heightIn(min = 48.dp)) { Text("Open") } }) {
                        Pages.open("maarse://$addr/"); PagesNav.open()
                    }
                }
            }
        }
    }
}

@Composable
private fun Line(addr: String, sub: String, kind: Char, title: String? = null, action: (@Composable () -> Unit)? = null, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().clickable(onClick = onClick).padding(start = 4.dp, end = 16.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        FavStar(kind, addr)
        Avatar(Node.label(addr), addr)
        Column(Modifier.weight(1f)) {
            Text(title ?: friendlyName(Node.label(addr)), fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis)
            Text(sub, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        action?.invoke()
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

/** Lets the Announces page jump to the Pages screen and to a contact (set by the shell). */
object PagesNav {
    var open: () -> Unit = {}
    var contact: (String) -> Unit = {}
}

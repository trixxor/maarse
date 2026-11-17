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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.Chat
import androidx.compose.material.icons.filled.Call
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Route
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dk.maarse.app.Node
import dk.maarse.app.NodeEntry
import dk.maarse.app.shortAddr

private fun reach(n: NodeEntry): String = buildList {
    n.hops?.let { add(if (it == 0) "nearby" else "$it hops") }
    if (n.lastHeard > 0) add("heard " + time(n.lastHeard))
    n.mv?.let { add("%.1f V".format(it / 1000.0)) }
}.joinToString(" · ")

/** People and nodes heard, newest first. */
@Composable
fun ContactsScreen(onContact: (String) -> Unit) {
    val nodes by Node.nodes.collectAsState()
    Node.nicknames.collectAsState().value
    var filter by remember { mutableStateOf("") }
    val list = nodes.values
        .filter { filter.isBlank() || Node.label(it.addr).contains(filter, true) || it.addr.startsWith(filter.lowercase()) }
        .sortedWith(compareByDescending<NodeEntry> { Node.nicknames.value.containsKey(it.addr) }.thenByDescending { it.lastHeard })
    Column(Modifier.fillMaxSize()) {
        OutlinedTextField(
            filter, { filter = it }, Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp), singleLine = true,
            placeholder = { Text("Search by name or address") }, leadingIcon = { Icon(Icons.Default.Search, null) },
            shape = RoundedCornerShape(24.dp),
        )
        if (list.isEmpty()) Text("Nobody heard yet. People show up when they announce themselves or write to you.",
            Modifier.padding(24.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
        LazyColumn {
            items(list, key = { it.addr }) { n ->
                val title = Node.label(n.addr)
                Row(
                    Modifier.fillMaxWidth().clickable { onContact(n.addr) }.padding(horizontal = 16.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Avatar(title, n.addr, 44.dp)
                    Column(Modifier.weight(1f)) {
                        Text(title, fontWeight = FontWeight.SemiBold)
                        Text(reach(n), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            }
        }
    }
}

/** One contact: name, how it is reached, actions and its address. */
@Composable
fun ContactScreen(addr: String, onBack: () -> Unit, onChat: (String) -> Unit) {
    val nodes by Node.nodes.collectAsState()
    Node.nicknames.collectAsState().value
    val traces by Node.traces.collectAsState()
    var naming by remember { mutableStateOf(false) }
    val n = nodes[addr]
    val title = Node.label(addr)
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState())) {
        IconButton(onClick = onBack, modifier = Modifier.padding(4.dp)) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
        Column(Modifier.fillMaxWidth().padding(bottom = 16.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Avatar(title, addr, 88.dp)
            Text(title, style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold)
            n?.name?.let { Text("Announces as ~$it", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
            Text(n?.let { reach(it) }?.ifBlank { null } ?: "Not heard yet",
                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.primary)
        }
        // Icon above a short label, like other messengers: never cut off.
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Action(Icons.AutoMirrored.Filled.Chat, "Message", Modifier.weight(1f)) { onChat("dm:$addr") }
            if (dk.maarse.app.Calls.possible()) Action(Icons.Default.Call, "Call", Modifier.weight(1f)) { dk.maarse.app.Calls.start(addr) }
            else Action(Icons.Default.Route, "Route", Modifier.weight(1f)) { Node.trace(addr) }
            Action(Icons.Default.Edit, "Name", Modifier.weight(1f)) { naming = true }
        }
        traces.firstOrNull { it.dest == addr }?.let { t ->
            GroupTitle("Route")
            Card2 {
                Text("${t.path.size} hops", fontWeight = FontWeight.SemiBold)
                Text("Signal out: ${t.out.joinToString(" · ") { "$it dB" }}", style = MaterialTheme.typography.bodySmall)
                Text("Signal back: ${t.back.joinToString(" · ") { "$it dB" }}", style = MaterialTheme.typography.bodySmall)
            }
        }
        n?.let {
            if (it.lat != null && it.lon != null) {
                GroupTitle("Position (shared by them)")
                Card2 { Text("%.4f, %.4f".format(it.lat, it.lon)) }
            }
        }
        GroupTitle("Address")
        Card2 {
            SelectionContainer { Text(addr, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall) }
            Text("Messages to this address are end-to-end encrypted. Short form: ${shortAddr(addr)}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
    if (naming) NicknameDialog(addr) { naming = false }
}

@Composable
private fun Action(icon: androidx.compose.ui.graphics.vector.ImageVector, label: String, modifier: Modifier, onClick: () -> Unit) {
    androidx.compose.material3.Surface(onClick = onClick, modifier = modifier, shape = RoundedCornerShape(16.dp),
        color = MaterialTheme.colorScheme.secondaryContainer) {
        Column(Modifier.padding(vertical = 12.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            Icon(icon, null)
            Text(label, style = MaterialTheme.typography.labelLarge, maxLines = 1, modifier = Modifier.padding(top = 4.dp))
        }
    }
}

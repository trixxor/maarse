package dk.maarse.app.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dk.maarse.app.Node
import dk.maarse.app.shortAddr

@Composable
fun NodesScreen(onChat: (String) -> Unit) {
    val nodes by Node.nodes.collectAsState()
    Node.nicknames.collectAsState().value
    val traces by Node.traces.collectAsState()
    var filter by remember { mutableStateOf("") }
    var open by remember { mutableStateOf<String?>(null) }
    val list = nodes.values
        .filter { filter.isBlank() || Node.label(it.addr).contains(filter, true) || it.addr.startsWith(filter.lowercase()) }
        .sortedByDescending { it.lastHeard }
    Column(Modifier.fillMaxSize()) {
        OutlinedTextField(filter, { filter = it }, Modifier.padding(8.dp), singleLine = true, label = { Text("Search by name or address") })
        if (list.isEmpty()) Hint("No nodes heard yet. Nodes appear when they announce themselves or write to you.")
        LazyColumn {
            items(list, key = { it.addr }) { n ->
                val details = buildList {
                    n.hops?.let { add(if (it == 0) "direct" else "$it hops") }
                    if (n.lastHeard > 0) add("heard " + time(n.lastHeard))
                    n.mv?.let { add("%.2f V".format(it / 1000.0)) }
                    if (n.usb == true) add("on USB power")
                    if (n.lat != null && n.lon != null) add("%.3f, %.3f".format(n.lat, n.lon))
                }
                ListItem(
                    headlineContent = { Text(Node.label(n.addr)) },
                    supportingContent = { Text(shortAddr(n.addr) + " · " + details.joinToString(" · ")) },
                    trailingContent = { TextButton(onClick = { open = if (open == n.addr) null else n.addr }) { Text(if (open == n.addr) "Less" else "More") } },
                )
                if (open == n.addr) Column(Modifier.padding(horizontal = 16.dp)) {
                    Text(n.addr, style = MaterialTheme.typography.bodySmall)
                    Row {
                        TextButton(onClick = { onChat("dm:${n.addr}") }) { Text("Write") }
                        TextButton(onClick = { Node.trace(n.addr) }) { Text("Trace route") }
                    }
                    traces.firstOrNull { it.dest == n.addr }?.let { t ->
                        Text("Route: ${t.path.size} hops · SNR out ${t.out.joinToString()} dB · back ${t.back.joinToString()} dB", style = MaterialTheme.typography.bodySmall)
                    }
                }
                HorizontalDivider()
            }
        }
    }
}

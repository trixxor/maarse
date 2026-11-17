package dk.maarse.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.Add
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Badge
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dk.maarse.app.Conn
import dk.maarse.app.Node
import dk.maarse.app.shortAddr

@Composable
fun ChatsScreen(onOpen: (String) -> Unit) {
    val version by Node.msgVersion.collectAsState()
    val channels by Node.channels.collectAsState()
    val unread by Node.unread.collectAsState()
    Node.nicknames.collectAsState().value
    val last = remember(version) { Node.db.lastPerConv() }
    // Joined channels show even without messages.
    val convs = last.map { it.conv } + channels.keys.map { "ch:$it" }.filter { c -> last.none { it.conv == c } }
    var adding by remember { mutableStateOf(false) }
    Box(Modifier.fillMaxSize()) {
        if (convs.isEmpty()) Column(Modifier.padding(24.dp)) {
            Text("No conversations yet", style = MaterialTheme.typography.titleMedium)
            Hint("Connect in Settings. Channels you join and people who write to you appear here. Start a chat from Nodes or with +.")
        }
        LazyColumn(Modifier.fillMaxSize()) {
            items(convs, key = { it }) { c ->
                val m = last.firstOrNull { it.conv == c }
                ListItem(
                    headlineContent = { Text(Node.convTitle(c), maxLines = 1) },
                    supportingContent = { Text(m?.let { (if (it.mine) "You: " else "") + it.text } ?: "No messages yet", maxLines = 1) },
                    trailingContent = {
                        Column(horizontalAlignment = Alignment.End) {
                            m?.let { Text(time(it.ts), style = MaterialTheme.typography.labelSmall) }
                            unread[c]?.takeIf { it > 0 }?.let { Badge { Text("$it") } }
                        }
                    },
                    modifier = Modifier.clickable { onOpen(c) },
                )
                HorizontalDivider()
            }
        }
        FloatingActionButton(onClick = { adding = true }, modifier = Modifier.align(Alignment.BottomEnd).padding(16.dp)) {
            Icon(Icons.Default.Add, "New chat or channel")
        }
    }
    if (adding) NewChatDialog(onDone = { adding = false; it?.let(onOpen) })
}

@Composable
private fun NewChatDialog(onDone: (String?) -> Unit) {
    var addr by remember { mutableStateOf("") }
    var channel by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = { onDone(null) },
        title = { Text("New conversation") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(addr, { addr = it.trim().lowercase() }, label = { Text("Address (32 hex characters)") }, singleLine = true)
                Text("or join a channel", style = MaterialTheme.typography.labelMedium)
                OutlinedTextField(channel, { channel = it }, label = { Text("Channel name, e.g. public") }, singleLine = true)
            }
        },
        confirmButton = {
            TextButton(onClick = {
                when {
                    addr.length == 32 && addr.all { it in "0123456789abcdef" } -> onDone("dm:$addr")
                    channel.isNotBlank() -> { Node.joinChannel(channel); onDone(null) }
                    else -> {}
                }
            }) { Text("Open") }
        },
        dismissButton = { TextButton(onClick = { onDone(null) }) { Text("Cancel") } },
    )
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ChatScreen(conv: String, onBack: () -> Unit) {
    val version by Node.msgVersion.collectAsState()
    val conn by Node.conn.collectAsState()
    val me by Node.me.collectAsState()
    Node.nicknames.collectAsState().value
    val msgs = remember(version, conv) { Node.db.messages(conv) }
    var text by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var naming by remember { mutableStateOf(false) }
    val list = rememberLazyListState()
    DisposableEffect(conv) {
        Node.openConv = conv
        onDispose { if (Node.openConv == conv) Node.openConv = null }
    }
    LaunchedEffect(msgs.size) {
        Node.markRead(conv)
        if (msgs.isNotEmpty()) list.scrollToItem(msgs.size - 1)
    }
    val isDm = conv.startsWith("dm:")
    val addr = conv.substringAfter(":")
    Column(Modifier.fillMaxSize().imePadding()) {
        TopAppBar(
            title = {
                Column {
                    Text(Node.convTitle(conv), maxLines = 1)
                    Text(if (isDm) shortAddr(addr) + routeText(addr) else "Channel", style = MaterialTheme.typography.labelSmall)
                }
            },
            navigationIcon = { IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") } },
            actions = { if (isDm) TextButton(onClick = { naming = true }) { Text("Name") } },
        )
        LazyColumn(Modifier.weight(1f).fillMaxWidth(), state = list, contentPadding = androidx.compose.foundation.layout.PaddingValues(8.dp)) {
            items(msgs, key = { it.id }) { m -> Bubble(m, isDm) }
        }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(horizontal = 12.dp)) }
        val max = Node.maxText(conv)
        val used = text.toByteArray().size
        Row(Modifier.fillMaxWidth().padding(8.dp), verticalAlignment = Alignment.CenterVertically) {
            OutlinedTextField(
                text, { text = it; error = null }, Modifier.weight(1f),
                placeholder = { Text(if (conn.state == Conn.State.ONLINE) "Write a message" else "Not connected") },
                supportingText = { Text("$used / $max bytes", color = if (used > max) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant) },
                enabled = me != null,
            )
            IconButton(onClick = {
                error = Node.sendText(conv, text)
                if (error == null) text = ""
            }, enabled = me != null && text.isNotBlank() && used <= max) { Icon(Icons.AutoMirrored.Filled.Send, "Send") }
        }
    }
    if (naming) {
        var nick by remember { mutableStateOf(Node.nicknames.value[addr] ?: "") }
        AlertDialog(
            onDismissRequest = { naming = false },
            title = { Text("Your name for ${shortAddr(addr)}") },
            text = { OutlinedTextField(nick, { nick = it }, singleLine = true, label = { Text("Nickname (only on this phone)") }) },
            confirmButton = { TextButton(onClick = { Node.setNickname(addr, nick); naming = false }) { Text("Save") } },
            dismissButton = { TextButton(onClick = { naming = false }) { Text("Cancel") } },
        )
    }
}

private fun routeText(addr: String): String {
    val n = Node.nodes.value[addr] ?: return ""
    return n.hops?.let { " · $it hops" } ?: ""
}

@Composable
private fun Bubble(m: dk.maarse.app.Msg, isDm: Boolean) {
    val mine = m.mine
    Row(Modifier.fillMaxWidth().padding(vertical = 3.dp), horizontalArrangement = if (mine) Arrangement.End else Arrangement.Start) {
        Column(
            Modifier.widthIn(max = 300.dp)
                .background(if (mine) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surfaceVariant, RoundedCornerShape(14.dp))
                .padding(horizontal = 12.dp, vertical = 8.dp),
        ) {
            if (!mine && !isDm) Text(Node.label(m.from), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary)
            Text(m.text)
            val (st, color) = statusText(m.status, channel = !isDm)
            val meta = buildList {
                add(time(m.ts))
                if (!mine && m.hops > 0) add("${m.hops} hops")
                if (mine && st.isNotEmpty()) add(st)
            }.joinToString(" · ")
            Text(meta, style = MaterialTheme.typography.labelSmall, color = color ?: MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

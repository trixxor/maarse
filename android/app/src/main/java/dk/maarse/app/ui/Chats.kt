package dk.maarse.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.Chat
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Link
import androidx.compose.material.icons.filled.QrCodeScanner
import androidx.compose.material.icons.filled.Tag
import androidx.compose.material.icons.filled.Warning
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text as RawText
import androidx.compose.material3.TextButton
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.material.icons.filled.AttachFile
import androidx.compose.material.icons.filled.Call
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Description
import androidx.compose.material.icons.filled.Mic
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material.icons.filled.Share
import androidx.compose.foundation.gestures.transformable
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import dk.maarse.app.Images
import android.graphics.BitmapFactory
import dk.maarse.app.Voice
import dk.maarse.app.VoicePlayer
import dk.maarse.app.VoiceRecorder
import dk.maarse.app.Conn
import dk.maarse.app.Msg
import dk.maarse.app.Mentions
import dk.maarse.app.Node
import dk.maarse.app.tr
import dk.maarse.app.NodeEntry
import dk.maarse.app.Sensors
import dk.maarse.app.shortAddr
import kotlinx.coroutines.delay
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

// ---------------------------------------------------------------------------------------------------
// Chats: the list (Channels, Direct, MeshCore), an alert card only when there is an alert
// ---------------------------------------------------------------------------------------------------

private val FILTERS = listOf("All", "Unread", "People", "Channels", "MeshCore")

@OptIn(ExperimentalFoundationApi::class)
@Composable
fun ChatsScreen(onOpen: (String) -> Unit, onAlerts: () -> Unit = {}, onNew: () -> Unit = {}, onSensor: (String) -> Unit = {}) {
    val version by Node.msgVersion.collectAsState()
    val channels by Node.channels.collectAsState()
    val unread by Node.unread.collectAsState()
    Node.nicknames.collectAsState().value
    Node.person.collectAsState().value
    val last = remember(version) { Node.db.lastPerConv() }
    val convs = last.map { it.conv } + channels.keys.map { "ch:$it" }.filter { c -> last.none { it.conv == c } }
    var removing by remember { mutableStateOf<String?>(null) }
    var filterSaved by rememberSaveable { mutableStateOf("All") }
    removing?.let { c ->
        AlertDialog(
            onDismissRequest = { removing = null },
            title = { Text("Delete the chat with ${friendlyName(Node.convTitle(c))}?") },
            text = { Text("Removes it from this phone and your node's history. The other person keeps their copy." +
                if (c.startsWith("ch:")) " You stay in the channel." else " It comes back if they write again.") },
            confirmButton = { TextButton(onClick = { Node.deleteConversation(c); removing = null }) { Text("Delete", color = MaterialTheme.colorScheme.error) } },
            dismissButton = { TextButton(onClick = { removing = null }) { Text("Cancel") } },
        )
    }
    val mc by Node.meshcoreAccess.collectAsState()
    val link by Node.mcLink.collectAsState()
    val showMcChip = link.state != 0
    val ready = link.state == 2 && mc.allowed && mc.key != null
    // The MeshCore list is refreshed while the Chats place is on screen.
    LaunchedEffect(ready) { if (ready) while (true) { Node.mcAsk("overview"); delay(15_000) } }
    val f = if (!showMcChip && filterSaved == "MeshCore") "All" else filterSaved
    val chips = if (showMcChip) FILTERS else FILTERS.dropLast(1)

    val channelConvs = convs.filter { it.startsWith("ch:") && (f == "All" || f == "Channels" || (f == "Unread" && (unread[it] ?: 0) > 0)) }
    val directConvs = convs.filter { it.startsWith("dm:") && (f == "All" || f == "People" || (f == "Unread" && (unread[it] ?: 0) > 0)) }
    val mcRows = if (ready && (f == "All" || f == "MeshCore" || f == "Unread")) mc.conversations.filter { f != "Unread" || it.unread > 0 } else emptyList()
    val nothing = channelConvs.isEmpty() && directConvs.isEmpty() && mcRows.isEmpty()

    Box(Modifier.fillMaxSize()) {
        Column(Modifier.fillMaxSize()) {
            ChipRow(chips, f, { filterSaved = it })
            LazyColumn(Modifier.weight(1f).fillMaxWidth(), contentPadding = PaddingValues(bottom = 96.dp)) {
                item(key = "alert-cards") { AlertCards(onAlerts, onSensor) }
                if (nothing) item(key = "empty") {
                    Column(Modifier.padding(24.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        if (convs.isEmpty() && f == "All") {
                            Text("No conversations yet", style = MaterialTheme.typography.titleMedium)
                            Text("Join a channel or write to someone with the button below. People who write to you show up here.",
                                style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        } else Text(when (f) {
                            "Unread" -> "Nothing unread."
                            "MeshCore" -> if (ready) "No MeshCore chats yet." else "MeshCore is not connected."
                            else -> "Nothing here yet."
                        }, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
                if (channelConvs.isNotEmpty()) item(key = "h-ch") { SectionLabel("Channels") }
                items(channelConvs, key = { it }) { c -> ChatRow(c, last.firstOrNull { it.conv == c }, unread[c] ?: 0, onOpen = { onOpen(c) }, onLong = { removing = c }) }
                if (directConvs.isNotEmpty()) item(key = "h-dm") { SectionLabel("Direct") }
                items(directConvs, key = { it }) { c -> ChatRow(c, last.firstOrNull { it.conv == c }, unread[c] ?: 0, onOpen = { onOpen(c) }, onLong = { removing = c }) }
                if (mcRows.isNotEmpty() || (ready && f == "MeshCore")) item(key = "h-mc") { SectionLabel("MeshCore", meshcore = true) }
                if (ready && f == "MeshCore") item(key = "mc-sending") { McSendingRow() }
                items(mcRows, key = { "mc" + it.kind + it.id }) { c -> McChatRow(c, mc) { McNav.open = c } }
            }
        }
        // Above the tab bar (this box ends where the bar starts); the list has bottom padding so the last row is never hidden.
        FloatingActionButton(
            onClick = onNew, modifier = Modifier.align(Alignment.BottomEnd).padding(16.dp),
            containerColor = MaterialTheme.colorScheme.primary, contentColor = MaterialTheme.colorScheme.onPrimary,
        ) { Icon(Icons.Default.Edit, "New chat") }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ChatRow(c: String, m: Msg?, n: Int, onOpen: () -> Unit, onLong: () -> Unit) {
    val title = friendlyName(Node.convTitle(c))
    val channel = c.startsWith("ch:")
    Row(
        Modifier.fillMaxWidth().heightIn(min = 64.dp).combinedClickable(onClick = onOpen, onLongClick = onLong).padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Avatar(avatarTitle(title), c.substringAfter(":"))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                RawText(title, Modifier.weight(1f), fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                m?.let { Text(time(it.ts), maxLines = 1, style = MaterialTheme.typography.labelSmall, color = if (n > 0) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant) }
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                if (m?.mine == true) DeliveryIcon(m.status, channel)
                if (m == null) Text("No messages yet", Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                else RawText(
                    (if (m.mine) "" else if (channel) Node.label(m.from) + ": " else "") + m.text,
                    Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1, overflow = TextOverflow.Ellipsis,
                )
                if (n > 0) CountBadge(n)
            }
        }
    }
}

/**
 * The alert card on top of Chats, only when there is something to act on: a verified public alert that is
 * live and loud, or a sensor that reports an alarm. Nothing at all otherwise (Alerts stays under Mesh > Details).
 */
@Composable
private fun AlertCards(onAlerts: () -> Unit, onSensor: (String) -> Unit) {
    val alerts by Node.alerts.collectAsState()
    val dismissed by Node.dismissed.collectAsState()
    val sensors by Sensors.list.collectAsState()
    val live = alerts.filter { Node.alertLoud(it) && it.key !in dismissed }.takeLast(2).reversed()
    val alarms = sensors.orEmpty().filter { it.alarm != null }
    if (live.isEmpty() && alarms.isEmpty()) return
    Column(Modifier.padding(top = 4.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        live.forEach { a -> Box(Modifier.clickable(onClick = onAlerts)) { AlertCard(a) { Node.dismissAlert(a) } } }
        alarms.forEach { s ->
            Row(
                Modifier.padding(horizontal = 8.dp).fillMaxWidth().heightIn(min = 56.dp).clip(RoundedCornerShape(12.dp)).background(Red)
                    .clickable { onSensor(s.node) }.padding(horizontal = 14.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Icon(Icons.Default.Warning, null, tint = androidx.compose.ui.graphics.Color.White)
                Column(Modifier.weight(1f)) {
                    RawText(tr("Alarm") + " · " + s.title, color = androidx.compose.ui.graphics.Color.White, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    RawText(s.alarm ?: "", color = androidx.compose.ui.graphics.Color.White, style = MaterialTheme.typography.bodySmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------
// New chat: three ways (a channel, a QR code, a link), your people, the address as a last resort
// ---------------------------------------------------------------------------------------------------

private fun addressIn(text: String): String? = Regex("[0-9a-fA-F]{32}").find(text)?.value?.lowercase()

/** One screen, no tab bar (shown as a sub-page). `onOpen` opens a Må:rse chat; `onDone` just leaves the screen. */
@Composable
fun NewChatScreen(onOpen: (String) -> Unit, onDone: () -> Unit = {}) {
    var dialog by remember { mutableStateOf("") }   // channel, link, address, meshcore
    var error by remember { mutableStateOf("") }
    val nodes by Node.nodes.collectAsState()
    val nicknames by Node.nicknames.collectAsState()
    val me by Node.me.collectAsState()
    val link by Node.mcLink.collectAsState()
    val mc by Node.meshcoreAccess.collectAsState()
    val mcReady = link.state == 2 && mc.allowed && mc.key != null
    val scan = rememberLauncherForActivityResult(com.journeyapps.barcodescanner.ScanContract()) { r ->
        val text = r.contents ?: return@rememberLauncherForActivityResult
        val a = addressIn(text)
        if (a != null) onOpen("dm:$a") else error = "That code does not hold a Må:rse address."
    }
    val people = remember(nodes, nicknames, me) {
        nodes.values.filter { it.addr != me?.addr && (!it.name.isNullOrBlank() || nicknames.containsKey(it.addr)) }.sortedByDescending { it.lastHeard }.take(12)
    }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
        Group {
            GroupRow("Join or create a channel", "#public, #aarhus or your own channel", leading = { RowIcon(Icons.Default.Tag) }) { dialog = "channel" }
            GroupRow("Add someone with a QR code", "Scan the code on their screen", leading = { RowIcon(Icons.Default.QrCodeScanner) }) {
                error = ""
                scan.launch(com.journeyapps.barcodescanner.ScanOptions().setPrompt("Scan their QR code").setBeepEnabled(false).setOrientationLocked(false))
            }
            GroupRow("Add someone from a link", "Paste the link they sent you", last = true, leading = { RowIcon(Icons.Default.Link) }) { dialog = "link" }
        }
        if (error.isNotEmpty()) Text(error, Modifier.padding(horizontal = 20.dp, vertical = 8.dp), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
        if (people.isNotEmpty()) {
            SectionLabel("Your people")
            Group {
                people.forEachIndexed { i, n ->
                    val title = friendlyName(Node.label(n.addr))
                    GroupRow(title, peopleReach(n), last = i == people.lastIndex, leading = { Avatar(avatarTitle(title), n.addr, 40.dp) }) { onOpen("dm:${n.addr}") }
                }
            }
        }
        if (mcReady) {
            SectionLabel("MeshCore", meshcore = true)
            Group {
                GroupRow("Join a MeshCore channel or message a node", "A separate radio network with its own identity", last = true, leading = { RowIcon(Icons.AutoMirrored.Filled.Chat, meshcore = true) }) { dialog = "meshcore" }
            }
        }
        TextButton(onClick = { dialog = "address" }, Modifier.padding(horizontal = 8.dp, vertical = 4.dp).heightIn(min = 48.dp)) { Text("More ways to add: type a 32 character address") }
        Hint("Public channels can be read and written by anyone, and messages can appear on the public Denmark map. Sender names are not verified.")
    }
    when (dialog) {
        "channel" -> JoinChannelDialog(onDone = { joined -> dialog = ""; if (joined) onDone() })
        "link" -> AddAddressDialog(true, onDone = { dialog = ""; it?.let { a -> onOpen("dm:$a") } })
        "address" -> AddAddressDialog(false, onDone = { dialog = ""; it?.let { a -> onOpen("dm:$a") } })
        "meshcore" -> NewMeshCoreChat(onDone = { opened -> dialog = ""; if (opened) onDone() })
    }
}

private fun peopleReach(n: NodeEntry): String = when (n.hops) {
    null -> if (n.lastHeard > 0) "Heard " + time(n.lastHeard) else "Not heard yet"
    0 -> "Nearby · direct"
    else -> "Reachable · " + plural(n.hops, "hop")
}

@Composable
private fun JoinChannelDialog(onDone: (joined: Boolean) -> Unit) {
    var channel by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = { onDone(false) },
        title = { Text("Join or create a channel") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(channel, { channel = it }, Modifier.fillMaxWidth(), label = { Text("Channel name, e.g. public") }, singleLine = true)
                Text("Anyone who knows the name can read and post. A name nobody has used yet creates a new channel.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        },
        confirmButton = { TextButton(onClick = { if (channel.isNotBlank()) { Node.joinChannel(channel); onDone(true) } }, enabled = channel.isNotBlank()) { Text("Join") } },
        dismissButton = { TextButton(onClick = { onDone(false) }) { Text("Cancel") } },
    )
}

/** A pasted link or a typed 32 character address; both are searched for the address. */
@Composable
private fun AddAddressDialog(fromLink: Boolean, onDone: (String?) -> Unit) {
    var text by remember { mutableStateOf("") }
    val ctx = LocalContext.current
    val addr = addressIn(text)
    AlertDialog(
        onDismissRequest = { onDone(null) },
        title = { Text(if (fromLink) "Add someone from a link" else "Type an address") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(text, { text = it }, Modifier.fillMaxWidth(), singleLine = true,
                    label = { Text(if (fromLink) "Their link" else "Their address (32 characters)") },
                    supportingText = { if (text.isNotBlank() && addr == null) Text("No address found yet.") })
                if (fromLink) OutlinedButton(onClick = {
                    val clip = ctx.getSystemService(android.content.ClipboardManager::class.java).primaryClip?.getItemAt(0)?.text?.toString().orEmpty()
                    if (clip.isNotBlank()) text = clip
                }, Modifier.heightIn(min = 48.dp)) { Text("Paste from the clipboard") }
            }
        },
        confirmButton = { TextButton(onClick = { addr?.let { onDone(it) } }, enabled = addr != null) { Text("Open") } },
        dismissButton = { TextButton(onClick = { onDone(null) }) { Text("Cancel") } },
    )
}

/** Chats tab: kept only so that old callers compile. MeshCore chats are now part of the list, never a mode. */
object MeshCoreMode {
    @Deprecated("MeshCore is no longer a mode") var on: Boolean = false
}

// ---------------------------------------------------------------------------------------------------
// The conversation
// ---------------------------------------------------------------------------------------------------

/** How the contact is reached, for the conversation header. */
private fun reachText(conv: String): String {
    if (conv.startsWith("ch:")) return "Channel"
    val n = Node.nodes.value[conv.substringAfter(":")] ?: return "Not heard yet"
    return when (n.hops) {
        null -> "Heard " + time(n.lastHeard)
        0 -> "Nearby · direct"
        else -> "Reachable · " + plural(n.hops, "hop")
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ChatScreen(conv: String, onBack: () -> Unit, onContact: (String) -> Unit) {
    val version by Node.msgVersion.collectAsState()
    val conn by Node.conn.collectAsState()
    val me by Node.me.collectAsState()
    Node.nicknames.collectAsState().value
    val msgs = remember(version, conv) { Node.db.messages(conv) }
    var text by remember { mutableStateOf("") }
    var error by remember { mutableStateOf<String?>(null) }
    var shown by remember { mutableStateOf<Long?>(null) }
    var menu by remember { mutableStateOf<Msg?>(null) }
    val list = rememberLazyListState()
    DisposableEffect(conv) {
        Node.openConv = conv
        onDispose { if (Node.openConv == conv) Node.openConv = null }
    }
    LaunchedEffect(msgs.size) {
        Node.markRead(conv)
        if (msgs.isNotEmpty()) list.scrollToItem(msgs.size)
    }
    val isDm = conv.startsWith("dm:")
    // Messages from another device of my own account arrive from the account address, which no node answers to.
    val person by Node.person.collectAsState()
    val ownAccount = isDm && conv.substringAfter(":") == person?.person
    val title = friendlyName(Node.convTitle(conv))
    // The delivery state in words sits under the last message of mine; tapping any message shows it too.
    val lastOwn = msgs.lastOrNull { it.mine }?.id
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surfaceContainerHigh).imePadding()) {
        Row(
            Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceContainer).padding(horizontal = 4.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            Row(
                Modifier.weight(1f).clip(RoundedCornerShape(12.dp)).clickable(enabled = isDm && !ownAccount) { onContact(conv.substringAfter(":")) }.padding(4.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Avatar(avatarTitle(title), conv.substringAfter(":"), 40.dp)
                Column {
                    RawText(title, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(reachText(conv), maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary)
                }
            }
            if (isDm && !ownAccount && dk.maarse.app.Calls.possible()) {
                val ctx = LocalContext.current
                val perm = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok -> if (ok) dk.maarse.app.Calls.start(conv.substringAfter(":")) }
                IconButton(onClick = {
                    if (ctx.checkSelfPermission(android.Manifest.permission.RECORD_AUDIO) == android.content.pm.PackageManager.PERMISSION_GRANTED) dk.maarse.app.Calls.start(conv.substringAfter(":"))
                    else perm.launch(android.Manifest.permission.RECORD_AUDIO)
                }) { Icon(Icons.Default.Call, "Call (over the internet)") }
            }
        }
        LazyColumn(Modifier.weight(1f).fillMaxWidth(), state = list, contentPadding = PaddingValues(horizontal = 10.dp, vertical = 8.dp)) {
            var lastDay = ""
            msgs.forEach { m ->
                val day = SimpleDateFormat("EEEE d MMM", Locale.getDefault()).format(Date(m.ts))
                if (day != lastDay) {
                    lastDay = day
                    item(key = "d$day${m.id}") {
                        Box(Modifier.fillMaxWidth().padding(vertical = 6.dp), contentAlignment = Alignment.Center) {
                            Text(day, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                                modifier = Modifier.clip(RoundedCornerShape(10.dp)).background(MaterialTheme.colorScheme.surfaceContainer).padding(horizontal = 10.dp, vertical = 4.dp))
                        }
                    }
                }
                item(key = m.id) {
                    Bubble(m, isDm, shown == m.id, m.id == lastOwn, onSender = onContact,
                        onMention = { addr ->
                            // The name the other person announced (the one they are mentioned by), else what we call them.
                            val name = Node.nodes.value[addr]?.name?.takeIf { it.isNotBlank() } ?: Node.label(addr)
                            val h = Mentions.handle(name)
                            if (!text.contains(h)) text = (if (text.isEmpty() || text.endsWith(" ")) text else "$text ") + h
                        },
                        onLong = { menu = m }) { shown = if (shown == m.id) null else m.id }
                }
            }
        }
        menu?.let { MessageMenu(it) { menu = null } }
        error?.let { Text(it, color = MaterialTheme.colorScheme.error, modifier = Modifier.padding(horizontal = 16.dp)) }
        val max = Node.maxText(conv)
        val used = text.toByteArray().size
        val ctx = LocalContext.current
        var askLora by remember { mutableStateOf<Triple<ByteArray, String, String>?>(null) }
        var askPic by remember { mutableStateOf<Triple<ByteArray, String, String>?>(null) }
        val pick = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri ->
            if (uri == null) return@rememberLauncherForActivityResult
            val bytes = runCatching { ctx.contentResolver.openInputStream(uri)?.use { it.readBytes() } }.getOrNull()
            val mime = ctx.contentResolver.getType(uri) ?: "application/octet-stream"
            val name = runCatching {
                ctx.contentResolver.query(uri, arrayOf(android.provider.OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c -> if (c.moveToFirst()) c.getString(0) else null }
            }.getOrNull() ?: "file"
            if (bytes == null) { error = "The file could not be read"; return@rememberLauncherForActivityResult }
            // Route not known to be internet all the way (the node answers
            // mailbox = true, via LoRa = true): over 8 KB the file would only
            // wait. Ask whether to compress it for LoRa.
            Node.queryFileLimit(conv.substringAfter(":")) { l ->
                Node.note(if (l == null) "File route: no answer from the node" else "File route: up to ${l.max / 1024} KB, via LoRa ${l.viaLora}, post office ${l.mailbox}")
                // Over LoRa a picture goes as colour, grey or dots: the user picks.
                val lora = if (l != null) l.viaLora && !l.mailbox else Node.fileMax() <= LORA_FILE
                if (l != null && l.mailbox && bytes.size > LORA_FILE) askLora = Triple(bytes, mime, name)
                else if (mime.startsWith("image/") && lora) askPic = Triple(bytes, mime, name)
                else error = Node.sendFile(conv, bytes, mime, name)
            }
        }
        askLora?.let { (bytes, mime, name) ->
            val image = mime.startsWith("image/")
            AlertDialog(
                onDismissRequest = { askLora = null },
                title = { Text("No internet route") },
                text = {
                    Text(
                        "The recipient is not reachable over the internet right now. Files over 8 KB are not sent over LoRa, " +
                            "so this one (${(bytes.size + 1023) / 1024} KB) waits until there is an internet route." +
                            (if (image) "\n\nCompress the picture to 8 KB and send it over LoRa now?" else "\n\nOnly pictures can be compressed.")
                    )
                },
                confirmButton = {
                    if (image) TextButton(onClick = { askLora = null; askPic = Triple(bytes, mime, name) }) { Text("Send over LoRa") }
                },
                dismissButton = {
                    Row {
                        TextButton(onClick = { askLora = null }) { Text("Cancel") }
                        TextButton(onClick = { askLora = null; error = Node.sendFile(conv, bytes, mime, name) }) { Text("Send full size") }
                    }
                },
            )
        }
        askPic?.let { (bytes, mime, name) ->
            PictureChoice(bytes, mime, onCancel = { askPic = null }) { data, m, ext ->
                askPic = null
                val n = if (data === bytes) name else name.substringBeforeLast('.') + ext
                error = Node.sendFile(conv, data, m, n, LORA_FILE)
            }
        }
        var recording by remember { mutableStateOf<VoiceRecorder?>(null) }
        var recSecs by remember { mutableStateOf(0) }
        LaunchedEffect(recording) {
            while (recording != null) {
                recSecs = ((System.currentTimeMillis() - recording!!.startedAt) / 1000).toInt()
                if (recSecs >= Voice.MAX_S) { val b = recording!!.stop(); recording = null; if (b != null) error = Node.sendFile(conv, b, Voice.MIME, "voice.c2") }
                kotlinx.coroutines.delay(250)
            }
        }
        val startRec = {
            val r = VoiceRecorder()
            if (r.start()) { recording = r; recSecs = 0 } else error = "The microphone could not be opened"
        }
        val micPerm = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok -> if (ok) startRec() else error = "Voice messages need the microphone" }
        if (ownAccount) Text(
            "This is your own account: it shows messages written on your other devices. To answer, use the device you want to write on.",
            Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp).navigationBarsPadding(),
            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        else Row(Modifier.fillMaxWidth().padding(start = 6.dp, end = 10.dp, top = 6.dp, bottom = 12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            if (recording != null) {
                IconButton(onClick = { recording?.cancel(); recording = null }) { Icon(Icons.Default.Close, "Cancel recording") }
                Row(Modifier.weight(1f).clip(RoundedCornerShape(24.dp)).background(MaterialTheme.colorScheme.tertiaryContainer).padding(horizontal = 16.dp, vertical = 14.dp),
                    verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.size(10.dp).clip(CircleShape).background(MaterialTheme.colorScheme.error))
                    Text("  Recording  0:%02d / 0:%02d".format(recSecs, Voice.MAX_S), color = MaterialTheme.colorScheme.onTertiaryContainer)
                }
                Box(Modifier.size(48.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primary).clickable {
                    val b = recording?.stop(); recording = null
                    error = if (b == null) "Too short" else Node.sendFile(conv, b, Voice.MIME, "voice.c2")
                }, contentAlignment = Alignment.Center) { Icon(Icons.AutoMirrored.Filled.Send, "Send voice message", tint = MaterialTheme.colorScheme.onPrimary) }
            } else {
                if (isDm) IconButton(onClick = { pick.launch("*/*") }, enabled = me != null) { Icon(Icons.Default.AttachFile, "Send a picture or file") }
                TextField(
                    text, { text = it; error = null }, Modifier.weight(1f),
                    placeholder = { Text(if (conn.state == Conn.State.ONLINE) "Message" else "Not connected — the message waits") },
                    suffix = { if (used > max * 3 / 4) Text("$used/$max", color = if (used > max) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant) },
                    shape = RoundedCornerShape(24.dp), maxLines = 5,
                    colors = TextFieldDefaults.colors(
                        focusedIndicatorColor = androidx.compose.ui.graphics.Color.Transparent, unfocusedIndicatorColor = androidx.compose.ui.graphics.Color.Transparent,
                        focusedContainerColor = MaterialTheme.colorScheme.surfaceContainer, unfocusedContainerColor = MaterialTheme.colorScheme.surfaceContainer,
                    ),
                    enabled = me != null,
                )
                val canSend = me != null && text.isNotBlank() && used <= max
                if (isDm && text.isBlank()) Box(
                    Modifier.size(48.dp).clip(CircleShape).background(if (me != null) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline)
                        .clickable(enabled = me != null) {
                            if (ctx.checkSelfPermission(android.Manifest.permission.RECORD_AUDIO) == android.content.pm.PackageManager.PERMISSION_GRANTED) startRec()
                            else micPerm.launch(android.Manifest.permission.RECORD_AUDIO)
                        },
                    contentAlignment = Alignment.Center,
                ) { Icon(Icons.Default.Mic, "Record a voice message", tint = MaterialTheme.colorScheme.onPrimary) }
                else Box(
                    Modifier.size(48.dp).clip(CircleShape).background(if (canSend) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outline)
                        .clickable(enabled = canSend) {
                            error = Node.sendText(conv, text)
                            if (error == null) text = ""
                        },
                    contentAlignment = Alignment.Center,
                ) { Icon(Icons.AutoMirrored.Filled.Send, "Send", tint = MaterialTheme.colorScheme.onPrimary) }
            }
        }
    }
}

/** Long press on a message: copy, stop sending, delete. */
@Composable
private fun MessageMenu(m: Msg, onDone: () -> Unit) {
    val ctx = LocalContext.current
    AlertDialog(
        onDismissRequest = onDone,
        title = { Text(if (m.mine) "Your message" else "Message") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                if (m.file == null) TextButton(onClick = {
                    ctx.getSystemService(android.content.ClipboardManager::class.java).setPrimaryClip(android.content.ClipData.newPlainText("Message", m.text)); onDone()
                }) { Text("Copy text") }
                if (Node.cancellable(m)) TextButton(onClick = { Node.cancel(m); onDone() }) {
                    Text(if (m.mine) "Stop sending" else "Stop receiving", color = MaterialTheme.colorScheme.error)
                }
                TextButton(onClick = { Node.deleteMessage(m); onDone() }) { Text("Delete from this phone", color = MaterialTheme.colorScheme.error) }
                if (m.mine && !Node.cancellable(m)) Text("Already sent messages cannot be called back; this only removes your copy.", style = MaterialTheme.typography.bodySmall)
            }
        },
        confirmButton = { TextButton(onClick = onDone) { Text("Close") } },
    )
}

/** A picture, a voice message or a file in a bubble. */
@Composable
private fun Attachment(m: Msg) {
    val ctx = LocalContext.current
    val path = m.file ?: return
    val mime = m.mime ?: ""
    when {
        mime.startsWith("image/") -> {
            val bmp = remember(path) { Images.thumb(path) }
            var full by remember { mutableStateOf(false) }
            bmp?.let {
                androidx.compose.foundation.Image(it.asImageBitmap(), "Picture", Modifier.padding(bottom = 4.dp).width(240.dp).clip(RoundedCornerShape(12.dp))
                    .clickable { full = true }, contentScale = androidx.compose.ui.layout.ContentScale.FillWidth,
                    // Dots (1-bit PNG sent over LoRa) stay sharp when enlarged.
                    filterQuality = if (mime == "image/png" && it.width <= 256) androidx.compose.ui.graphics.FilterQuality.None else androidx.compose.ui.graphics.FilterQuality.Low)
            }
            if (full) FullImage(path, mime) { full = false }
        }
        mime == Voice.MIME -> {
            var playing by remember { mutableStateOf(false) }
            Row(verticalAlignment = Alignment.CenterVertically) {
                IconButton(onClick = {
                    if (playing) { VoicePlayer.stop(); playing = false } else { playing = true; VoicePlayer.play(path) { playing = false } }
                }) { Icon(if (playing) Icons.Default.Stop else Icons.Default.PlayArrow, if (playing) "Stop" else "Play") }
                RawText(m.text, color = MaterialTheme.colorScheme.onSurface)
            }
        }
        else -> Row(Modifier.clip(RoundedCornerShape(10.dp)).clickable { openFile(ctx, path, mime) }.padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Default.Description, null)
            Text("  Open", color = MaterialTheme.colorScheme.primary, fontWeight = FontWeight.SemiBold)
        }
    }
}

/** A picture over the whole screen: pinch to zoom, drag to move, share, close. */
@Composable
private fun FullImage(path: String, mime: String, onClose: () -> Unit) {
    val ctx = LocalContext.current
    val bmp = remember(path) { Images.thumb(path, 2048) }
    var scale by remember { mutableStateOf(1f) }
    var offset by remember { mutableStateOf(androidx.compose.ui.geometry.Offset.Zero) }
    val zoom = androidx.compose.foundation.gestures.rememberTransformableState { z, pan, _ ->
        scale = (scale * z).coerceIn(1f, 6f)
        offset = if (scale == 1f) androidx.compose.ui.geometry.Offset.Zero else offset + pan
    }
    androidx.compose.ui.window.Dialog(onDismissRequest = onClose, properties = androidx.compose.ui.window.DialogProperties(usePlatformDefaultWidth = false)) {
        Box(Modifier.fillMaxSize().background(androidx.compose.ui.graphics.Color.Black)) {
            bmp?.let {
                androidx.compose.foundation.Image(
                    it.asImageBitmap(), "Picture",
                    Modifier.fillMaxSize().androidx_transform(scale, offset).then(Modifier.transformable(zoom)),
                    contentScale = androidx.compose.ui.layout.ContentScale.Fit,
                )
            }
            Row(Modifier.align(Alignment.TopEnd).padding(8.dp)) {
                IconButton(onClick = { openFile(ctx, path, mime) }) { Icon(Icons.Default.Share, "Share or save", tint = androidx.compose.ui.graphics.Color.White) }
                IconButton(onClick = onClose) { Icon(Icons.Default.Close, "Close", tint = androidx.compose.ui.graphics.Color.White) }
            }
        }
    }
}

private fun Modifier.androidx_transform(scale: Float, offset: androidx.compose.ui.geometry.Offset) =
    this.graphicsLayer(scaleX = scale, scaleY = scale, translationX = offset.x, translationY = offset.y)

private fun openFile(ctx: android.content.Context, path: String, mime: String) {
    val uri = androidx.core.content.FileProvider.getUriForFile(ctx, "dk.maarse.app.files", java.io.File(path))
    val i = android.content.Intent(android.content.Intent.ACTION_VIEW).setDataAndType(uri, mime.ifBlank { "*/*" })
        .addFlags(android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION)
    runCatching { ctx.startActivity(android.content.Intent.createChooser(i, "Open with")) }
}

/** The time and, for my own messages, the delivery tick: one line, bottom right in the bubble. */
@Composable
private fun BubbleMeta(m: Msg, isDm: Boolean) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        Text(SimpleDateFormat("HH:mm", Locale.getDefault()).format(Date(m.ts)), maxLines = 1, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (m.mine) DeliveryIcon(m.status, !isDm)
    }
}

/**
 * One message. Short text puts the time and tick on the same line; longer text puts them below.
 * The delivery in words shows under my last message and on tap; received messages carry a route chip
 * (how many hops they came over).
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun Bubble(m: Msg, isDm: Boolean, open: Boolean, lastOwn: Boolean, onSender: (String) -> Unit, onMention: (String) -> Unit, onLong: () -> Unit, onTap: () -> Unit) {
    val mine = m.mine
    val progress = fileProgress(m)
    val plain = m.file == null || !(m.mime == Voice.MIME || m.mime?.startsWith("image/") == true)
    val compact = m.file == null && progress == null && m.text.length <= 22 && !m.text.contains("\n")
    Column {
        BubbleFrame(mine, onClick = onTap, onLong = onLong) {
            // In a channel: tap the sender to mention them (@name), press and hold to see them and write directly.
            // Names are never translated.
            if (!mine && !isDm) RawText(Node.label(m.from), style = MaterialTheme.typography.labelMedium, color = avatarColor(m.from), fontWeight = FontWeight.SemiBold,
                modifier = Modifier.clip(RoundedCornerShape(6.dp)).combinedClickable(onClick = { onMention(m.from) }, onLongClick = { onSender(m.from) }).padding(vertical = 2.dp))
            Attachment(m)
            // A file on its way (either direction): a progress bar, as in the web client.
            progress?.let { p ->
                if (p < 0f) LinearProgressIndicator(Modifier.fillMaxWidth().padding(vertical = 4.dp))
                else LinearProgressIndicator(progress = { p }, modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp))
                Text(
                    if (p < 0f) (if (mine) "Waiting for the recipient…" else "Receiving…") else "${if (mine) "Sending" else "Receiving"} ${(p * 100).toInt()} %",
                    style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            val color = if (mine) MaterialTheme.colorScheme.onPrimaryContainer else MaterialTheme.colorScheme.onSurface
            if (compact) Row(verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                RawText(m.text, color = color)
                BubbleMeta(m, isDm)
            } else {
                // A picture or a voice message speaks for itself.
                if (plain) RawText(m.text, color = color)
                Box(Modifier.align(Alignment.End)) { BubbleMeta(m, isDm) }
            }
            if (open || (mine && lastOwn)) {
                val (words, wordColor) = statusText(m.status, !isDm)
                val how = if (mine) words else if (m.hops > 0) "Came over " + plural(m.hops, "hop") else "Came directly"
                Text(how, style = MaterialTheme.typography.labelSmall, color = readableTone(wordColor) ?: MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
        if (!mine && m.hops > 0) MiniChip(plural(m.hops, "hop"), Modifier.padding(start = 8.dp, bottom = 2.dp))
    }
}

/** Nickname dialog, from the contact page. */
@Composable
fun NicknameDialog(addr: String, onDone: () -> Unit) {
    var nick by remember { mutableStateOf(Node.nicknames.value[addr] ?: "") }
    AlertDialog(
        onDismissRequest = onDone,
        title = { Text("Your name for ${shortAddr(addr)}") },
        text = { OutlinedTextField(nick, { nick = it }, singleLine = true, label = { Text("Only on this phone") }) },
        confirmButton = { TextButton(onClick = { Node.setNickname(addr, nick); onDone() }) { Text("Save") } },
        dismissButton = { TextButton(onClick = onDone) { Text("Cancel") } },
    )
}

/**
 * How far a file on its way has come, 0..1; -1 = started, no chunk yet;
 * null = not on its way. Outgoing: status "sending done/total"; incoming:
 * the label ends in "receiving N %" (or "receiving…").
 */
private fun fileProgress(m: Msg): Float? {
    if (m.mine) {
        if (!m.status.startsWith("sending")) return null
        val parts = m.status.removePrefix("sending").trim().split("/")
        val done = parts.getOrNull(0)?.toFloatOrNull() ?: return -1f
        val total = parts.getOrNull(1)?.toFloatOrNull()?.takeIf { it > 0f } ?: return -1f
        return if (done <= 0f) -1f else (done / total).coerceIn(0f, 1f)
    }
    if (m.file != null) return null
    val t = m.text
    val i = t.lastIndexOf("receiving")
    if (i < 0) return null
    val pct = t.substring(i + 9).trim().removeSuffix("%").trim().toFloatOrNull() ?: return -1f
    return (pct / 100f).coerceIn(0f, 1f)
}

/** Largest file sent over LoRa (maarse-host LORA_MAX_FILE). */
private const val LORA_FILE = 8192

/** How a picture goes over LoRa: colour, grey or dots, each with its real size and air time. */
@Composable
private fun PictureChoice(bytes: ByteArray, mime: String, onCancel: () -> Unit, onPick: (ByteArray, String, String) -> Unit) {
    class Opt(val title: String, val sub: String, val data: ByteArray, val mime: String, val ext: String)
    var opts by remember { mutableStateOf<List<Opt>?>(null) }
    LaunchedEffect(bytes) {
        opts = kotlinx.coroutines.withContext(kotlinx.coroutines.Dispatchers.Default) {
            listOfNotNull(
                (if (bytes.size <= LORA_FILE) bytes else Images.shrink(bytes, LORA_FILE))?.let { Opt("Colour", "Best picture", it, if (it === bytes) mime else "image/webp", ".webp") },
                Images.grey(bytes)?.let { Opt("Grey, small", "Fastest; text stays readable", it, "image/webp", ".webp") },
                Images.dots(bytes)?.let { Opt("Dots", "Retro look, best for drawings", it, "image/png", ".png") },
            )
        }
    }
    AlertDialog(
        onDismissRequest = onCancel,
        title = { Text("This picture goes over LoRa") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Text("Pick how it is sent. Smaller arrives sooner and uses less air time.", style = MaterialTheme.typography.bodyMedium)
                val list = opts
                if (list == null) Text("Making the small versions…", color = MaterialTheme.colorScheme.onSurfaceVariant)
                else list.forEach { o ->
                    val bmp = remember(o) { BitmapFactory.decodeByteArray(o.data, 0, o.data.size) }
                    Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(MaterialTheme.colorScheme.surfaceContainer)
                        .clickable { onPick(o.data, o.mime, o.ext) }.padding(8.dp),
                        horizontalArrangement = Arrangement.spacedBy(12.dp), verticalAlignment = Alignment.CenterVertically) {
                        bmp?.let {
                            androidx.compose.foundation.Image(it.asImageBitmap(), o.title, Modifier.size(72.dp).clip(RoundedCornerShape(6.dp)).background(androidx.compose.ui.graphics.Color.White),
                                contentScale = androidx.compose.ui.layout.ContentScale.Fit,
                                filterQuality = if (o.mime == "image/png") androidx.compose.ui.graphics.FilterQuality.None else androidx.compose.ui.graphics.FilterQuality.Low)
                        }
                        Column(Modifier.weight(1f)) {
                            Text(o.title, fontWeight = FontWeight.SemiBold)
                            Text("%.1f KB · about %d s".format(o.data.size / 1024f, Images.loraSecs(o.data.size)), style = MaterialTheme.typography.bodySmall)
                            Text(o.sub, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    }
                }
            }
        },
        confirmButton = {},
        dismissButton = { TextButton(onClick = onCancel) { Text("Cancel") } },
    )
}

// ---------------------------------------------------------------------------------------------------
// The contact page: the name first, how far you can trust it, the route, the address behind a button
// ---------------------------------------------------------------------------------------------------

@Composable
private fun ActionTile(icon: ImageVector, label: String, modifier: Modifier, onClick: () -> Unit) {
    Column(
        modifier.heightIn(min = 72.dp).clip(RoundedCornerShape(16.dp)).background(MaterialTheme.colorScheme.surfaceContainer).clickable(onClick = onClick).padding(vertical = 12.dp),
        horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        Icon(icon, null, tint = MaterialTheme.colorScheme.primary)
        Text(label, style = MaterialTheme.typography.labelMedium, maxLines = 1)
    }
}

/** One contact. Replaces the old contact screen: shown by the shell for every contact. */
@Composable
fun ContactPage(addr: String, onBack: () -> Unit, onChat: (String) -> Unit) {
    val nodes by Node.nodes.collectAsState()
    Node.nicknames.collectAsState().value
    Node.keys.collectAsState().value
    Node.verified.collectAsState().value
    val traces by Node.traces.collectAsState()
    var naming by remember { mutableStateOf(false) }
    var compare by remember { mutableStateOf(false) }
    val ctx = LocalContext.current
    val n = nodes[addr]
    val raw = Node.label(addr)
    val title = friendlyName(raw)
    val unnamed = isUnnamed(raw)
    val trace = traces.firstOrNull { it.dest == addr }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            Spacer(Modifier.weight(1f))
            FavStar('u', addr)
        }
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Avatar(avatarTitle(title), addr, 88.dp)
            RawText(title, style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold, maxLines = 2, overflow = TextOverflow.Ellipsis)
            n?.name?.let { RawText(tr("Announces as {0}", "~$it"), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
            if (unnamed) OutlinedButton(onClick = { naming = true }, Modifier.heightIn(min = 48.dp)) { Text("Give this contact a name") }
            Text(n?.let { peopleReach(it) } ?: "Not heard yet", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.primary)
        }
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            ActionTile(Icons.AutoMirrored.Filled.Chat, "Message", Modifier.weight(1f)) { onChat("dm:$addr") }
            if (dk.maarse.app.Calls.possible()) ActionTile(Icons.Default.Call, "Call", Modifier.weight(1f)) { dk.maarse.app.Calls.start(addr) }
            ActionTile(Icons.Default.Edit, "Name", Modifier.weight(1f)) { naming = true }
        }
        // Trust: verified, or what to do about it.
        SectionLabel("Trust")
        val verified = Node.isVerified(addr)
        val changed = Node.keyChanged(addr)
        Group {
            GroupRow(
                when { changed -> "The key has changed"; verified -> "Verified"; else -> "Not verified yet" },
                when { changed -> "Compare the safety number again before you trust this contact"; verified -> "You compared the safety number"; else -> "Compare a safety number in person or on a call" },
                last = true,
            ) { compare = !compare }
        }
        if (compare) SafetyNumberCard(addr)
        // Route.
        SectionLabel("Route")
        Card2 {
            if (trace != null) {
                Text(plural(trace.path.size, "hop"), fontWeight = FontWeight.SemiBold)
                Text("Signal out: ${trace.out.joinToString(" · ") { "$it dB" }}", style = MaterialTheme.typography.bodySmall)
                Text("Signal back: ${trace.back.joinToString(" · ") { "$it dB" }}", style = MaterialTheme.typography.bodySmall)
            } else Text(n?.let { peopleReach(it) } ?: "Not heard yet", style = MaterialTheme.typography.bodyMedium)
            OutlinedButton(onClick = { Node.trace(addr) }, Modifier.heightIn(min = 48.dp)) { Text(if (trace == null) "Trace the route" else "Trace route again") }
        }
        n?.let {
            if (it.lat != null && it.lon != null) {
                SectionLabel("Position (shared by them)")
                Card2 { Text("%.4f, %.4f".format(it.lat, it.lon)) }
            }
        }
        // The address: short form and a copy button, not 32 characters on screen.
        SectionLabel("Address")
        Card2 {
            Text(shortAddr(addr) + "…", fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodyLarge)
            Text("Messages to this address are end-to-end encrypted.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            OutlinedButton(onClick = {
                ctx.getSystemService(android.content.ClipboardManager::class.java).setPrimaryClip(android.content.ClipData.newPlainText("Address", addr))
            }, Modifier.heightIn(min = 48.dp)) { Text("Copy full address") }
        }
    }
    if (naming) NicknameDialog(addr) { naming = false }
}

/** The safety number with a contact: compare it with theirs in person or on a call, then mark it verified. */
@Composable
private fun SafetyNumberCard(addr: String) {
    val mine by Node.myKey.collectAsState()
    val num = remember(addr, mine, Node.keys.value[addr]) { Node.safetyNumber(addr) }
    Card2(Modifier.padding(top = 8.dp)) {
        when {
            mine == null -> Text("The node has not reported its own key (older firmware?).", style = MaterialTheme.typography.bodySmall)
            num == null -> Text("The contact's key is not known yet. Wait for an announce from it.", style = MaterialTheme.typography.bodySmall)
            else -> {
                if (Node.keyChanged(addr)) Text("The contact's key has changed since you verified it. Compare the number again before you trust it.",
                    color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
                Text("Compare these numbers with the ones on their screen, in person or on a call where you know their voice. If they are the same, nobody is in between.",
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                // 12 groups of 5 digits, 4 per row.
                num.chunked(5).chunked(4).forEach { row ->
                    Row(Modifier.padding(vertical = 2.dp), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
                        row.forEach { Text(it, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.SemiBold, style = MaterialTheme.typography.titleMedium) }
                    }
                }
                val ok = Node.isVerified(addr)
                OutlinedButton(onClick = { Node.setVerified(addr, !ok) }, Modifier.heightIn(min = 48.dp)) { Text(if (ok) "Remove verification" else "Mark as verified") }
            }
        }
    }
}

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
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.FilterChip
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
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
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
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
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
import dk.maarse.app.Node
import dk.maarse.app.shortAddr
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

@OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)
@Composable
fun ChatsScreen(onOpen: (String) -> Unit, onAlerts: () -> Unit = {}) {
    val version by Node.msgVersion.collectAsState()
    val channels by Node.channels.collectAsState()
    val unread by Node.unread.collectAsState()
    Node.nicknames.collectAsState().value
    val last = remember(version) { Node.db.lastPerConv() }
    val convs = last.map { it.conv } + channels.keys.map { "ch:$it" }.filter { c -> last.none { it.conv == c } }
    var adding by remember { mutableStateOf(false) }
    var removing by remember { mutableStateOf<String?>(null) }
    removing?.let { c ->
        AlertDialog(
            onDismissRequest = { removing = null },
            title = { Text("Delete the chat with ${Node.convTitle(c)}?") },
            text = { Text("Removes it from this phone and your node's history. The other person keeps their copy." +
                if (c.startsWith("ch:")) " You stay in the channel." else " It comes back if they write again.") },
            confirmButton = { TextButton(onClick = { Node.deleteConversation(c); removing = null }) { Text("Delete", color = MaterialTheme.colorScheme.error) } },
            dismissButton = { TextButton(onClick = { removing = null }) { Text("Cancel") } },
        )
    }
    val mc by Node.meshcoreAccess.collectAsState()
    val link by Node.mcLink.collectAsState()
    val showMeshcore = link.state != 0
    val ready = link.state == 2 && mc.allowed && mc.key != null
    if (!ready) MeshCoreMode.on = false
    Column(Modifier.fillMaxSize()) {
    if (showMeshcore) McChips(link.copy(state = if (ready || link.state != 2) link.state else 1), !MeshCoreMode.on, { MeshCoreMode.on = false },
        MeshCoreMode.on, { MeshCoreMode.on = true }, if (mc.totalUnread > 0) "MeshCore (${mc.totalUnread})" else "MeshCore")
    if (ready && MeshCoreMode.on) {
        MeshCoreChats()
    } else Box(Modifier.weight(1f).fillMaxWidth()) {
        if (convs.isEmpty()) Column(Modifier.padding(24.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text("No conversations yet", style = MaterialTheme.typography.titleMedium)
            Text("Join a channel or write to someone with the button below. People who write to you show up here.",
                style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 88.dp)) {
            item(key = "alerts") {
                val alerts by Node.alerts.collectAsState()
                Row(
                    Modifier.fillMaxWidth().clickable(onClick = onAlerts).padding(horizontal = 16.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Avatar("!", "alerts")
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                        Text("Alerts", fontWeight = FontWeight.SemiBold)
                        Text(alerts.lastOrNull()?.text?.takeIf { it.isNotBlank() } ?: if (alerts.isEmpty()) "Verified public warnings" else "${alerts.size} received",
                            style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                }
            }
            items(convs, key = { it }) { c ->
                val m = last.firstOrNull { it.conv == c }
                val n = unread[c] ?: 0
                val title = Node.convTitle(c)
                Row(
                    Modifier.fillMaxWidth().combinedClickable(onClick = { onOpen(c) }, onLongClick = { removing = c }).padding(horizontal = 16.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Avatar(title, c.substringAfter(":"))
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(title, Modifier.weight(1f), fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            m?.let { Text(time(it.ts), style = MaterialTheme.typography.labelSmall, color = if (n > 0) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant) }
                        }
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            if (m?.mine == true) DeliveryIcon(m.status, c.startsWith("ch:"))
                            Text(
                                m?.let { (if (it.mine) "" else if (c.startsWith("ch:")) Node.label(it.from) + ": " else "") + it.text } ?: "No messages yet",
                                Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
                                maxLines = 1, overflow = TextOverflow.Ellipsis,
                            )
                            if (n > 0) Box(
                                Modifier.clip(CircleShape).background(MaterialTheme.colorScheme.primary).padding(horizontal = 7.dp, vertical = 2.dp),
                            ) { Text("$n", color = MaterialTheme.colorScheme.onPrimary, style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.SemiBold) }
                        }
                    }
                }
            }
        }
        FloatingActionButton(
            onClick = { adding = true }, modifier = Modifier.align(Alignment.BottomEnd).padding(16.dp),
            containerColor = MaterialTheme.colorScheme.primary, contentColor = MaterialTheme.colorScheme.onPrimary,
        ) { Icon(Icons.Default.Edit, "New chat") }
    }
    }
    if (adding) NewChatDialog(onDone = { adding = false; it?.let(onOpen) })
}

/** Chats tab: true while the MeshCore messages and channels are shown instead of the conversation list. */
object MeshCoreMode {
    var on by mutableStateOf(false)
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun MeshCoreChips(unread: Int) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        FilterChip(selected = !MeshCoreMode.on, onClick = { MeshCoreMode.on = false }, label = { Text("Chats") })
        FilterChip(selected = MeshCoreMode.on, onClick = { MeshCoreMode.on = true },
            label = { Text(if (unread > 0) "MeshCore ($unread)" else "MeshCore") })
    }
}

@Composable
private fun NewChatDialog(onDone: (String?) -> Unit) {
    var addr by remember { mutableStateOf("") }
    var channel by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = { onDone(null) },
        title = { Text("New chat") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(addr, { addr = Regex("[0-9a-f]{32}").find(it.lowercase())?.value ?: it.trim().lowercase() },
                    label = { Text("Their address or account (32 characters)") }, singleLine = true)
                Text("or join a channel", style = MaterialTheme.typography.labelMedium)
                OutlinedTextField(channel, { channel = it }, label = { Text("Channel name, e.g. public") }, singleLine = true)
                Text("Anyone can read or post in public and hashtag channels. Sender names are not verified.")
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

/** How the contact is reached, for the conversation header. */
private fun reachText(conv: String): String {
    if (conv.startsWith("ch:")) return "Channel"
    val n = Node.nodes.value[conv.substringAfter(":")] ?: return "Not heard yet"
    return when (n.hops) {
        null -> "Heard " + time(n.lastHeard)
        0 -> "Nearby · direct"
        else -> "Reachable · ${n.hops} hops"
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
    val title = Node.convTitle(conv)
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surfaceContainerHigh).imePadding()) {
        Row(
            Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceContainer).padding(horizontal = 4.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            Row(
                Modifier.weight(1f).clip(RoundedCornerShape(12.dp)).clickable(enabled = isDm) { onContact(conv.substringAfter(":")) }.padding(4.dp),
                verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                Avatar(title, conv.substringAfter(":"), 40.dp)
                Column {
                    Text(title, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(reachText(conv), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary)
                }
            }
            if (isDm && dk.maarse.app.Calls.possible()) {
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
                item(key = m.id) { Bubble(m, isDm, shown == m.id, onSender = onContact, onLong = { menu = m }) { shown = if (shown == m.id) null else m.id } }
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
        Row(Modifier.fillMaxWidth().padding(start = 6.dp, end = 10.dp, top = 6.dp, bottom = 12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
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
                Text(m.text, color = MaterialTheme.colorScheme.onSurface)
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

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun Bubble(m: Msg, isDm: Boolean, open: Boolean, onSender: (String) -> Unit, onLong: () -> Unit, onTap: () -> Unit) {
    val mine = m.mine
    Row(Modifier.fillMaxWidth().padding(vertical = 2.dp), horizontalArrangement = if (mine) Arrangement.End else Arrangement.Start) {
        Column(
            Modifier.widthIn(max = 300.dp)
                .clip(RoundedCornerShape(topStart = 18.dp, topEnd = 18.dp, bottomStart = if (mine) 18.dp else 6.dp, bottomEnd = if (mine) 6.dp else 18.dp))
                .background(if (mine) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surfaceContainer)
                .combinedClickable(onClick = onTap, onLongClick = onLong).padding(start = 12.dp, end = 10.dp, top = 7.dp, bottom = 5.dp),
        ) {
            // In a channel: tap the sender to see them and write directly.
            if (!mine && !isDm) Text(Node.label(m.from), style = MaterialTheme.typography.labelMedium, color = avatarColor(m.from), fontWeight = FontWeight.SemiBold,
                modifier = Modifier.clip(RoundedCornerShape(6.dp)).clickable { onSender(m.from) }.padding(vertical = 2.dp))
            Attachment(m)
            // A file on its way (either direction): a progress bar, as in the web client.
            fileProgress(m)?.let { p ->
                if (p < 0f) LinearProgressIndicator(Modifier.fillMaxWidth().padding(vertical = 4.dp))
                else LinearProgressIndicator(progress = { p }, modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp))
                Text(
                    if (p < 0f) (if (mine) "Waiting for the recipient…" else "Receiving…") else "${if (mine) "Sending" else "Receiving"} ${(p * 100).toInt()} %",
                    style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            // A picture or a voice message speaks for itself.
            if (m.file == null || !(m.mime == Voice.MIME || m.mime?.startsWith("image/") == true)) Text(m.text, color = if (mine) MaterialTheme.colorScheme.onPrimaryContainer else MaterialTheme.colorScheme.onSurface)
            Row(Modifier.align(Alignment.End), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(SimpleDateFormat("HH:mm", Locale.getDefault()).format(Date(m.ts)), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                if (mine) DeliveryIcon(m.status, !isDm)
            }
            if (open) {
                val (words, color) = statusText(m.status, !isDm)
                val how = if (mine) words else if (m.hops > 0) "Came over ${m.hops} hops" else "Came directly"
                Text(how, style = MaterialTheme.typography.labelSmall, color = color ?: MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
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

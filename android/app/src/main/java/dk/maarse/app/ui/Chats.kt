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
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import dk.maarse.app.Images
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

@Composable
fun ChatsScreen(onOpen: (String) -> Unit) {
    val version by Node.msgVersion.collectAsState()
    val channels by Node.channels.collectAsState()
    val unread by Node.unread.collectAsState()
    Node.nicknames.collectAsState().value
    val last = remember(version) { Node.db.lastPerConv() }
    val convs = last.map { it.conv } + channels.keys.map { "ch:$it" }.filter { c -> last.none { it.conv == c } }
    var adding by remember { mutableStateOf(false) }
    Box(Modifier.fillMaxSize()) {
        if (convs.isEmpty()) Column(Modifier.padding(24.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text("No conversations yet", style = MaterialTheme.typography.titleMedium)
            Text("Join a channel or write to someone with the button below. People who write to you show up here.",
                style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 88.dp)) {
            items(convs, key = { it }) { c ->
                val m = last.firstOrNull { it.conv == c }
                val n = unread[c] ?: 0
                val title = Node.convTitle(c)
                Row(
                    Modifier.fillMaxWidth().clickable { onOpen(c) }.padding(horizontal = 16.dp, vertical = 10.dp),
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
    if (adding) NewChatDialog(onDone = { adding = false; it?.let(onOpen) })
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
        val pick = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri ->
            if (uri == null) return@rememberLauncherForActivityResult
            val bytes = runCatching { ctx.contentResolver.openInputStream(uri)?.use { it.readBytes() } }.getOrNull()
            val mime = ctx.contentResolver.getType(uri) ?: "application/octet-stream"
            val name = runCatching {
                ctx.contentResolver.query(uri, arrayOf(android.provider.OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c -> if (c.moveToFirst()) c.getString(0) else null }
            }.getOrNull() ?: "file"
            error = if (bytes == null) "The file could not be read" else Node.sendFile(conv, bytes, mime, name)
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
                if (Node.cancellable(m)) TextButton(onClick = { Node.cancel(m); onDone() }) { Text("Stop sending", color = MaterialTheme.colorScheme.error) }
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
            bmp?.let {
                androidx.compose.foundation.Image(it.asImageBitmap(), "Picture", Modifier.padding(bottom = 4.dp).width(240.dp).clip(RoundedCornerShape(12.dp))
                    .clickable { openFile(ctx, path, mime) }, contentScale = androidx.compose.ui.layout.ContentScale.FillWidth)
            }
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

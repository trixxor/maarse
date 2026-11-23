package dk.maarse.app.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.Chat
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.Done
import androidx.compose.material.icons.filled.DoneAll
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.Router
import androidx.compose.material.icons.filled.Sensors
import androidx.compose.material.icons.filled.Star
import androidx.compose.material.icons.filled.StarBorder
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text as RawText
import androidx.compose.material3.TextButton
import androidx.compose.material3.TextField
import androidx.compose.material3.TextFieldDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dk.maarse.app.Mentions
import dk.maarse.app.Node
import dk.maarse.app.hex
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.TextStyle
import java.time.temporal.ChronoUnit
import java.util.Locale

/*
 * MeshCore in the main look (redesign C): chats and nodes use the same rows, bubbles and buttons as Må:rse.
 * MeshCore is only a marker: a violet tag (NetworkTag), a violet section title and a violet unread badge (see Kit.kt, Mc).
 */

object McNav {
    var open by mutableStateOf<Node.McConversation?>(null)
    var node by mutableStateOf<String?>(null)
    var nodeMode by mutableStateOf(false)
    var filter by mutableStateOf("all")
    var loggedIn by mutableStateOf<String?>(null)
    var loggedAt by mutableStateOf("")
    var loginKind by mutableStateOf(0)   // 0 waiting for the answer, 1 administrator, 2 guest, 3 not accepted
    var loginNote by mutableStateOf("")
    val log = mutableStateListOf<Pair<Boolean, String>>()   // (typed by us, text)
    var lastStatus by mutableStateOf("")
    var scanAskedAt by mutableStateOf(0L)
    var advertNote by mutableStateOf("")
}

private fun payload(c: Node.McConversation, verb: String, argument: String = ""): String = when {
    c.kind == "public" -> if (argument.isEmpty()) "public-history" else "public-$verb:$argument"
    else -> {
        val stem = if (c.kind == "channel") "channel" else "direct"
        if (argument.isEmpty()) "$stem-$verb:${c.id}" else "$stem-$verb:${c.id}:$argument"
    }
}

private fun historyPayload(c: Node.McConversation) = if (c.kind == "public") "public-history" else payload(c, "history")

private val zone: ZoneId get() = ZoneId.systemDefault()

/** Today: the time. Yesterday: the word. Within a week: the weekday. Older: the date. */
private fun listTime(seconds: Long): String {
    if (seconds <= 0) return ""
    val moment = Instant.ofEpochSecond(seconds).atZone(zone)
    val days = ChronoUnit.DAYS.between(moment.toLocalDate(), LocalDate.now(zone))
    return when {
        days <= 0 -> "%02d:%02d".format(moment.hour, moment.minute)
        days == 1L -> "Yesterday"
        days < 7 -> moment.dayOfWeek.getDisplayName(TextStyle.SHORT, Locale.ENGLISH)
        else -> "${moment.dayOfMonth}/${moment.monthValue}"
    }
}

private fun clock(seconds: Long): String {
    if (seconds <= 0) return ""
    val moment = Instant.ofEpochSecond(seconds).atZone(zone)
    return "%02d:%02d".format(moment.hour, moment.minute)
}

private fun dayName(seconds: Long): String {
    val date = Instant.ofEpochSecond(seconds).atZone(zone).toLocalDate()
    val days = ChronoUnit.DAYS.between(date, LocalDate.now(zone))
    return when {
        days <= 0 -> "Today"
        days == 1L -> "Yesterday"
        else -> "${date.dayOfMonth}/${date.monthValue}/${date.year}"
    }
}

private fun ago(seen: Long): String {
    if (seen <= 0) return "not heard yet"
    val s = (System.currentTimeMillis() / 1000 - seen).coerceAtLeast(0)
    return when {
        s < 90 -> "just now"
        s < 5400 -> "${(s + 30) / 60} min ago"
        s < 129600 -> "${s / 3600} h ago"
        else -> "${s / 86400} d ago"
    }
}

private fun snrText(n: Node.McNode) = "SNR ${if (n.snr > 0) "+" else ""}${Math.round(n.snr)}"
private fun hops(n: Int) = "$n hop${if (n == 1) "" else "s"}"
private val mcTypes = listOf("Node", "Contact", "Repeater", "Room", "Sensor")

private fun nodeIcon(type: Int): ImageVector = when (type) {
    2 -> Icons.Default.Router
    3 -> Icons.Default.Home
    4 -> Icons.Default.Sensors
    else -> Icons.AutoMirrored.Filled.Chat
}

/** A MeshCore channel avatar: a violet disc with "#". */
@Composable
private fun ChannelDisc(size: Int = 50) {
    Box(Modifier.size(size.dp).clip(CircleShape).background(Mc.container), contentAlignment = Alignment.Center) {
        Text("#", color = Mc.color, fontWeight = FontWeight.SemiBold, fontSize = (size * 0.42).sp)
    }
}

/** A MeshCore repeater, room or sensor: a rounded square with its icon (people use the round avatars). */
@Composable
private fun NodeIcon(type: Int, size: Int = 46) {
    Box(Modifier.size(size.dp).clip(RoundedCornerShape(12.dp)).background(MaterialTheme.colorScheme.primaryContainer), contentAlignment = Alignment.Center) {
        Icon(nodeIcon(type), null, tint = MaterialTheme.colorScheme.onPrimaryContainer, modifier = Modifier.size((size / 2).dp))
    }
}

@Composable
private fun NodeAvatar(n: Node.McNode, size: Int = 46) {
    val name = n.name.ifEmpty { n.key.take(8) + "…" }
    if (n.type in 2..4) NodeIcon(n.type, size) else Avatar(if (n.name.isEmpty()) "?" else name, n.key, size.dp)
}

private val muted: Color @Composable get() = MaterialTheme.colorScheme.onSurfaceVariant

/** "Channel", "Contact", "Repeater", "Room" or "Sensor": what a MeshCore chat is with. */
private fun kindLabel(c: Node.McConversation, mc: Node.MeshCoreAccess): String = when (c.kind) {
    "public", "channel" -> "Channel"
    else -> mcTypes.getOrElse(mc.nodes.firstOrNull { it.key == c.id }?.type ?: 1) { "Contact" }
}

// ------------------------------------------------------------------ Chats list (a section of the Chats place)

/** One MeshCore row in the Chats list: the same row as a Må:rse chat, with its kind and a violet unread badge. */
@Composable
fun McChatRow(c: Node.McConversation, mc: Node.MeshCoreAccess, onClick: () -> Unit) {
    val kind = kindLabel(c, mc)
    Row(
        Modifier.fillMaxWidth().heightIn(min = 64.dp).clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        if (c.kind == "direct") {
            val node = mc.nodes.firstOrNull { it.key == c.id }
            if (node != null && node.type in 2..4) NodeIcon(node.type, 50) else Avatar(if (c.title.endsWith("…")) "?" else c.title, c.id, 50.dp)
        } else ChannelDisc()
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                RawText(c.title, Modifier.weight(1f, fill = false), fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                NetworkTag(kind)
                Box(Modifier.weight(1f))
                Text(listTime(c.time), maxLines = 1, style = MaterialTheme.typography.labelSmall, color = if (c.unread > 0) Mc.color else muted)
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                if (c.last.isEmpty()) Text("No messages yet", Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium, color = muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                else RawText((if (c.mine) "You: " else "") + c.last, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium, color = muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (c.unread > 0) CountBadge(c.unread, meshcore = true)
            }
        }
    }
}

/** "Sending settings . 1 byte path hash": a quiet row at the top of the MeshCore chats. */
@Composable
fun McSendingRow() {
    val mc by Node.meshcoreAccess.collectAsState()
    var open by remember { mutableStateOf(false) }
    TextButton(onClick = { open = true }, Modifier.padding(horizontal = 8.dp).heightIn(min = 48.dp)) {
        Text("Sending settings · ${mc.radio.hashSize} byte path hash" + (if (mc.radio.scope.isEmpty()) "" else " · ${mc.radio.scope.split(",").joinToString(" ") { "#$it" }}"),
            color = Mc.color, style = MaterialTheme.typography.labelLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
    if (open) McSendingSettings(mc.radio) { open = false }
}

/** Path hash size and region scope for everything this account sends over MeshCore. */
@Composable
fun McSendingSettings(radio: Node.McRadio, onDone: () -> Unit) {
    var size by remember { mutableStateOf(radio.hashSize) }
    var region by remember { mutableStateOf(radio.scope) }
    var error by remember { mutableStateOf("") }
    val scope = rememberCoroutineScope()
    AlertDialog(
        onDismissRequest = onDone,
        title = { Text("Sending settings") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("Path hash size", style = MaterialTheme.typography.labelLarge, color = muted)
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    for (n in 1..3) @OptIn(ExperimentalMaterial3Api::class) FilterChip(size == n, { size = n }, label = { Text(if (n == 1) "1 byte" else "$n bytes", maxLines = 1) })
                }
                OutlinedTextField(region, { region = it.replace(" ", "").replace("#", "") }, Modifier.fillMaxWidth(), singleLine = true,
                    label = { Text("Regions, e.g. dk,dk-nordjylland", maxLines = 1) })
                Text("2 or 3 bytes need repeaters that support it. A region scope makes only repeaters with that region pass your messages on. With several regions, each message is sent once per region.", style = MaterialTheme.typography.bodySmall, color = muted)
                if (error.isNotEmpty()) Text(error, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
            }
        },
        confirmButton = {
            TextButton(onClick = {
                if (region.isNotEmpty() && (region.split(",").size > 8 || !region.split(",").all { Regex("[A-Za-z0-9-]{1,24}").matches(it) })) {
                    error = "Up to 8 regions, separated by commas. A region uses letters, digits and -, up to 24 characters."
                } else {
                    error = "Saving…"
                    scope.launch {
                        val answer = Node.mcCall("set-radio:$size:${region.toByteArray().hex()}")
                        if (answer?.radio?.hashSize == size && answer.radio.scope == region) { Node.mcAsk("overview"); onDone() }
                        else error = "Not saved. The backbone server did not accept it: it probably needs updating to the newest version."
                    }
                }
            }) { Text("Save") }
        },
        dismissButton = { TextButton(onClick = onDone) { Text("Cancel") } },
    )
}

/** New MeshCore chat: a channel name or a node's public key. `onDone(true)` when a chat was started or a channel joined. */
@Composable
fun NewMeshCoreChat(onDone: (opened: Boolean) -> Unit) {
    var channel by remember { mutableStateOf("") }
    var key by remember { mutableStateOf("") }
    var error by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = { onDone(false) },
        title = { Text("New MeshCore chat") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(channel, { channel = it.lowercase().removePrefix("#") }, Modifier.fillMaxWidth(), singleLine = true,
                    label = { Text("Join a channel, e.g. hike", maxLines = 1) })
                OutlinedTextField(key, { key = it.trim().lowercase() }, Modifier.fillMaxWidth(), singleLine = true,
                    label = { Text("or a node's public key (64 characters)", maxLines = 1) })
                Text("You can also start a chat from Nodes, where you see who is nearby.", style = MaterialTheme.typography.bodySmall, color = muted)
                if (error.isNotEmpty()) Text(error, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
            }
        },
        confirmButton = {
            TextButton(onClick = {
                when {
                    Regex("[a-z0-9-]{1,24}").matches(channel) -> { Node.mcAsk("channel-join:$channel"); Node.mcAsk("overview"); onDone(true) }
                    Regex("[0-9a-f]{64}").matches(key) -> { McNav.open = Node.McConversation("direct", key, key.take(8) + "…", "", false, 0, 0); onDone(true) }
                    else -> error = "Enter a channel name (a-z, 0-9, -) or a 64-character public key."
                }
            }) { Text("Continue") }
        },
        dismissButton = { TextButton(onClick = { onDone(false) }) { Text("Cancel") } },
    )
}

// ------------------------------------------------------------------ Conversation

/** The time, and for my own messages how far the message got (the words under the bubble). */
private fun deliveryState(c: Node.McConversation, m: Node.McMessage): String {
    if (!m.mine) return ""
    return when {
        c.kind == "direct" -> if (m.delivered) "Delivered" else "Sent"
        m.repeats > 0 -> "Heard by ${m.repeats} repeater${if (m.repeats == 1) "" else "s"}" +
            (if (m.snr != 0.0) " · best SNR ${if (m.snr > 0) "+" else ""}${Math.round(m.snr)}" else "")
        m.repeats == 0 -> "Sent · not heard by a repeater yet"
        else -> "Sent"
    }
}

/** A MeshCore conversation as a full screen without the tab bar. Same bubbles as a Må:rse chat. */
@Composable
fun McChatScreen(c: Node.McConversation, onBack: () -> Unit) {
    val mc by Node.meshcoreAccess.collectAsState()
    // Keep the list and this conversation fresh while it is open.
    LaunchedEffect(c) {
        while (true) {
            Node.mcAsk("overview")
            Node.mcAsk(historyPayload(c))
            delay(6_000)
        }
    }
    BackHandler(onBack = onBack)
    val items = when (c.kind) {
        "public" -> mc.items
        "channel" -> mc.channel?.takeIf { it.contact == c.id }?.items.orEmpty()
        else -> mc.direct?.takeIf { it.contact == c.id }?.items.orEmpty()
    }
    val lastId = when (c.kind) {
        "public" -> mc.lastId
        "channel" -> mc.channel?.takeIf { it.contact == c.id }?.lastId ?: 0
        else -> mc.direct?.takeIf { it.contact == c.id }?.lastId ?: 0
    }
    LaunchedEffect(c, lastId) { if (lastId > 0) { Node.mcAsk(payload(c, "read", lastId.toString())); Node.mcAsk("overview") } }
    val node = if (c.kind == "direct") mc.nodes.firstOrNull { it.key == c.id } else null
    val max = if (c.kind == "direct") 60 else 90
    var text by remember { mutableStateOf("") }
    var shown by remember { mutableStateOf<Int?>(null) }
    val state = rememberLazyListState()
    LaunchedEffect(items.size) { if (items.isNotEmpty()) state.scrollToItem(items.size) }
    val lastOwn = items.indexOfLast { it.mine }
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surfaceContainerHigh)) {
        Row(
            Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceContainer).padding(horizontal = 4.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            if (c.kind == "direct") {
                if (node != null && node.type in 2..4) NodeIcon(node.type, 40) else Avatar(if (c.title.endsWith("…")) "?" else c.title, c.id, 40.dp)
            } else ChannelDisc(40)
            Column(Modifier.weight(1f)) {
                RawText(c.title, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(
                    when (c.kind) {
                        "public" -> "MeshCore · public channel"
                        "channel" -> "MeshCore · channel"
                        else -> "MeshCore" + (node?.let { " · ${hops(it.hops)} · ${snrText(it)}" } ?: " · direct message")
                    }, style = MaterialTheme.typography.labelSmall, color = Mc.color, maxLines = 1, overflow = TextOverflow.Ellipsis,
                )
            }
            if (node != null) IconButton(onClick = { Node.mcAsk("favorite:${node.key}:${if (node.favorite) 0 else 1}"); Node.mcAsk("nodes") }) {
                Icon(if (node.favorite) Icons.Default.Star else Icons.Default.StarBorder, if (node.favorite) "Remove from favourites" else "Add to favourites",
                    tint = if (node.favorite) Mc.color else muted)
            }
        }
        LazyColumn(Modifier.weight(1f).fillMaxWidth(), state = state, contentPadding = PaddingValues(horizontal = 10.dp, vertical = 8.dp)) {
            if (items.isEmpty()) item { Text("No messages yet. Say hello.", Modifier.padding(16.dp), color = muted) }
            items(items.size) { i ->
                val m = items[i]
                val newDay = i == 0 || dayName(items[i - 1].ts) != dayName(m.ts)
                Column(Modifier.fillMaxWidth()) {
                    if (newDay && m.ts > 0) Box(Modifier.fillMaxWidth().padding(vertical = 6.dp), contentAlignment = Alignment.Center) {
                        Text(dayName(m.ts), style = MaterialTheme.typography.labelSmall, color = muted,
                            modifier = Modifier.clip(RoundedCornerShape(10.dp)).background(MaterialTheme.colorScheme.surfaceContainer).padding(horizontal = 10.dp, vertical = 4.dp))
                    }
                    McBubble(c, m, open = shown == i, lastOwn = i == lastOwn, onTap = { shown = if (shown == i) null else i },
                        onMention = { sender ->
                            val h = Mentions.handle(sender)
                            if (!text.contains(h)) text = (if (text.isEmpty() || text.endsWith(" ")) text else "$text ") + h
                        })
                }
            }
        }
        Row(
            Modifier.fillMaxWidth().padding(start = 10.dp, end = 10.dp, top = 6.dp, bottom = 12.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically,
        ) {
            TextField(
                text, { if (it.toByteArray().size <= max) text = it }, Modifier.weight(1f),
                placeholder = { Text("Write a message", maxLines = 1) },
                suffix = { Text("${text.toByteArray().size}/$max", style = MaterialTheme.typography.labelSmall, color = muted) },
                shape = RoundedCornerShape(24.dp), maxLines = 3,
                colors = TextFieldDefaults.colors(
                    focusedIndicatorColor = Color.Transparent, unfocusedIndicatorColor = Color.Transparent,
                    focusedContainerColor = MaterialTheme.colorScheme.surfaceContainer, unfocusedContainerColor = MaterialTheme.colorScheme.surfaceContainer,
                ),
            )
            // The send button carries the MeshCore marker colour, so it is clear which network the message goes to.
            Box(
                Modifier.size(48.dp).clip(CircleShape).background(if (text.isBlank()) MaterialTheme.colorScheme.outline else Mc.color).clickable(enabled = text.isNotBlank()) {
                    Node.mcAsk(payload(c, "send", text.trim().toByteArray().hex())); Node.mcAsk(historyPayload(c)); Node.mcAsk("overview")
                    text = ""
                },
                contentAlignment = Alignment.Center,
            ) { Icon(Icons.AutoMirrored.Filled.Send, "Send", tint = if (text.isBlank()) MaterialTheme.colorScheme.surface else Mc.onColor) }
        }
    }
}

/** One MeshCore message in the same bubble as a Må:rse message. The sender of a channel message sits above the text. */
@Composable
private fun McBubble(c: Node.McConversation, m: Node.McMessage, open: Boolean, lastOwn: Boolean, onTap: () -> Unit, onMention: (String) -> Unit) {
    // In a channel the radio writes "Name: text": show the name as a label, the text without it.
    val sender = if (!m.mine && c.kind != "direct") Mentions.mcSender(m.text) else null
    val body = if (sender != null) m.text.removePrefix("$sender: ") else m.text
    val compact = sender == null && body.length <= 22 && '\n' !in body
    val delivered = m.mine && (m.delivered || m.repeats > 0)
    val meta: @Composable () -> Unit = {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            Text(clock(m.ts), maxLines = 1, style = MaterialTheme.typography.labelSmall, color = muted)
            if (m.mine) Icon(if (delivered) Icons.Filled.DoneAll else Icons.Filled.Done, if (delivered) "Delivered" else "Sent", Modifier.size(15.dp),
                tint = if (delivered) MaterialTheme.colorScheme.primary else muted)
        }
    }
    BubbleFrame(m.mine, onClick = onTap) {
        if (sender != null) RawText(sender, style = MaterialTheme.typography.labelMedium, color = avatarColor(sender), fontWeight = FontWeight.SemiBold,
            modifier = Modifier.clip(RoundedCornerShape(6.dp)).clickable { onMention(sender) }.padding(vertical = 2.dp))
        val color = if (m.mine) MaterialTheme.colorScheme.onPrimaryContainer else MaterialTheme.colorScheme.onSurface
        if (compact) Row(verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(10.dp)) { RawText(body, color = color); meta() }
        else { RawText(body, color = color); Box(Modifier.align(Alignment.End)) { meta() } }
        if (m.mine && (open || lastOwn)) Text(deliveryState(c, m), style = MaterialTheme.typography.labelSmall, color = if (delivered) MaterialTheme.colorScheme.primary else muted)
    }
}

// ------------------------------------------------------------------ Nodes tab

/** The Må:rse | MeshCore switch on the Nodes tab. */
@Composable
fun McNodeChips() {
    val link by Node.mcLink.collectAsState()
    McChips(link, !McNav.nodeMode, { McNav.nodeMode = false }, McNav.nodeMode, { McNav.nodeMode = true }, "MeshCore")
}

/** Shared switch: MeshCore is disabled while it cannot be reached, with the reason below. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun McChips(link: Node.McLink, marseOn: Boolean, onMarse: () -> Unit, mcOn: Boolean, onMeshcore: () -> Unit, label: String) {
    val ready = link.state == 2
    Column {
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChip(selected = marseOn, onClick = onMarse, label = { Text("Må:rse", maxLines = 1) })
            FilterChip(selected = mcOn && ready, onClick = { if (ready) onMeshcore() }, enabled = ready, label = { Text(label, maxLines = 1) })
        }
        if (!ready && link.state != 0) Text(
            if (link.state == 1) "Connecting to MeshCore…" else "MeshCore is not available. ${link.note} Trying again…",
            Modifier.padding(horizontal = 16.dp, vertical = 2.dp), style = MaterialTheme.typography.bodySmall, color = if (link.state == 1) muted else MaterialTheme.colorScheme.error,
        )
    }
}

private val NODE_FILTERS = listOf("all" to "All", "fav" to "Favourites", "2" to "Repeaters", "1" to "Contacts", "3" to "Rooms", "4" to "Sensors")

@Composable
fun MeshCoreNodes(onChat: () -> Unit = {}) {
    val mc by Node.meshcoreAccess.collectAsState()
    LaunchedEffect(Unit) {
        while (true) {
            Node.mcAsk("nodes")
            val s = Node.meshcoreAccess.value.scan
            val recent = McNav.scanAskedAt > 0 && System.currentTimeMillis() - McNav.scanAskedAt < 25_000
            delay(if (s.waiting || s.active || recent) 2_000 else 15_000)
        }
    }
    val detail = McNav.node?.let { key -> mc.nodes.firstOrNull { it.key == key } }
    if (detail != null) { McNodeScreen(detail, mc, onChat); return }
    val shown = mc.nodes.filter { n ->
        when (McNav.filter) { "all" -> true; "fav" -> n.favorite; else -> n.type.toString() == McNav.filter }
    }
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("Heard by your MeshCore radio", Modifier.weight(1f), style = MaterialTheme.typography.bodySmall, color = muted, maxLines = 2)
            var advertMenu by remember { mutableStateOf(false) }
            OutlinedButton(onClick = { advertMenu = true }, shape = RoundedCornerShape(18.dp), contentPadding = PaddingValues(horizontal = 14.dp)) { Text("Advert", maxLines = 1) }
            if (advertMenu) AlertDialog(
                onDismissRequest = { advertMenu = false },
                title = { Text("Send an advert") },
                text = { Text("An advert tells other nodes that you exist. A zero-hop advert is heard only by nodes in direct radio range. A flood advert is passed on by repeaters, so far more nodes hear it, but it uses more airtime.") },
                confirmButton = {
                    Column(horizontalAlignment = Alignment.End) {
                        TextButton(onClick = { advertMenu = false; Node.mcAsk("announce-flood"); McNav.advertNote = "Flood advert sent. Repeaters pass it on." }) { Text("Flood advert") }
                        TextButton(onClick = { advertMenu = false; Node.mcAsk("announce"); McNav.advertNote = "Zero-hop advert sent. Nodes in direct range hear it." }) { Text("Zero-hop advert") }
                    }
                },
                dismissButton = { TextButton(onClick = { advertMenu = false }) { Text("Cancel") } },
            )
            OutlinedButton(
                onClick = { McNav.scanAskedAt = System.currentTimeMillis(); Node.mcAsk("scan"); Node.mcAsk("nodes") },
                enabled = !mc.scan.waiting && !mc.scan.active, shape = RoundedCornerShape(18.dp), contentPadding = PaddingValues(horizontal = 14.dp),
            ) { Text("Scan now", maxLines = 1) }
        }
        if (McNav.advertNote.isNotEmpty()) Text(McNav.advertNote, Modifier.padding(horizontal = 16.dp, vertical = 2.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.primary)
        ScanStatus(mc)
        ChipRow(NODE_FILTERS.map { it.second }, NODE_FILTERS.first { it.first == McNav.filter }.second, { label -> McNav.filter = NODE_FILTERS.first { it.second == label }.first })
        if (shown.isEmpty()) Text(if (mc.nodes.isEmpty()) "No nodes heard yet. Choose Scan now, or wait for adverts." else "Nothing in this filter.",
            Modifier.padding(24.dp), color = muted)
        LazyColumn(contentPadding = PaddingValues(bottom = 16.dp)) {
            val favourites = shown.filter { it.favorite }
            if (favourites.isNotEmpty()) item { SectionLabel("Favourites", meshcore = true) }
            items(favourites, key = { "f" + it.key }) { McNodeRow(it) }
            val others = shown.filter { !it.favorite }
            if (others.isNotEmpty()) item { SectionLabel("Heard nearby · ${others.size}", meshcore = true) }
            items(others, key = { it.key }) { McNodeRow(it) }
        }
    }
}

/** What the scan is doing, and what it has found so far. */
@Composable
private fun ScanStatus(mc: Node.MeshCoreAccess) {
    val asked = McNav.scanAskedAt
    val sinceAsk = System.currentTimeMillis() - asked
    val scan = mc.scan
    val answered = mc.nodes.filter { it.hops == 0 && it.seen >= asked / 1000 - 2 }.sortedByDescending { it.seen }
    val total = maxOf(scan.found, answered.size)
    val text: String
    val running: Boolean
    when {
        scan.active -> { text = "Scanning for nearby MeshCore nodes… $total found so far"; running = true }
        scan.waiting -> { text = "Waiting for the radio to start the scan…"; running = true }
        asked > 0 && sinceAsk < 8_000 -> { text = "Asking the radio to scan…"; running = true }
        asked > 0 && sinceAsk < 30_000 && total == 0 -> {
            text = "Scan finished: no node answered. Nodes more than one hop away are found by their adverts instead."; running = false
        }
        asked > 0 && sinceAsk < 30_000 -> { text = "Scan complete: $total node${if (total == 1) "" else "s"} answered"; running = false }
        else -> return
    }
    Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(text, style = MaterialTheme.typography.bodySmall, color = if (running) Mc.color else MaterialTheme.colorScheme.primary)
        if (running) LinearProgressIndicator(Modifier.fillMaxWidth(), color = Mc.color, trackColor = Mc.container)
        if (answered.isNotEmpty()) Text(
            answered.take(4).joinToString(" · ") { it.name.ifEmpty { it.key.take(8) + "…" } } + if (answered.size > 4) " · +${answered.size - 4} more" else "",
            style = MaterialTheme.typography.labelSmall, color = muted, maxLines = 2, overflow = TextOverflow.Ellipsis,
        )
    }
}

@Composable
private fun McNodeRow(n: Node.McNode) {
    val name = n.name.ifEmpty { n.key.take(8) + "…" }
    Row(
        Modifier.fillMaxWidth().heightIn(min = 64.dp).clickable { McNav.node = n.key }.padding(start = 16.dp, end = 4.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        NodeAvatar(n)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            RawText(name, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text("${mcTypes.getOrElse(n.type) { "Node" }} · ${hops(n.hops)} · ${ago(n.seen)}", style = MaterialTheme.typography.bodySmall, color = muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        if (n.seen > 0) SignalBars(Math.round(n.snr).toInt())
        IconButton(onClick = { Node.mcAsk("favorite:${n.key}:${if (n.favorite) 0 else 1}"); Node.mcAsk("nodes") }) {
            Icon(if (n.favorite) Icons.Default.Star else Icons.Default.StarBorder, if (n.favorite) "Remove from favourites" else "Add to favourites",
                tint = if (n.favorite) Mc.color else muted)
        }
    }
}

// ------------------------------------------------------------------ One node

@Composable
private fun McNodeScreen(n: Node.McNode, mc: Node.MeshCoreAccess, onChat: () -> Unit) {
    val name = n.name.ifEmpty { n.key.take(8) + "…" }
    val loggedIn = McNav.loggedIn == n.key
    BackHandler { if (loggedIn) McNav.loggedIn = null else McNav.node = null }
    if (loggedIn) { LoggedIn(n, name, mc); return }
    var sheet by remember { mutableStateOf(false) }
    val admin = n.type in 2..4   // repeaters, rooms and sensors are logged in to
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = { McNav.node = null }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            Text("Node", Modifier.weight(1f), style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
            IconButton(onClick = { Node.mcAsk("favorite:${n.key}:${if (n.favorite) 0 else 1}"); Node.mcAsk("nodes") }) {
                Icon(if (n.favorite) Icons.Default.Star else Icons.Default.StarBorder, if (n.favorite) "Remove from favourites" else "Add to favourites",
                    tint = if (n.favorite) Mc.color else muted)
            }
        }
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                NodeAvatar(n, 72)
                RawText(name, style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.Bold, maxLines = 2, overflow = TextOverflow.Ellipsis)
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    NetworkTag("MeshCore " + mcTypes.getOrElse(n.type) { "Node" }.lowercase())
                    Text("${n.key.take(4)}…${n.key.takeLast(4)}", style = MaterialTheme.typography.labelMedium, fontFamily = FontFamily.Monospace, color = muted)
                }
            }
            val facts = listOf(
                "Last heard" to ago(n.seen), "Hops" to n.hops.toString(),
                "SNR / RSSI" to if (n.seen > 0) "${if (n.snr > 0) "+" else ""}${Math.round(n.snr)} dB / ${Math.round(n.rssi)}" else "-",
                "Times heard" to n.count.toString(),
            )
            for (pair in facts.chunked(2)) Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                for ((label, value) in pair) Column(
                    Modifier.weight(1f).clip(RoundedCornerShape(12.dp)).background(MaterialTheme.colorScheme.surfaceContainer).padding(horizontal = 14.dp, vertical = 10.dp),
                    verticalArrangement = Arrangement.spacedBy(2.dp),
                ) { Text(label, style = MaterialTheme.typography.labelMedium, color = muted, maxLines = 1); Text(value, fontWeight = FontWeight.SemiBold, maxLines = 1) }
            }
            if (admin) Text("Administration needs the repeater's own password. Log in to see its status and run commands.", style = MaterialTheme.typography.bodySmall, color = muted)
        }
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp)) {
            Button(
                onClick = {
                    if (admin) sheet = true else {
                        McNav.node = null
                        McNav.open = Node.McConversation("direct", n.key, name, "", false, 0, 0)
                        McNav.nodeMode = false
                        onChat()   // show the Chats place, where the conversation is
                    }
                }, Modifier.fillMaxWidth().heightIn(min = 48.dp), shape = RoundedCornerShape(24.dp),
            ) { Text(if (admin) "Log in" else "Send message", fontWeight = FontWeight.SemiBold) }
        }
    }
    if (sheet) LoginSheet(n, name) { sheet = false }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun LoginSheet(n: Node.McNode, name: String, onDone: () -> Unit) {
    var password by remember { mutableStateOf("") }
    ModalBottomSheet(onDismissRequest = onDone) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp).padding(bottom = 28.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                NodeAvatar(n, 40)
                Column(Modifier.weight(1f)) {
                    RawText("Log in to $name", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text("Sent to the repeater over the radio", style = MaterialTheme.typography.bodySmall, color = muted)
                }
            }
            OutlinedTextField(
                password, { password = it }, Modifier.fillMaxWidth(), singleLine = true, label = { Text("Repeater password") },
                visualTransformation = PasswordVisualTransformation(), keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
            )
            Text("The password is not saved. You type it each time you log in, and it is only used for this session.", style = MaterialTheme.typography.bodySmall, color = muted)
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                OutlinedButton(onClick = onDone, Modifier.weight(1f).heightIn(min = 48.dp)) { Text("Cancel") }
                Button(
                    onClick = {
                        val bytes = password.toByteArray()
                        if (bytes.isNotEmpty() && bytes.size <= 60) {
                            McNav.log.clear(); McNav.lastStatus = ""; McNav.loginKind = 0; McNav.loginNote = ""
                            McNav.loggedIn = n.key
                            McNav.loggedAt = java.time.LocalTime.now().let { "%02d:%02d".format(it.hour, it.minute) }
                            Node.mcAsk("repeater-login:${n.key}:${bytes.hex()}")
                            Node.mcAsk("repeater-status:${n.key}")
                            password = ""
                            onDone()
                        }
                    }, Modifier.weight(1f).heightIn(min = 48.dp), enabled = password.isNotEmpty(),
                ) { Text("Log in") }
            }
        }
    }
}

/** After the login: wait for the repeater's answer, then show the admin screen (or why it did not work). */
@Composable
private fun LoggedIn(n: Node.McNode, name: String, mc: Node.MeshCoreAccess) {
    val status = if (mc.repeaterKey == n.key) mc.repeaterStatus else ""
    LaunchedEffect(status) {
        if (McNav.loginKind == 0 && status.isNotBlank() && !status.startsWith("Waiting for repeater login") && status != "Request queued.") {
            when {
                status.startsWith("Administrator login accepted") -> McNav.loginKind = 1
                status.startsWith("Guest login accepted") -> McNav.loginKind = 2
                else -> { McNav.loginKind = 3; McNav.loginNote = status }
            }
        }
    }
    LaunchedEffect(n.key, McNav.loginKind) { while (McNav.loginKind == 0) { Node.mcAsk("repeater-status:${n.key}"); delay(1_500) } }
    when (McNav.loginKind) {
        1, 2 -> RepeaterAdminScreen(n, name, mc, admin = McNav.loginKind == 1, onLogout = { McNav.loggedIn = null; McNav.loginKind = 0 })
        3 -> Column(Modifier.fillMaxSize().padding(24.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            RawText("Could not log in to $name", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold)
            Text(McNav.loginNote, color = muted)
            Button(onClick = { McNav.loggedIn = null; McNav.loginKind = 0 }, Modifier.heightIn(min = 48.dp)) { Text("Try again") }
        }
        else -> Column(Modifier.fillMaxSize().padding(24.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            RawText("Logging in to $name…", style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.SemiBold)
            Text("The password is sent over the radio. This takes a few seconds.", color = muted)
            LinearProgressIndicator(Modifier.fillMaxWidth(), color = Mc.color, trackColor = Mc.container)
        }
    }
}

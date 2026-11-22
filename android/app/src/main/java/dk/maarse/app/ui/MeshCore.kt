package dk.maarse.app.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.Chat
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.Router
import androidx.compose.material.icons.filled.Sensors
import androidx.compose.material.icons.filled.Star
import androidx.compose.material.icons.filled.StarBorder
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.FilterChipDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text as RawText
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.TextStyle as UiTextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dk.maarse.app.Node
import dk.maarse.app.hex
import kotlinx.coroutines.delay
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.TextStyle
import java.time.temporal.ChronoUnit
import java.util.Locale

/** MeshCore as chats and nodes: the same overview as Må:rse, in amber (see the design mockups). */
private val McAmber = Color(0xFFE9B25A)
private val McAmberDark = Color(0xFF3A2C10)
private val McAmberText = Color(0xFF2A1B00)
private val McAmberBubbleText = Color(0xFFF6E3BD)
private val McPerson = Color(0xFF1D2A24)
private val McRoom = Color(0xFF12342A)
private val McGreen = Color(0xFF5CC8A1)
private val McDanger = Color(0xFFF08A7A)
private val McLight = Color(0xFFE8F1EC)

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
private val mcTypes = listOf("Node", "Chat", "Repeater", "Room", "Sensor")

private data class Look(val bg: Color, val fg: Color, val icon: ImageVector)
private fun look(type: Int) = when (type) {
    2 -> Look(McAmberDark, McAmber, Icons.Default.Router)
    3 -> Look(McRoom, McGreen, Icons.Default.Home)
    4 -> Look(McPerson, McLight, Icons.Default.Sensors)
    else -> Look(McPerson, McLight, Icons.AutoMirrored.Filled.Chat)
}

@Composable
private fun Initial(glyph: String, bg: Color, fg: Color, size: Int = 46) {
    Box(Modifier.size(size.dp).clip(CircleShape).background(bg), contentAlignment = Alignment.Center) {
        Text(glyph, color = fg, fontWeight = FontWeight.SemiBold, fontSize = (size * 0.4).sp)
    }
}

@Composable
private fun NodeIcon(type: Int, size: Int = 44) {
    val l = look(type)
    Box(Modifier.size(size.dp).clip(RoundedCornerShape(12.dp)).background(l.bg), contentAlignment = Alignment.Center) {
        Icon(l.icon, null, tint = l.fg, modifier = Modifier.size((size / 2).dp))
    }
}

private val muted: Color @Composable get() = MaterialTheme.colorScheme.onSurfaceVariant

// ------------------------------------------------------------------ Chats tab

@Composable
fun MeshCoreChats() {
    val mc by Node.meshcoreAccess.collectAsState()
    val open = McNav.open
    LaunchedEffect(open) {
        while (true) {
            Node.mcAsk("overview")
            open?.let { Node.mcAsk(historyPayload(it)) }
            delay(if (open != null) 6_000 else 15_000)
        }
    }
    if (open == null) MeshCoreList(mc) else McConversationScreen(open, mc)
}

@Composable
private fun MeshCoreList(mc: Node.MeshCoreAccess) {
    var adding by remember { mutableStateOf(false) }
    Box(Modifier.fillMaxSize()) {
        if (mc.conversations.isEmpty()) Text("Loading conversations…", Modifier.padding(24.dp), color = muted)
        LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 88.dp)) {
            items(mc.conversations, key = { it.kind + it.id }) { c ->
                Row(
                    Modifier.fillMaxWidth().clickable { McNav.open = c }.padding(horizontal = 20.dp, vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    if (c.kind == "direct") Initial(c.title.firstOrNull { it.isLetterOrDigit() }?.uppercase() ?: "?", McPerson, McLight)
                    else Initial("#", McAmberDark, McAmber)
                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                        Row(verticalAlignment = Alignment.Bottom) {
                            Text(c.title, Modifier.weight(1f), fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            Text(listTime(c.time), fontSize = 12.sp, color = muted)
                        }
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Text(if (c.last.isEmpty()) "No messages yet" else (if (c.mine) "You: " else "") + c.last,
                                Modifier.weight(1f), fontSize = 14.sp, color = muted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            if (c.unread > 0) Box(
                                Modifier.heightIn(min = 20.dp).widthIn(min = 20.dp).clip(RoundedCornerShape(10.dp)).background(McAmber).padding(horizontal = 6.dp),
                                contentAlignment = Alignment.Center,
                            ) { Text("${c.unread}", color = McAmberText, fontSize = 12.sp, fontWeight = FontWeight.Bold) }
                        }
                    }
                }
            }
        }
        Box(
            Modifier.align(Alignment.BottomEnd).padding(20.dp).size(56.dp).clip(RoundedCornerShape(16.dp)).background(McAmber).clickable { adding = true },
            contentAlignment = Alignment.Center,
        ) { Icon(Icons.Default.Edit, "New chat", tint = McAmberText) }
    }
    if (adding) NewMeshCoreChat { adding = false }
}

@Composable
private fun NewMeshCoreChat(onDone: () -> Unit) {
    var channel by remember { mutableStateOf("") }
    var key by remember { mutableStateOf("") }
    var error by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDone,
        title = { Text("New MeshCore chat") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(channel, { channel = it.lowercase().removePrefix("#") }, singleLine = true,
                    label = { Text("Join a channel, e.g. hike") })
                OutlinedTextField(key, { key = it.trim().lowercase() }, singleLine = true,
                    label = { Text("or a node's public key (64 characters)") })
                Text("You can also start a chat from Nodes, where you see who is nearby.", fontSize = 12.sp, color = muted)
                if (error.isNotEmpty()) Text(error, color = MaterialTheme.colorScheme.error, fontSize = 12.sp)
            }
        },
        confirmButton = {
            TextButton(onClick = {
                when {
                    Regex("[a-z0-9-]{1,24}").matches(channel) -> { Node.mcAsk("channel-join:$channel"); Node.mcAsk("overview"); onDone() }
                    Regex("[0-9a-f]{64}").matches(key) -> { McNav.open = Node.McConversation("direct", key, key.take(8) + "…", "", false, 0, 0); onDone() }
                    else -> error = "Enter a channel name (a-z, 0-9, -) or a 64-character public key."
                }
            }) { Text("Continue", color = McAmber) }
        },
        dismissButton = { TextButton(onClick = onDone) { Text("Cancel") } },
    )
}

// ------------------------------------------------------------------ Conversation

private fun deliveryLine(c: Node.McConversation, m: Node.McMessage): String {
    val time = clock(m.ts)
    if (!m.mine) return time
    val state = when {
        c.kind == "direct" -> if (m.delivered) "Delivered" else "Sent"
        m.repeats > 0 -> "Heard by ${m.repeats} repeater${if (m.repeats == 1) "" else "s"}" +
            (if (m.snr != 0.0) " · best SNR ${if (m.snr > 0) "+" else ""}${Math.round(m.snr)}" else "")
        m.repeats == 0 -> "Sent · not heard by a repeater yet"
        else -> "Sent"
    }
    return "$time · $state"
}

@Composable
private fun McConversationScreen(c: Node.McConversation, mc: Node.MeshCoreAccess) {
    BackHandler { McNav.open = null; Node.mcAsk("overview") }
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
    val state = rememberLazyListState()
    LaunchedEffect(items.size) { if (items.isNotEmpty()) state.scrollToItem(items.size) }
    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surface).padding(start = 8.dp, end = 12.dp, top = 8.dp, bottom = 10.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            IconButton(onClick = { McNav.open = null; Node.mcAsk("overview") }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            if (c.kind == "direct") Initial(c.title.firstOrNull { it.isLetterOrDigit() }?.uppercase() ?: "?", McPerson, McLight, 40)
            else Initial("#", McAmberDark, McAmber, 40)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(c.title, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(
                    when (c.kind) {
                        "public" -> "MeshCore · public channel"
                        "channel" -> "MeshCore · channel"
                        else -> "MeshCore" + (node?.let { " · ${hops(it.hops)} · ${snrText(it)}" } ?: " · direct message")
                    }, fontSize = 12.sp, color = McAmber,
                )
            }
            if (node != null) IconButton(onClick = { Node.mcAsk("favorite:${node.key}:${if (node.favorite) 0 else 1}"); Node.mcAsk("nodes") }) {
                Icon(if (node.favorite) Icons.Default.Star else Icons.Default.StarBorder, "Favourite", tint = if (node.favorite) McAmber else muted)
            }
        }
        LazyColumn(
            Modifier.weight(1f).fillMaxWidth(), state = state, contentPadding = PaddingValues(16.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            if (items.isEmpty()) item { Text("No messages yet. Say hello.", color = muted) }
            items(items.size) { i ->
                val m = items[i]
                val newDay = i == 0 || dayName(items[i - 1].ts) != dayName(m.ts)
                Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    if (newDay && m.ts > 0) Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
                        Text(dayName(m.ts), fontSize = 12.sp, color = muted,
                            modifier = Modifier.clip(RoundedCornerShape(10.dp)).background(MaterialTheme.colorScheme.surfaceVariant).padding(horizontal = 10.dp, vertical = 4.dp))
                    }
                    Column(Modifier.fillMaxWidth(), horizontalAlignment = if (m.mine) Alignment.End else Alignment.Start, verticalArrangement = Arrangement.spacedBy(3.dp)) {
                        Box(
                            Modifier.widthIn(max = 300.dp).clip(RoundedCornerShape(16.dp)).background(if (m.mine) McAmberDark else McPerson)
                                .padding(horizontal = 14.dp, vertical = 10.dp),
                        ) { RawText(m.text, color = if (m.mine) McAmberBubbleText else McLight, fontSize = 15.sp) }
                        Text(deliveryLine(c, m), fontSize = 11.sp, color = if (m.mine && (m.repeats > 0 || m.delivered)) McAmber else muted,
                            modifier = Modifier.padding(horizontal = 6.dp))
                    }
                }
            }
        }
        Row(
            Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surface).padding(start = 12.dp, end = 12.dp, top = 8.dp, bottom = 12.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.weight(1f).height(48.dp).clip(RoundedCornerShape(24.dp)).background(McPerson).padding(horizontal = 18.dp), contentAlignment = Alignment.CenterStart) {
                androidx.compose.foundation.text.BasicTextField(
                    text, { if (it.toByteArray().size <= max) text = it }, Modifier.fillMaxWidth(), singleLine = true,
                    textStyle = UiTextStyle(color = McLight, fontSize = 15.sp),
                    cursorBrush = SolidColor(McAmber),
                    decorationBox = { inner -> if (text.isEmpty()) Text("Write a message", color = muted, fontSize = 15.sp); inner() },
                )
            }
            Text("${text.toByteArray().size}/$max", fontSize = 11.sp, color = muted, textAlign = TextAlign.End, modifier = Modifier.widthIn(min = 40.dp))
            Box(
                Modifier.size(48.dp).clip(CircleShape).background(if (text.isBlank()) McAmberDark else McAmber).clickable(enabled = text.isNotBlank()) {
                    Node.mcAsk(payload(c, "send", text.trim().toByteArray().hex())); Node.mcAsk(historyPayload(c)); Node.mcAsk("overview")
                    text = ""
                },
                contentAlignment = Alignment.Center,
            ) { Icon(Icons.AutoMirrored.Filled.Send, "Send", tint = McAmberText) }
        }
    }
}

// ------------------------------------------------------------------ Nodes tab

/** The Må:rse | MeshCore switch on the Nodes tab. */
@Composable
fun McNodeChips() {
    val link by Node.mcLink.collectAsState()
    McChips(link, !McNav.nodeMode, { McNav.nodeMode = false }, McNav.nodeMode, { McNav.nodeMode = true }, "MeshCore")
}

/** Shared by Chats and Nodes. MeshCore is grey while it cannot be reached, with the reason below. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun McChips(link: Node.McLink, marseOn: Boolean, onMarse: () -> Unit, mcOn: Boolean, onMeshcore: () -> Unit, label: String) {
    val ready = link.state == 2
    Column {
        Row(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChip(selected = marseOn, onClick = onMarse, label = { Text("Må:rse") })
            FilterChip(
                selected = mcOn && ready, onClick = { if (ready) onMeshcore() }, enabled = ready, label = { Text(label) },
                colors = FilterChipDefaults.filterChipColors(selectedContainerColor = McAmber, selectedLabelColor = McAmberText),
            )
        }
        if (!ready && link.state != 0) Text(
            if (link.state == 1) "Connecting to MeshCore…" else "MeshCore is not available. ${link.note} Trying again…",
            Modifier.padding(horizontal = 20.dp, vertical = 2.dp), fontSize = 12.sp, color = if (link.state == 1) muted else McAmber,
        )
    }
}

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
        Row(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("Heard by your MeshCore radio", Modifier.weight(1f), fontSize = 13.sp, color = muted)
            var advertMenu by remember { mutableStateOf(false) }
            OutlinedButton(
                onClick = { advertMenu = true }, shape = RoundedCornerShape(18.dp),
                border = androidx.compose.foundation.BorderStroke(1.dp, McAmber), contentPadding = PaddingValues(horizontal = 14.dp),
            ) { Text("Advert", color = McAmber, fontWeight = FontWeight.SemiBold, fontSize = 13.sp) }
            if (advertMenu) AlertDialog(
                onDismissRequest = { advertMenu = false },
                title = { Text("Send an advert") },
                text = { Text("An advert tells other nodes that you exist. A zero-hop advert is heard only by nodes in direct radio range. A flood advert is passed on by repeaters, so far more nodes hear it, but it uses more airtime.") },
                confirmButton = {
                    Column(horizontalAlignment = Alignment.End) {
                        TextButton(onClick = { advertMenu = false; Node.mcAsk("announce-flood"); McNav.advertNote = "Flood advert sent. Repeaters pass it on." }) { Text("Flood advert", color = McAmber) }
                        TextButton(onClick = { advertMenu = false; Node.mcAsk("announce"); McNav.advertNote = "Zero-hop advert sent. Nodes in direct range hear it." }) { Text("Zero-hop advert", color = McAmber) }
                    }
                },
                dismissButton = { TextButton(onClick = { advertMenu = false }) { Text("Cancel") } },
            )
            Box(Modifier.size(8.dp))
            OutlinedButton(
                onClick = { McNav.scanAskedAt = System.currentTimeMillis(); Node.mcAsk("scan"); Node.mcAsk("nodes") },
                enabled = !mc.scan.waiting && !mc.scan.active, shape = RoundedCornerShape(18.dp),
                border = androidx.compose.foundation.BorderStroke(1.dp, McAmber), contentPadding = PaddingValues(horizontal = 14.dp),
            ) { Text("Scan now", color = McAmber, fontWeight = FontWeight.SemiBold, fontSize = 13.sp) }
        }
        if (McNav.advertNote.isNotEmpty()) Text(McNav.advertNote, Modifier.padding(horizontal = 20.dp, vertical = 2.dp), fontSize = 13.sp, color = McGreen)
        ScanStatus(mc)
        Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 20.dp, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            for ((id, label) in listOf("all" to "All", "fav" to "Favourites", "2" to "Repeaters", "1" to "Chat", "3" to "Rooms", "4" to "Sensors")) {
                val on = McNav.filter == id
                Box(
                    Modifier.height(30.dp).clip(RoundedCornerShape(15.dp)).background(if (on) McAmberDark else Color.Transparent)
                        .border(1.dp, if (on) McAmber else MaterialTheme.colorScheme.outlineVariant, RoundedCornerShape(15.dp))
                        .clickable { McNav.filter = id }.padding(horizontal = 12.dp),
                    contentAlignment = Alignment.Center,
                ) { Text(label, fontSize = 13.sp, fontWeight = FontWeight.Medium, color = if (on) McAmber else muted) }
            }
        }
        if (shown.isEmpty()) Text(if (mc.nodes.isEmpty()) "No nodes heard yet. Choose Scan now, or wait for adverts." else "Nothing in this filter.",
            Modifier.padding(24.dp), color = muted)
        LazyColumn {
            val favourites = shown.filter { it.favorite }
            if (favourites.isNotEmpty()) item { SectionTitle("FAVOURITES", "") }
            items(favourites, key = { "f" + it.key }) { McNodeRow(it) }
            val others = shown.filter { !it.favorite }
            if (others.isNotEmpty()) item { SectionTitle("HEARD NEARBY", "${others.size} node${if (others.size == 1) "" else "s"}") }
            items(others, key = { it.key }) { McNodeRow(it) }
        }
    }
}

@Composable
private fun SectionTitle(title: String, right: String) {
    Row(Modifier.fillMaxWidth().padding(start = 20.dp, end = 20.dp, top = 14.dp, bottom = 6.dp)) {
        Text(title, Modifier.weight(1f), fontSize = 12.sp, fontWeight = FontWeight.SemiBold, letterSpacing = 0.7.sp, color = muted)
        if (right.isNotEmpty()) Text(right, fontSize = 12.sp, color = muted)
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
    Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text(text, fontSize = 13.sp, color = if (running) McAmber else McGreen)
        if (running) LinearProgressIndicator(Modifier.fillMaxWidth(), color = McAmber, trackColor = McAmberDark)
        if (answered.isNotEmpty()) Text(
            answered.take(4).joinToString(" · ") { it.name.ifEmpty { it.key.take(8) + "…" } } + if (answered.size > 4) " · +${answered.size - 4} more" else "",
            fontSize = 12.sp, color = muted, maxLines = 2, overflow = TextOverflow.Ellipsis,
        )
    }
}

@Composable
private fun McNodeRow(n: Node.McNode) {
    val name = n.name.ifEmpty { n.key.take(8) + "…" }
    Row(
        Modifier.fillMaxWidth().clickable { McNav.node = n.key }.padding(horizontal = 20.dp, vertical = 9.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        NodeIcon(n.type)
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Text(name, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text("${mcTypes.getOrElse(n.type) { "Node" }} · ${hops(n.hops)}", fontSize = 13.sp, color = muted)
        }
        Column(horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(3.dp)) {
            if (n.seen > 0) Text(snrText(n), fontSize = 12.sp)
            Text(ago(n.seen), fontSize = 11.sp, color = muted)
        }
        Icon(
            if (n.favorite) Icons.Default.Star else Icons.Default.StarBorder, if (n.favorite) "Favourite" else "Add to favourites",
            tint = if (n.favorite) McAmber else muted,
            modifier = Modifier.size(24.dp).clickable { Node.mcAsk("favorite:${n.key}:${if (n.favorite) 0 else 1}"); Node.mcAsk("nodes") },
        )
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
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClick = { McNav.node = null }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            Text("Node", Modifier.weight(1f), fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
            IconButton(onClick = { Node.mcAsk("favorite:${n.key}:${if (n.favorite) 0 else 1}"); Node.mcAsk("nodes") }) {
                Icon(if (n.favorite) Icons.Default.Star else Icons.Default.StarBorder, if (n.favorite) "Remove from favourites" else "Add to favourites",
                    tint = if (n.favorite) McAmber else muted)
            }
        }
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()).padding(horizontal = 20.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                val l = look(n.type)
                if (admin || n.type == 4) NodeIcon(n.type, 72)
                else Initial(name.firstOrNull { it.isLetterOrDigit() }?.uppercase() ?: "?", l.bg, l.fg, 72)
                Text(name, fontSize = 20.sp, fontWeight = FontWeight.SemiBold)
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    Text("MeshCore ${mcTypes.getOrElse(n.type) { "Node" }.lowercase()}${if (n.type == 1) " node" else ""}", fontSize = 12.sp, fontWeight = FontWeight.SemiBold, color = McAmber,
                        modifier = Modifier.clip(RoundedCornerShape(10.dp)).background(McAmberDark).padding(horizontal = 9.dp, vertical = 3.dp))
                    Text("${n.key.take(4)}…${n.key.takeLast(4)}", fontSize = 12.sp, fontFamily = FontFamily.Monospace, color = muted)
                }
            }
            val facts = listOf(
                "Last heard" to ago(n.seen), "Hops" to n.hops.toString(),
                "SNR / RSSI" to if (n.seen > 0) "${if (n.snr > 0) "+" else ""}${Math.round(n.snr)} dB / ${Math.round(n.rssi)}" else "-",
                "Times heard" to n.count.toString(),
            )
            for (pair in facts.chunked(2)) Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                for ((label, value) in pair) Column(
                    Modifier.weight(1f).clip(RoundedCornerShape(12.dp)).background(MaterialTheme.colorScheme.surfaceVariant).padding(horizontal = 14.dp, vertical = 10.dp),
                    verticalArrangement = Arrangement.spacedBy(2.dp),
                ) { Text(label, fontSize = 12.sp, color = muted); Text(value, fontSize = 15.sp, fontWeight = FontWeight.SemiBold) }
            }
            if (admin) Text("Administration needs the repeater's own password. Log in to see its status and run commands.", fontSize = 13.sp, color = muted)
        }
        Column(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 14.dp)) {
            Button(
                onClick = {
                    if (admin) sheet = true else {
                        McNav.node = null
                        McNav.open = Node.McConversation("direct", n.key, name, "", false, 0, 0)
                        McNav.nodeMode = false
                        MeshCoreMode.on = true
                        onChat()   // show the Chats tab, where the conversation is
                    }
                }, Modifier.fillMaxWidth().height(48.dp), shape = RoundedCornerShape(24.dp),
                colors = ButtonDefaults.buttonColors(containerColor = McAmber, contentColor = McAmberText),
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
                NodeIcon(n.type, 40)
                Column {
                    Text("Log in to $name", fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                    Text("Sent to the repeater over the radio", fontSize = 12.sp, color = muted)
                }
            }
            OutlinedTextField(
                password, { password = it }, Modifier.fillMaxWidth(), singleLine = true, label = { Text("Repeater password") },
                visualTransformation = PasswordVisualTransformation(), keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
            )
            Text("The password is not saved. You type it each time you log in, and it is only used for this session.", fontSize = 12.sp, color = muted)
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                OutlinedButton(onClick = onDone, Modifier.weight(1f)) { Text("Cancel") }
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
                    }, Modifier.weight(1f), enabled = password.isNotEmpty(),
                    colors = ButtonDefaults.buttonColors(containerColor = McAmber, contentColor = McAmberText),
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
            Text("Could not log in to $name", fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
            Text(McNav.loginNote, color = muted)
            Button(onClick = { McNav.loggedIn = null; McNav.loginKind = 0 }, colors = ButtonDefaults.buttonColors(containerColor = McAmber, contentColor = McAmberText)) { Text("Try again") }
        }
        else -> Column(Modifier.fillMaxSize().padding(24.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            Text("Logging in to $name…", fontSize = 18.sp, fontWeight = FontWeight.SemiBold)
            Text("The password is sent over the radio. This takes a few seconds.", color = muted)
            LinearProgressIndicator(Modifier.fillMaxWidth(), color = McAmber, trackColor = McAmberDark)
        }
    }
}

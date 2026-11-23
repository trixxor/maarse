package dk.maarse.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.Done
import androidx.compose.material.icons.filled.DoneAll
import androidx.compose.material.icons.filled.MailOutline
import androidx.compose.material.icons.filled.Schedule
import androidx.compose.material.icons.outlined.ErrorOutline
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dk.maarse.app.Conn
import dk.maarse.app.Mode
import dk.maarse.app.Node

// ---------- avatars ----------

private val AVATAR_LIGHT = listOf(0xFF3B6EA8, 0xFF6B5B95, 0xFFB5651D, 0xFF2E7D8C, 0xFF556B2F, 0xFF9C4A6E, 0xFF2F7D5B, 0xFF7A5C2E)
private val AVATAR_DARK = listOf(0xFF5B8CC6, 0xFF8A7AB5, 0xFFC98447, 0xFF4B9DAC, 0xFF7A8F4F, 0xFFBA6A8E, 0xFF4F9D7B, 0xFF9C7C4C)

/** The avatar colour for a conversation: the same everywhere for one address. */
@Composable
fun avatarColor(key: String): Color {
    val list = if (isSystemInDarkTheme()) AVATAR_DARK else AVATAR_LIGHT
    return Color(list[(key.hashCode() and 0x7fffffff) % list.size])
}

fun initials(title: String): String {
    val t = title.trim().removePrefix("~")
    if (t.startsWith("#")) return "#"
    val words = t.split(' ', '·', '-').filter { it.isNotBlank() }
    return when {
        words.isEmpty() -> "?"
        words.size == 1 -> words[0].take(1).uppercase()
        else -> (words[0].take(1) + words[1].take(1)).uppercase()
    }
}

@Composable
fun Avatar(title: String, key: String, size: Dp = 48.dp) {
    val channel = title.trim().startsWith("#")
    Box(
        Modifier.size(size).clip(CircleShape).background(if (channel) MaterialTheme.colorScheme.primary else avatarColor(key)),
        contentAlignment = Alignment.Center,
    ) {
        Text(initials(title), color = if (channel) MaterialTheme.colorScheme.onPrimary else Color.White, fontWeight = FontWeight.SemiBold, fontSize = (size.value * 0.38f).sp)
    }
}

// ---------- the connection chip ----------

fun modeName(m: Mode) = when (m) {
    Mode.PHONE -> "Phone node"
    Mode.BLE -> "Radio over Bluetooth"
    Mode.REMOTE -> "Home computer"
    Mode.SERVER -> "Server on this phone"
}

/**
 * Shown only when something is wrong: not connected, connecting, or no way
 * out (airplane mode, no internet, no radio). Everything fine: nothing.
 */
@Composable
fun ConnectionBanner(onClick: () -> Unit) {
    val c by Node.conn.collectAsState()
    val off by Node.offline.collectAsState()
    val (title, text) = when {
        c.state == Conn.State.OFF -> "Not connected" to "Tap to choose how this phone connects."
        c.state == Conn.State.CONNECTING -> "Connecting…" to c.text
        off -> "No connection" to Node.offlineWhy.collectAsState().value
        else -> return
    }
    Row(
        Modifier.padding(horizontal = 16.dp, vertical = 4.dp).fillMaxWidth().clip(RoundedCornerShape(14.dp))
            .background(MaterialTheme.colorScheme.tertiaryContainer).clickable(onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(10.dp).clip(CircleShape).background(Amber))
        Spacer(Modifier.width(10.dp))
        Column {
            Text(title, fontWeight = FontWeight.SemiBold, color = MaterialTheme.colorScheme.onTertiaryContainer)
            Text(text, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onTertiaryContainer)
        }
    }
}

/** How we are connected, in a few words (the You page). */
@Composable
fun connectionSummary(): String {
    val c by Node.conn.collectAsState()
    val off by Node.offline.collectAsState()
    val stats by Node.stats.collectAsState()
    return buildString {
        append(modeName(c.mode))
        append(when { c.state == Conn.State.OFF -> " · not connected"; c.state == Conn.State.CONNECTING -> " · connecting"; off -> " · no connection"; else -> " · online" })
        if (c.state == Conn.State.ONLINE && c.mode == Mode.PHONE) stats[5]?.let { append(" · $it internet link${if (it == 1) "" else "s"}") }
    }
}

// ---------- delivery ----------

/** A small icon for a sent message's state; the words come on tap. */
@Composable
fun DeliveryIcon(status: String, channel: Boolean) {
    val (icon, tint, label) = when {
        status == "delivered" || status == "carried" -> Triple(Icons.Filled.DoneAll, MaterialTheme.colorScheme.primary, "Delivered")
        status == "mailbox" -> Triple(Icons.Filled.MailOutline, Amber, "Waiting in a post office")
        status == "filebox" -> Triple(Icons.Filled.MailOutline, Amber, "Kept by your node until they are reachable")
        status == "sent" -> Triple(Icons.Filled.Done, MaterialTheme.colorScheme.onSurfaceVariant, if (channel) "Sent" else "Sent, waiting for receipt")
        status == "queued" -> Triple(Icons.Filled.Schedule, Amber, "Waiting for a connection")
        status.startsWith("error") || status.startsWith("cancelled") -> Triple(Icons.Outlined.ErrorOutline, MaterialTheme.colorScheme.error, status)
        else -> Triple(Icons.Filled.Schedule, MaterialTheme.colorScheme.onSurfaceVariant, "Sending")
    }
    Icon(icon, label, Modifier.size(15.dp), tint = tint)
}

// ---------- grouped lists (settings pages) ----------

@Composable
fun GroupTitle(t: String) {
    Text(t, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary, fontWeight = FontWeight.SemiBold,
        modifier = Modifier.padding(start = 28.dp, top = 14.dp, bottom = 6.dp))
}

@Composable
fun Group(content: @Composable () -> Unit) {
    Column(
        Modifier.padding(horizontal = 16.dp).fillMaxWidth().clip(RoundedCornerShape(16.dp))
            .background(MaterialTheme.colorScheme.surfaceContainer),
    ) { content() }
}

@Composable
fun GroupRow(label: String, sub: String? = null, last: Boolean = false, leading: (@Composable () -> Unit)? = null, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().heightIn(min = 56.dp).clickable(onClick = onClick).padding(horizontal = 14.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        leading?.invoke()
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(label, style = MaterialTheme.typography.bodyLarge)
            sub?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
        }
        Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, null, tint = MaterialTheme.colorScheme.outline)
    }
    if (!last) HorizontalDivider(Modifier.padding(start = 14.dp), color = MaterialTheme.colorScheme.outlineVariant)
}

/** A rounded card on the page background. */
@Composable
fun Card2(modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    Column(
        modifier.padding(horizontal = 16.dp).fillMaxWidth().clip(RoundedCornerShape(18.dp))
            .background(MaterialTheme.colorScheme.surfaceContainer).padding(16.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) { content() }
}

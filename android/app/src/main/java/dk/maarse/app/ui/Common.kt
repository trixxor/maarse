package dk.maarse.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dk.maarse.app.Conn
import dk.maarse.app.Ev
import dk.maarse.app.Names
import dk.maarse.app.Node
import dk.maarse.app.shortAddr
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

val Green = Color(0xFF2F7D5B)
val Amber = Color(0xFFB7791F)
val Red = Color(0xFFC0392B)

@Composable
fun MaarseTheme(content: @Composable () -> Unit) {
    val dark = isSystemInDarkTheme()
    // The mockup's palette: one green accent, calm grey-green grounds; amber
    // only for warnings and mail waiting. Dark follows the system.
    val scheme = if (dark) darkColorScheme(
        primary = Color(0xFF7FD1A5), onPrimary = Color(0xFF00391F),
        primaryContainer = Color(0xFF1F4D36), onPrimaryContainer = Color(0xFFBFEFD2),
        secondaryContainer = Color(0xFF243B30), onSecondaryContainer = Color(0xFFCDE8D8),
        background = Color(0xFF111412), onBackground = Color(0xFFE2E6E3),
        surface = Color(0xFF111412), onSurface = Color(0xFFE2E6E3),
        surfaceVariant = Color(0xFF1C211E), onSurfaceVariant = Color(0xFFB4BCB7),
        surfaceContainer = Color(0xFF1A1E1C), surfaceContainerHigh = Color(0xFF222825),
        outline = Color(0xFF3A423D), outlineVariant = Color(0xFF2C332F),
        tertiaryContainer = Color(0xFF4A3214), onTertiaryContainer = Color(0xFFFFDDB8),
        error = Color(0xFFFFB4A9),
    ) else lightColorScheme(
        primary = Green, onPrimary = Color.White,
        primaryContainer = Color(0xFFD8EEDF), onPrimaryContainer = Color(0xFF0E3A26),
        secondaryContainer = Color(0xFFE6F1EB), onSecondaryContainer = Color(0xFF1A3A2A),
        background = Color(0xFFF7F8F6), onBackground = Color(0xFF1A1D1B),
        surface = Color(0xFFF7F8F6), onSurface = Color(0xFF1A1D1B),
        surfaceVariant = Color(0xFFFFFFFF), onSurfaceVariant = Color(0xFF5B635E),
        surfaceContainer = Color(0xFFFFFFFF), surfaceContainerHigh = Color(0xFFEEF1EE),
        outline = Color(0xFFC9CFCB), outlineVariant = Color(0xFFE3E7E4),
        tertiaryContainer = Color(0xFFFDF1E6), onTertiaryContainer = Color(0xFF5A2D05),
        error = Red,
    )
    MaterialTheme(colorScheme = scheme, content = content)
}

/** Always-visible line: how we are connected, and what the node is doing. */
@Composable
fun StatusStrip() {
    val c by Node.conn.collectAsState()
    val me by Node.me.collectAsState()
    val stats by Node.stats.collectAsState()
    val color = when (c.state) { Conn.State.ONLINE -> Green; Conn.State.CONNECTING -> Amber; Conn.State.OFF -> Color.Gray }
    Row(
        Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceVariant).padding(horizontal = 12.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Spacer(Modifier.size(10.dp).clip(CircleShape).background(color))
        Spacer(Modifier.width(8.dp))
        Text(c.text, style = MaterialTheme.typography.labelMedium, modifier = Modifier.weight(1f), maxLines = 1)
        val bits = buildList {
            stats[7]?.let { add(Names.ROLES.getOrElse(it) { "?" }) }
            if ((stats[98] ?: 0) != 0) add("mobile")
            stats[96]?.takeIf { it > 0 }?.let { add("carrying $it") }
            stats[61]?.takeIf { it > 0 }?.let { add("✉ $it") }
            stats[85]?.let { add("$it %") }
            if (isEmpty()) me?.let { add(shortAddr(it.addr)) }
        }
        Text(bits.joinToString(" · "), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

fun time(ms: Long): String {
    val d = Date(ms)
    val today = SimpleDateFormat("yyyyMMdd", Locale.ROOT).format(Date()) == SimpleDateFormat("yyyyMMdd", Locale.ROOT).format(d)
    return SimpleDateFormat(if (today) "HH:mm" else "d MMM HH:mm", Locale.getDefault()).format(d)
}

/** Delivery state in words, so nobody has to guess what a tick means. */
fun statusText(s: String, channel: Boolean = false): Pair<String, Color?> = when {
    // A channel message goes to everyone; nobody sends a receipt for it.
    channel && s == "sent" -> "Sent to the channel" to null
    s == "delivered" -> "✓ Delivered" to Green
    s == "carried" -> "✓ Delivered (carried by others)" to Green
    s == "mailbox" -> "✉ Waiting in a post office" to Amber
    s == "sent" -> "Sent, waiting for receipt" to null
    s.startsWith("error") -> s.replaceFirstChar { it.uppercase() } to Red
    else -> s to null
}

@Composable
fun AlertCard(a: Ev.Alert, onHide: (() -> Unit)? = null) {
    val over = Node.alertCancelledOrUpdated(a)
    val ended = over != null || System.currentTimeMillis() >= a.endsMs
    val drill = a.status == 1 || a.status == 2
    val loud = Node.alertLoud(a)
    val bg = when {
        ended -> MaterialTheme.colorScheme.surfaceVariant
        drill -> Color(0xFF3B5B8C)
        loud -> Red
        else -> Amber
    }
    val fg = if (ended) MaterialTheme.colorScheme.onSurfaceVariant else Color.White
    Card(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 4.dp), colors = CardDefaults.cardColors(containerColor = bg, contentColor = fg)) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Row {
                val status = Names.ALERT_STATUS.getOrElse(a.status) { "" }
                if (status.isNotEmpty()) Text("$status · ", fontWeight = FontWeight.Bold)
                Text(
                    if (a.kind == 2) "Cancelled: alert ${a.reference}" else Names.ALERT_EVENTS.getOrElse(a.event) { "Alert" } + if (a.kind == 1) " (update)" else "",
                    fontWeight = FontWeight.Bold, modifier = Modifier.weight(1f),
                )
                Text(Names.ALERT_SEVERITY.getOrElse(a.severity) { "" })
            }
            if (a.text.isNotEmpty()) Text(a.text, style = MaterialTheme.typography.bodyLarge)
            val from = a.name.ifEmpty { "trusted key " + shortAddr(a.issuer) }
            val how = when (a.trust) { 1 -> "verified by certificate"; 2 -> "verified (old format, no expiry)"; else -> "verified by key" }
            var who = "From $from · $how · issued ${time(a.issued * 1000)} · valid until ${time(a.endsMs)} · ${a.hops} hops · no. ${a.serial}"
            a.area?.let { who += " · area %.3f, %.3f (%.1f km)".format(it.first, it.second, it.third) }
            if (over != null) who += if (over.kind == 2) " · CANCELLED" else " · replaced by an update"
            Text(who, style = MaterialTheme.typography.labelSmall)
            if (onHide != null) TextButton(onClick = onHide) { Text("Hide", color = fg) }
        }
    }
}

/** Live alerts on top of every screen until hidden. */
@Composable
fun AlertBar() {
    val list by Node.alerts.collectAsState()
    val dismissed by Node.dismissed.collectAsState()
    val live = list.filter { Node.alertLoud(it) && it.key !in dismissed }.takeLast(3).reversed()
    Column { live.forEach { a -> AlertCard(a) { Node.dismissAlert(a) } } }
}

@Composable
fun SectionTitle(t: String) {
    Text(t, style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary, modifier = Modifier.padding(start = 16.dp, top = 16.dp, bottom = 4.dp))
}

@Composable
fun Hint(t: String) {
    Text(t, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(horizontal = 16.dp, vertical = 2.dp))
}

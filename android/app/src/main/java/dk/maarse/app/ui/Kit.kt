package dk.maarse.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.FilterChip
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dk.maarse.app.tr

/**
 * Shared building blocks of the "Mesh native" design (redesign C).
 * MeshCore is only ever a marker: a small tag, a section title, a badge. It never recolours a screen.
 * Amber means "waiting" and red means "act"; MeshCore uses its own violet so the meanings never collide.
 */
object Mc {
    private val light = Color(0xFF5B4FC4)
    private val dark = Color(0xFFB3A8FF)
    private val containerLight = Color(0xFFEEEBFB)
    private val containerDark = Color(0xFF2A2650)
    private val onContainerLight = Color(0xFF1F1A5C)
    private val onContainerDark = Color(0xFFE4E0FF)
    val color: Color @Composable get() = if (isSystemInDarkTheme()) dark else light
    val container: Color @Composable get() = if (isSystemInDarkTheme()) containerDark else containerLight
    val onContainer: Color @Composable get() = if (isSystemInDarkTheme()) onContainerDark else onContainerLight
    /** Text on a solid MeshCore badge. */
    val onColor: Color @Composable get() = if (isSystemInDarkTheme()) Color(0xFF1C1650) else Color.White
}

/** The small "MeshCore" marker next to a row, a node or a section title. */
@Composable
fun NetworkTag(label: String = "MeshCore", modifier: Modifier = Modifier) {
    Box(modifier.clip(RoundedCornerShape(6.dp)).background(Mc.container).padding(horizontal = 7.dp, vertical = 1.dp)) {
        Text(tr(label).uppercase(), color = Mc.color, fontSize = 10.5.sp, fontWeight = FontWeight.Bold, letterSpacing = 0.5.sp, maxLines = 1)
    }
}

/** An unread counter. `meshcore` uses the MeshCore marker colour instead of the primary green. */
@Composable
fun CountBadge(n: Int, meshcore: Boolean = false) {
    val bg = if (meshcore) Mc.color else MaterialTheme.colorScheme.primary
    val fg = if (meshcore) Mc.onColor else MaterialTheme.colorScheme.onPrimary
    Box(Modifier.clip(CircleShape).background(bg).padding(horizontal = 7.dp, vertical = 2.dp)) {
        Text("$n", color = fg, style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.SemiBold)
    }
}

/** A section title in a list; `meshcore` adds the MeshCore tag after it. */
@Composable
fun SectionLabel(text: String, meshcore: Boolean = false, modifier: Modifier = Modifier) {
    Row(modifier.padding(start = 16.dp, end = 16.dp, top = 14.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(text, style = MaterialTheme.typography.labelLarge, color = if (meshcore) Mc.color else MaterialTheme.colorScheme.primary, fontWeight = FontWeight.SemiBold)
        if (meshcore) Text("separate radio network", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

/** Four signal bars from a signal-to-noise value in dB (null = unknown, drawn empty). */
@Composable
fun SignalBars(snr: Int?, modifier: Modifier = Modifier) {
    val on = when { snr == null -> 0; snr >= 8 -> 4; snr >= 4 -> 3; snr >= 0 -> 2; snr > -12 -> 1; else -> 0 }
    Row(modifier.height(14.dp), horizontalArrangement = Arrangement.spacedBy(2.dp), verticalAlignment = Alignment.Bottom) {
        listOf(5, 8, 11, 14).forEachIndexed { i, h ->
            Box(Modifier.width(4.dp).height(h.dp).clip(RoundedCornerShape(1.dp)).background(if (i < on) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outlineVariant))
        }
    }
}

/** A segmented control with equal parts, e.g. Nodes | Sensors | Pages. */
@Composable
fun SegmentedTabs(items: List<String>, selected: Int, onSelect: (Int) -> Unit, modifier: Modifier = Modifier) {
    Row(modifier.padding(horizontal = 16.dp, vertical = 4.dp).fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(MaterialTheme.colorScheme.surfaceContainerHigh).padding(3.dp)) {
        items.forEachIndexed { i, label ->
            val on = i == selected
            Box(
                Modifier.weight(1f).clip(RoundedCornerShape(10.dp)).background(if (on) MaterialTheme.colorScheme.surfaceContainer else Color.Transparent)
                    .clickable { onSelect(i) }.padding(vertical = 8.dp),
                contentAlignment = Alignment.Center,
            ) { Text(label, fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal, color = if (on) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurfaceVariant) }
        }
    }
}

/** The small status pill in a header: a dot, a few words, optionally tappable. */
@Composable
fun StatusPill(text: String, dot: Color, onClick: (() -> Unit)? = null, modifier: Modifier = Modifier) {
    Row(
        modifier.clip(RoundedCornerShape(99.dp)).background(MaterialTheme.colorScheme.secondaryContainer)
            .then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier).padding(horizontal = 11.dp, vertical = 5.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Box(Modifier.size(9.dp).clip(CircleShape).background(dot))
        Text(text, style = MaterialTheme.typography.labelMedium, fontWeight = FontWeight.SemiBold, color = MaterialTheme.colorScheme.onSecondaryContainer, maxLines = 1)
    }
}

/** Shows a person without a name as "Unnamed · 4230" instead of the 8 hex characters. */
fun friendlyName(title: String): String =
    if (Regex("[0-9a-f]{8}").matches(title)) tr("Unnamed") + " · " + title.take(4) else title

/** True for a person shown by their 8 hex characters only (no name, no nickname). */
fun isUnnamed(title: String): Boolean = friendlyName(title) != title || title.startsWith(tr("Unnamed") + " · ")

/** The text an avatar draws its letters from: unnamed people get "?", not the digits of their address. */
fun avatarTitle(title: String): String = if (isUnnamed(title)) "?" else title

/** A row of filter chips that scrolls sideways when it does not fit. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ChipRow(items: List<String>, selected: String, onSelect: (String) -> Unit, modifier: Modifier = Modifier) {
    Row(modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        items.forEach { label -> FilterChip(selected = label == selected, onClick = { onSelect(label) }, label = { Text(label, maxLines = 1) }) }
    }
}

/**
 * The frame of one chat bubble, shared by Må:rse and MeshCore chats: same shape, same colours, own messages on the
 * right in the green container. The caller draws what is inside.
 */
@Composable
@OptIn(ExperimentalFoundationApi::class)
fun BubbleFrame(mine: Boolean, modifier: Modifier = Modifier, onClick: (() -> Unit)? = null, onLong: (() -> Unit)? = null, content: @Composable ColumnScope.() -> Unit) {
    Row(Modifier.fillMaxWidth().padding(vertical = 2.dp), horizontalArrangement = if (mine) Arrangement.End else Arrangement.Start) {
        Column(
            modifier.widthIn(max = 300.dp)
                .clip(RoundedCornerShape(topStart = 18.dp, topEnd = 18.dp, bottomStart = if (mine) 18.dp else 6.dp, bottomEnd = if (mine) 6.dp else 18.dp))
                .background(if (mine) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surfaceContainer)
                .then(if (onClick != null || onLong != null) Modifier.combinedClickable(onClick = { onClick?.invoke() }, onLongClick = onLong) else Modifier)
                .padding(start = 12.dp, end = 10.dp, top = 7.dp, bottom = 5.dp),
            content = content,
        )
    }
}

/** A small rounded chip under a bubble or in a list, e.g. a route ("2 hops"). */
@Composable
fun MiniChip(text: String, modifier: Modifier = Modifier, color: Color = MaterialTheme.colorScheme.onSurfaceVariant) {
    Text(text, modifier.clip(RoundedCornerShape(8.dp)).background(MaterialTheme.colorScheme.surfaceContainerHigh).padding(horizontal = 7.dp, vertical = 2.dp),
        style = MaterialTheme.typography.labelSmall, color = color, maxLines = 1)
}

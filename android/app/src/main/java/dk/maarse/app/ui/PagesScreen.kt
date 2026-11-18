package dk.maarse.app.ui

import android.graphics.BitmapFactory
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.unit.dp
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.ui.focus.focusRequester
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.automirrored.filled.ArrowForward
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import dk.maarse.app.Node
import dk.maarse.app.Pages

/** A star that adds a user ('u') or a node's pages ('p') to the favourites. */
@Composable
fun FavStar(kind: Char, addr: String) {
    Pages.version.collectAsState().value
    val on = Pages.isFav(kind, addr)
    androidx.compose.material3.IconButton(onClick = { Pages.toggleFav(kind, addr) }) {
        Text(if (on) "★" else "☆", style = MaterialTheme.typography.titleLarge,
            color = if (on) Color(0xFFE7A94B) else MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.semantics { contentDescription = if (on) "Remove from favourites" else "Add to favourites" })
    }
}

/** maarse:// pages, like a small web browser: a toolbar (back, forward,
 *  reload, an address pill with the page's title, bookmark star, home), a
 *  loading bar, the page, and a status bar at the bottom. With no page open
 *  it shows a start page: bookmarks, nodes sharing pages, recently visited. */
@Composable
fun PagesPage(onExit: () -> Unit = {}) {
    // Read here, so every change in Pages redraws this screen.
    @Suppress("UNUSED_VARIABLE") val v = Pages.version.collectAsState().value
    val cur = Pages.url
    androidx.activity.compose.BackHandler(Pages.history.isNotEmpty() || cur != null) {
        if (Pages.history.isNotEmpty()) Pages.back() else Pages.home()
    }
    Column(Modifier.fillMaxSize()) {
        BrowserBar(cur, onExit, v)
        if (Pages.loading) androidx.compose.material3.LinearProgressIndicator(Modifier.fillMaxWidth().height(3.dp))
        else androidx.compose.foundation.layout.Spacer(Modifier.height(3.dp))
        androidx.compose.material3.HorizontalDivider()
        Box(Modifier.weight(1f).fillMaxWidth()) {
            val markup = cur?.let { Pages.markup(it) }
            when {
                cur == null -> StartPage(v)
                markup == null -> Column(Modifier.fillMaxSize().padding(24.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(if (Pages.loading) "Loading the page…" else Pages.status.ifEmpty { "This page is not here yet." },
                        style = MaterialTheme.typography.bodyLarge)
                    if (!Pages.loading) OutlinedButton(onClick = { Pages.reload() }) { Text("Try again") }
                }
                else -> {
                  val scroll = rememberScrollState()
                  Column(Modifier.fillMaxSize().verticalScroll(scroll).padding(horizontal = 14.dp, vertical = 12.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (Pages.status.isNotEmpty()) Text(Pages.status, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
                    MuPage(markup, cur, v, scroll)
                    androidx.compose.foundation.layout.Spacer(Modifier.height(24.dp))
                  }
                }
            }
        }
        StatusBar(cur, v)
    }
}

/** The toolbar: back, forward, reload; the address pill; bookmark; home. */
@Composable
// `version` is passed so the bar redraws when Pages changes.
private fun BrowserBar(cur: Pages.Url?, onExit: () -> Unit, @Suppress("UNUSED_PARAMETER") version: Int) {
    // Read here too: a child given an unchanged-looking version may be skipped.
    Pages.version.collectAsState().value
    var editing by remember(cur) { mutableStateOf(cur == null) }
    val full = cur?.toString() ?: ""
    var input by remember(cur) { mutableStateOf(androidx.compose.ui.text.input.TextFieldValue(full, androidx.compose.ui.text.TextRange(0, full.length))) }
    val focus = remember { androidx.compose.ui.focus.FocusRequester() }
    val title = cur?.let { u -> Pages.markup(u)?.let { Pages.title(it) } }?.ifEmpty { null }
    fun go() { if (input.text.isNotBlank()) { Pages.open(input.text); editing = false } }
    Row(Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceContainer).padding(horizontal = 2.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically) {
        if (editing && cur != null) {
            // Typing an address: the field takes the whole bar, like a browser.
            androidx.compose.material3.IconButton(onClick = { editing = false }) {
                androidx.compose.material3.Icon(androidx.compose.material.icons.Icons.Default.Close, "Cancel")
            }
        } else {
            androidx.compose.material3.IconButton(onClick = { if (Pages.history.isNotEmpty()) Pages.back() else onExit() }) {
                androidx.compose.material3.Icon(androidx.compose.material.icons.Icons.AutoMirrored.Filled.ArrowBack, "Back")
            }
            if (!editing) androidx.compose.material3.IconButton(onClick = { Pages.forward() }, enabled = Pages.forwardStack.isNotEmpty()) {
                androidx.compose.material3.Icon(androidx.compose.material.icons.Icons.AutoMirrored.Filled.ArrowForward, "Forward")
            }
        }
        // The address pill: the page's title and node when shown; tap to type.
        Row(Modifier.weight(1f).clip(RoundedCornerShape(24.dp)).background(MaterialTheme.colorScheme.surface)
            .clickable { editing = true }.padding(horizontal = 12.dp, vertical = if (editing) 0.dp else 8.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            if (cur != null && !editing) RouteBadge(Pages.overLora(cur.addr))
            // A locked page: how many may read it.
            val lk = cur?.let { Pages.markup(it) }?.let { muSettings(it).lock } ?: 0
            if (cur != null && !editing && lk > 0) Text("🔒 $lk", style = MaterialTheme.typography.labelMedium, color = Color(0xFFFFCF6B),
                modifier = Modifier.background(Color(0xFF3A2A10), RoundedCornerShape(9.dp)).padding(horizontal = 7.dp, vertical = 2.dp))
            if (editing) {
                androidx.compose.foundation.text.BasicTextField(
                    input, { input = it }, Modifier.weight(1f).padding(vertical = 12.dp).focusRequester(focus), singleLine = true,
                    textStyle = MaterialTheme.typography.bodyMedium.copy(color = MaterialTheme.colorScheme.onSurface, fontFamily = FontFamily.Monospace),
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Go, autoCorrectEnabled = false,
                        keyboardType = androidx.compose.ui.text.input.KeyboardType.Uri),
                    keyboardActions = KeyboardActions(onGo = { go() }),
                    decorationBox = { inner -> Box { if (input.text.isEmpty()) Text("maarse://address/path", color = MaterialTheme.colorScheme.onSurfaceVariant, style = MaterialTheme.typography.bodyMedium); inner() } },
                )
                if (input.text.isNotEmpty()) Text("✕", Modifier.clip(RoundedCornerShape(12.dp)).clickable { input = androidx.compose.ui.text.input.TextFieldValue("") }.padding(6.dp),
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
                // Focus the field (keyboard up, whole address selected) when typing starts.
                androidx.compose.runtime.LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
            } else Column(Modifier.weight(1f)) {
                Text(title ?: full, maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
                    style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold)
                if (cur != null) Text(Node.label(cur.addr) + if (cur.path == "index") "" else " / ${cur.path}", maxLines = 1,
                    overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis, style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
        when {
            editing -> androidx.compose.material3.IconButton(onClick = { go() }) {
                androidx.compose.material3.Icon(androidx.compose.material.icons.Icons.AutoMirrored.Filled.ArrowForward, "Go")
            }
            Pages.loading -> androidx.compose.material3.IconButton(onClick = { Pages.home() }) {
                androidx.compose.material3.Icon(androidx.compose.material.icons.Icons.Default.Close, "Stop")
            }
            else -> androidx.compose.material3.IconButton(onClick = { Pages.reload() }) {
                androidx.compose.material3.Icon(androidx.compose.material.icons.Icons.Default.Refresh, "Reload")
            }
        }
        if (!editing) {
            if (cur != null) FavStar('p', cur.addr)
            androidx.compose.material3.IconButton(onClick = { Pages.home() }) {
                androidx.compose.material3.Icon(androidx.compose.material.icons.Icons.Default.Home, "Start page")
            }
        }
    }
}

/** "LoRa" or "Net": how the page comes (pictures wait for a tap over LoRa). */
@Composable
private fun RouteBadge(lora: Boolean) {
    Text(if (lora) "LoRa" else "Net", style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.SemiBold,
        color = if (lora) Color(0xFF8A5A00) else MaterialTheme.colorScheme.primary,
        modifier = Modifier.clip(RoundedCornerShape(6.dp))
            .background(if (lora) Color(0x33E7A94B) else MaterialTheme.colorScheme.primaryContainer).padding(horizontal = 6.dp, vertical = 2.dp))
}

/** The bottom bar: route, size and when the page was fetched. */
@Composable
private fun StatusBar(cur: Pages.Url?, @Suppress("UNUSED_PARAMETER") version: Int) {
    // Read here too: a child given an unchanged-looking version may be skipped.
    Pages.version.collectAsState().value
    if (cur == null) return
    val text = when {
        Pages.loading -> "Loading from ${Node.label(cur.addr)}…"
        Pages.pageBytes > 0 -> "${if (Pages.overLora(cur.addr)) "Over LoRa" else "Over the internet"} · ${Pages.pageBytes} bytes · fetched " +
            java.text.SimpleDateFormat("HH:mm", java.util.Locale.getDefault()).format(java.util.Date(Pages.pageAt))
        else -> ""
    }
    if (text.isEmpty()) return
    Text(text, Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceContainer).padding(horizontal = 14.dp, vertical = 6.dp),
        style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

/** The start page: bookmarks as tiles, nodes sharing pages, recently visited. */
@Composable
private fun StartPage(@Suppress("UNUSED_PARAMETER") version: Int) {
    // Read here too: a child given an unchanged-looking version may be skipped.
    Pages.version.collectAsState().value
    val nodes = Node.nodes.value
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(14.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text("Bookmarks", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
        if (Pages.favPages.isEmpty()) Text("Star a page to keep it here.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Pages.favPages.chunked(2).forEach { row ->
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                row.forEach { a ->
                    val t = Pages.siteTitle(a).ifEmpty { Node.label(a) }
                    Column(Modifier.weight(1f).clip(RoundedCornerShape(14.dp)).background(MaterialTheme.colorScheme.surfaceContainer)
                        .clickable { Pages.open("maarse://$a/") }.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Avatar(t, a, 36.dp)
                        Text(t, fontWeight = FontWeight.SemiBold, maxLines = 2, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis)
                        Text(Node.label(a), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
                    }
                }
                if (row.size == 1) androidx.compose.foundation.layout.Spacer(Modifier.weight(1f))
            }
        }
        val others = Pages.sharing.filter { it !in Pages.favPages }
        if (others.isNotEmpty()) {
            Text("Nodes sharing pages", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
            others.forEach { a ->
                Row(Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).clickable { Pages.open("maarse://$a/") }.padding(vertical = 6.dp),
                    verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    Avatar(Node.label(a), a)
                    Column(Modifier.weight(1f)) {
                        Text(Pages.siteTitle(a).ifEmpty { nodes[a]?.name?.let { "~$it" } ?: a.take(8) }, fontWeight = FontWeight.SemiBold)
                        Text("maarse://${a.take(8)}…/", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            }
        }
        if (Pages.recent.isNotEmpty()) {
            Text("Recently visited", style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
            Pages.recent.forEach { (u, t) ->
                Row(Modifier.fillMaxWidth().clickable { Pages.open(u.toString()) }.padding(vertical = 6.dp)) {
                    Column {
                        Text(t.ifEmpty { u.path }, fontWeight = FontWeight.Medium)
                        Text(u.toString(), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1,
                            overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis)
                    }
                }
            }
        }
        Text("Type a node's address in the bar above (maarse://<32 hex characters>/). Pages are text, links and small pictures – never HTML.",
            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

private val PIC_LINE = Regex("""^!\[([^\]]*)\]\(([^)\s]+)(?:\s+"(grey|dots|colour|color)(!?)")?\)\s*$""")

/** One block of a page, parsed from the markup. */
internal sealed class Block {
    data class Para(val text: String, val centre: Boolean = false) : Block()
    data class Heading(val level: Int, val text: String, val centre: Boolean) : Block()
    data class Item(val text: String) : Block()
    /** Quote lines in a row; with a colour it is a callout box (`>!red …`). */
    data class Quote(val lines: List<String>, val colour: String?) : Block()
    data class Code(val text: String) : Block()
    data class Pic(val caption: String, val src: String, val style: String, val forced: Boolean) : Block()
    /** A pipe table; `aligns` is 0 left, 1 centre, 2 right per column. */
    data class Table(val head: List<String>?, val aligns: List<Int>, val rows: List<List<String>>) : Block()
    data object Rule : Block()
}

private val HEADING = Regex("""^(#{1,3})\s+(.*)$""")
private val ITEM = Regex("""^\s*[-*]\s+(.*)$""")
private val RULE = Regex("""^---+\s*$""")
private val TABLE_ROW = Regex("""^\s*\|""")
private val TABLE_SEP = Regex("""^:?-+:?$""")
private val CELL_SPLIT = Regex("""(?<!\\)\|""")
/** A centred line: `-> text <-`. */
private val CENTRE = Regex("""^->\s*(.*?)\s*<-\s*$""")
/** A quote line; `>!colour` starts a coloured box (callout). */
private val QUOTE = Regex("""^>(?:!([a-z]+)(?=\s|$))?\s?(.*)$""")

/** Page colours: text colour and soft background, light and dark (the web client's --pf-* and --pb-*). */
private class PageColour(val fgLight: Long, val bgLight: Long, val fgDark: Long, val bgDark: Long)
private val PAGE_COLOURS = mapOf(
    "red" to PageColour(0xFFB3261E, 0xFFFBDAD6, 0xFFFF8A80, 0xFF4A1D1A),
    "orange" to PageColour(0xFFB55400, 0xFFFDE2C8, 0xFFFFB46B, 0xFF4A2C10),
    "yellow" to PageColour(0xFF8A6A00, 0xFFFFF0A8, 0xFFFFD95A, 0xFF443A0E),
    "green" to PageColour(0xFF2E7D32, 0xFFD6EFD8, 0xFF7FD68A, 0xFF1B3A1F),
    "blue" to PageColour(0xFF1F5FBF, 0xFFDBE6FB, 0xFF8AB4FF, 0xFF1B2B4A),
    "purple" to PageColour(0xFF7A3DB0, 0xFFEBDCF7, 0xFFCDA2FF, 0xFF34234A),
    "grey" to PageColour(0xFF5F6B66, 0xFFE4E9E6, 0xFFA3AFA9, 0xFF2B3430),
)
private fun pageColour(name: String?): PageColour? = PAGE_COLOURS[if (name == "gray") "grey" else name]

/** Splits pipe-table lines into a header (when the second row is `|---|:--:|`), alignments and rows. */
internal fun parseTable(lines: List<String>): Block.Table {
    fun cells(l: String): List<String> {
        var s = l.trim().removePrefix("|")
        if (s.endsWith("|") && !s.endsWith("\\|")) s = s.dropLast(1)
        return s.split(CELL_SPLIT).map { it.trim().replace("\\|", "|") }
    }
    var rows = lines.map(::cells)
    var head: List<String>? = null
    var aligns = emptyList<Int>()
    if (rows.size > 1 && rows[1].all { TABLE_SEP.matches(it) }) {
        aligns = rows[1].map { if (it.endsWith(":")) (if (it.startsWith(":")) 1 else 2) else 0 }
        head = rows[0]
        rows = rows.drop(2)
    }
    val n = minOf(20, maxOf(head?.size ?: 0, rows.maxOfOrNull { it.size } ?: 0)).coerceAtLeast(1)
    fun fit(r: List<String>) = List(n) { r.getOrElse(it) { "" } }
    return Block.Table(head?.let(::fit), List(n) { aligns.getOrElse(it) { 0 } }, rows.map(::fit))
}

/** Splits markup into blocks (the same rules as the web client's renderMarkup). */
private fun parseMarkup(markup: String): List<Block> {
    val out = mutableListOf<Block>()
    val para = StringBuilder()
    var table: MutableList<String>? = null
    var quote: MutableList<String>? = null
    var quoteColour: String? = null
    fun flush() {
        if (para.isNotEmpty()) { out += Block.Para(para.toString()); para.clear() }
        table?.let { out += parseTable(it); table = null }
        quote?.let { out += Block.Quote(it, quoteColour); quote = null }
    }
    var code: MutableList<String>? = null
    for (line in markup.replace("\r", "").split("\n")) {
        val c = code
        if (c != null) {
            if (line.startsWith("```")) { out += Block.Code(c.joinToString("\n")); code = null } else c += line
            continue
        }
        if (table != null && !TABLE_ROW.containsMatchIn(line)) flush()
        val h = HEADING.find(line)
        val pic = PIC_LINE.find(line)
        val item = ITEM.find(line)
        val q = QUOTE.find(line)
        val centre = CENTRE.find(line)
        when {
            line.startsWith("```") -> { flush(); code = mutableListOf() }
            h != null -> {
                flush()
                val hc = CENTRE.find(h.groupValues[2])
                out += Block.Heading(h.groupValues[1].length, hc?.groupValues?.get(1) ?: h.groupValues[2], hc != null)
            }
            line.isBlank() -> flush()
            line.startsWith("@title ") -> {}
            RULE.matches(line) -> { flush(); out += Block.Rule }
            pic != null -> { flush(); out += Block.Pic(pic.groupValues[1], pic.groupValues[2], pic.groupValues[3], pic.groupValues[4] == "!") }
            TABLE_ROW.containsMatchIn(line) -> { if (table == null) { flush(); table = mutableListOf() }; table!! += line }
            item != null -> { flush(); out += Block.Item(item.groupValues[1]) }
            q != null -> {
                // Quote lines in a row make one box; `>!colour` starts a new, coloured one.
                val colour = q.groupValues[1]
                if (quote == null || colour.isNotEmpty()) { flush(); quote = mutableListOf(); quoteColour = pageColour(colour)?.let { colour } }
                quote!! += q.groupValues[2]
            }
            centre != null -> { flush(); out += Block.Para(centre.groupValues[1], centre = true) }
            else -> {
                quote?.let { out += Block.Quote(it, quoteColour); quote = null }
                if (para.isNotEmpty()) para.append(' '); para.append(line.trim())
            }
        }
    }
    code?.let { out += Block.Code(it.joinToString("\n")) }
    flush()
    return out
}

/** The page's markup as Compose: headings, paragraphs, lists, quotes, callouts, tables, code, rules and pictures. */
@Composable
private fun Markup(markup: String, base: Pages.Url, version: Int) {
    // Read here too: a child given an unchanged-looking version may be skipped.
    Pages.version.collectAsState().value
    val blocks = remember(markup) { parseMarkup(markup) }
    val body = MaterialTheme.typography.bodyLarge
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        for (b in blocks) when (b) {
            is Block.Para -> if (b.centre) Inline(b.text, base, body, align = TextAlign.Center, modifier = Modifier.fillMaxWidth()) else Inline(b.text, base, body)
            is Block.Heading -> Inline(b.text, base, when (b.level) { 1 -> MaterialTheme.typography.headlineSmall; 2 -> MaterialTheme.typography.titleLarge; else -> MaterialTheme.typography.titleMedium }, bold = true,
                align = if (b.centre) TextAlign.Center else null, modifier = if (b.centre) Modifier.fillMaxWidth() else Modifier)
            is Block.Item -> Row { Text("•  "); Inline(b.text, base, body) }
            is Block.Quote -> {
                val pc = pageColour(b.colour)
                if (pc == null) Box(Modifier.padding(start = 8.dp).background(MaterialTheme.colorScheme.surfaceContainer).padding(8.dp)) { Inline(b.lines.joinToString("\n"), base, body) }
                else {
                    val dark = pageDark()
                    val bar = Color(if (dark) pc.fgDark else pc.fgLight)
                    Box(Modifier.fillMaxWidth().background(Color(if (dark) pc.bgDark else pc.bgLight), RoundedCornerShape(topEnd = 6.dp, bottomEnd = 6.dp))
                        .drawBehind { drawRect(bar, size = androidx.compose.ui.geometry.Size(3.dp.toPx(), size.height)) }
                        .padding(start = 13.dp, end = 10.dp, top = 6.dp, bottom = 6.dp)) { Inline(b.lines.joinToString("\n"), base, body) }
                }
            }
            is Block.Table -> PageTable(b, base)
            is Block.Code -> CodeBlock(b.text)
            is Block.Pic -> Picture(b.caption, b.src, b.style, base, version, b.forced)
            Block.Rule -> HorizontalDivider()
        }
    }
}

@Composable
private fun pageDark() = MaterialTheme.colorScheme.surface.luminance() < 0.5f

/** A pipe table. Columns get their natural width when the table fits the screen,
 *  else they share the width like a browser does (text wraps), and when even the
 *  longest words do not fit, the table scrolls sideways on its own, never the page. */
@Composable
internal fun PageTable(t: Block.Table, base: Pages.Url) {
    val line = MaterialTheme.colorScheme.outlineVariant
    val headBg = MaterialTheme.colorScheme.surfaceContainer
    val n = t.aligns.size
    val rows = listOfNotNull(t.head) + t.rows
    androidx.compose.foundation.layout.BoxWithConstraints(Modifier.fillMaxWidth()) {
        val avail = constraints.maxWidth
        Box(Modifier.horizontalScroll(rememberScrollState())) {
            androidx.compose.ui.layout.Layout(content = {
                rows.forEachIndexed { r, row ->
                    val head = t.head != null && r == 0
                    row.forEachIndexed { i, cell ->
                        Box((if (head) Modifier.background(headBg) else Modifier).border(0.5.dp, line).padding(horizontal = 8.dp, vertical = 4.dp)) {
                            Inline(cell, base, MaterialTheme.typography.bodyMedium, bold = head, modifier = Modifier.fillMaxWidth(),
                                align = when (t.aligns[i]) { 1 -> TextAlign.Center; 2 -> TextAlign.End; else -> TextAlign.Start })
                        }
                    }
                }
            }) { ms, _ ->
                val count = ms.size / n
                val nat = IntArray(n); val min = IntArray(n)
                ms.forEachIndexed { k, m ->
                    nat[k % n] = maxOf(nat[k % n], m.maxIntrinsicWidth(androidx.compose.ui.unit.Constraints.Infinity))
                    min[k % n] = maxOf(min[k % n], m.minIntrinsicWidth(androidx.compose.ui.unit.Constraints.Infinity))
                }
                val sn = nat.sum(); val sm = min.sum()
                val w = when {
                    sn <= avail -> nat
                    sm >= avail -> min
                    else -> IntArray(n) { min[it] + ((nat[it] - min[it]).toLong() * (avail - sm) / (sn - sm)).toInt() }
                }
                val h = IntArray(count) { r -> (0 until n).maxOf { i -> ms[r * n + i].maxIntrinsicHeight(w[i]) } }
                val ps = ms.mapIndexed { k, m -> m.measure(androidx.compose.ui.unit.Constraints.fixed(w[k % n], h[k / n])) }
                layout(w.sum(), h.sum()) {
                    var y = 0
                    for (r in 0 until count) {
                        var x = 0
                        for (i in 0 until n) { ps[r * n + i].place(x, y); x += w[i] }
                        y += h[r]
                    }
                }
            }
        }
    }
}

@Composable
internal fun CodeBlock(t: String) {
    // ASCII art and tables keep their lines: the font shrinks so the longest
    // line fits the screen (down to a readable minimum), then it scrolls.
    androidx.compose.foundation.layout.BoxWithConstraints(Modifier.fillMaxWidth().clip(RoundedCornerShape(6.dp)).background(MaterialTheme.colorScheme.surfaceContainer)) {
        val longest = t.lines().maxOfOrNull { it.length }?.coerceAtLeast(1) ?: 1
        val density = androidx.compose.ui.platform.LocalDensity.current
        val widthPx = with(density) { (maxWidth - 20.dp).toPx() }
        val fitSp = with(density) { (widthPx / (longest * 0.75f)).toSp().value }
        val size = fitSp.coerceIn(6.5f, 13f)
        Text(t, fontFamily = FontFamily.Monospace, fontSize = androidx.compose.ui.unit.TextUnit(size, androidx.compose.ui.unit.TextUnitType.Sp),
            lineHeight = androidx.compose.ui.unit.TextUnit(size * 1.25f, androidx.compose.ui.unit.TextUnitType.Sp),
            modifier = Modifier.horizontalScroll(rememberScrollState()).padding(10.dp), softWrap = false)
    }
}

/** Inline markup: `code`, **bold**, ~~struck~~, *italic*, {red:colour}, {!red:highlight}
 *  and [text](link). Bold, struck, italic and colour may hold other inline markup;
 *  an unknown colour is plain text. The same rules as the web client's pageInline. */
private val INLINE = Regex("""`([^`]+)`|\*\*(.+?)\*\*|~~(.+?)~~|\*([^*\s](?:[^*]*[^*\s])?)\*|\{(!?)([a-z]+):([^{}]+)\}|\[([^\]]+)\]\(([^)\s]+)\)""")
private val LINK_OUT = Regex("^https?://", RegexOption.IGNORE_CASE)

private fun AnnotatedString.Builder.inline(text: String, base: Pages.Url, accent: Color, ink: Color, dark: Boolean, depth: Int = 0) {
    if (depth > 4) { append(text); return }
    var last = 0
    for (m in INLINE.findAll(text)) {
        append(text.substring(last, m.range.first))
        val g = m.groupValues
        when {
            g[1].isNotEmpty() -> withStyle(SpanStyle(fontFamily = FontFamily.Monospace)) { append(g[1]) }
            g[2].isNotEmpty() -> withStyle(SpanStyle(fontWeight = FontWeight.Bold)) { inline(g[2], base, accent, ink, dark, depth + 1) }
            g[3].isNotEmpty() -> withStyle(SpanStyle(textDecoration = TextDecoration.LineThrough)) { inline(g[3], base, accent, ink, dark, depth + 1) }
            g[4].isNotEmpty() -> withStyle(SpanStyle(fontStyle = FontStyle.Italic)) { inline(g[4], base, accent, ink, dark, depth + 1) }
            g[6].isNotEmpty() -> {
                val pc = pageColour(g[6])
                val st = when {
                    pc == null -> SpanStyle()
                    g[5] == "!" -> SpanStyle(color = ink, background = Color(if (dark) pc.bgDark else pc.bgLight))
                    else -> SpanStyle(color = Color(if (dark) pc.fgDark else pc.fgLight))
                }
                withStyle(st) { inline(g[7], base, accent, ink, dark, depth + 1) }
            }
            else -> {
                val target = g[9]
                val p = Pages.parse(target, base)
                if (p != null) withLink(LinkAnnotation.Clickable(p.toString(), TextLinkStyles(SpanStyle(color = accent))) { Pages.open(p.toString()) }) { append(g[8]) }
                else if (LINK_OUT.containsMatchIn(target)) append("${g[8]} <$target>")
                else append(g[8])
            }
        }
        last = m.range.last + 1
    }
    append(text.substring(last))
}

/** A line of inline markup: links to maarse:// pages open here; links out are shown as text. */
@Composable
private fun Inline(text: String, base: Pages.Url, style: androidx.compose.ui.text.TextStyle, bold: Boolean = false,
                   align: TextAlign? = null, modifier: Modifier = Modifier) {
    val accent = MaterialTheme.colorScheme.primary
    val ink = MaterialTheme.colorScheme.onSurface
    val dark = pageDark()
    val s: AnnotatedString = buildAnnotatedString { inline(text, base, accent, ink, dark) }
    Text(s, modifier, style = style, fontWeight = if (bold) FontWeight.SemiBold else null, textAlign = align)
}

// Pictures come in steps, all made by the host as WebP and the same on every
// route: Greyscale (about 1.3 KB) first, then on request Colour 4 KB and
// Colour 8 KB. Over LoRa the first waits for a tap; over the internet it loads
// at once. Tapping the picture offers the next steps. `forced` ("dots!" on the page):
// that copy on every route, no steps.
private data class PicStep(val id: String, val name: String, val time: String, val net: Boolean = false)
// Full size is the original: over the internet only; over LoRa 8 KB is the most.
private val PIC_STEPS = listOf(PicStep("_c4", "Colour 4 KB", "about 35 s"), PicStep("_c8", "Colour 8 KB", "about 1 min"),
    PicStep("_full", "Full size", "", net = true))

/** A picture: over LoRa it waits for a tap; over the internet it loads at once.
 *  `version` is passed so a picture redraws when Pages changes. */
@OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
@Composable
internal fun Picture(caption: String, src: String, style: String, base: Pages.Url, @Suppress("UNUSED_PARAMETER") version: Int, forced: Boolean = false) {
    // Read here too: a child given an unchanged-looking version may be skipped.
    Pages.version.collectAsState().value
    val st = when (style) { "color", "colour" -> "colour"; "dots" -> "dots"; else -> "grey" }
    val rel = src.trimStart('/').lowercase()
    val path = if (forced) "_f$st/$rel" else "_$st/$rel"
    val loadText = "Click here to see the picture · " + when {
        !forced -> "smallest size, about 1.3 KB"
        st == "dots" -> "dots, set by the page"
        st == "colour" -> "Colour 8 KB, set by the page"
        else -> "Greyscale, set by the page"
    }
    val key = "${base.addr}/$path"
    val lora = Pages.overLora(base.addr)
    // The best step that has come wins: Colour 8 KB, Colour 4 KB, then the first copy.
    val steps = if (forced) emptyList() else PIC_STEPS.filter { !(it.net && lora) }
    val top = steps.lastOrNull { Pages.picture("${base.addr}/${it.id}/$rel") is Pages.Pic.Got }
    val first = Pages.picture(key)
    val pic = top?.let { Pages.picture("${base.addr}/${it.id}/$rel") } ?: first
    var large by remember(key) { mutableStateOf(false) }
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        when (pic) {
            is Pages.Pic.Got -> {
                val bmp = remember(pic) { BitmapFactory.decodeByteArray(pic.data, 0, pic.data.size) }
                val stepName = top?.name ?: when { pic.mime == "image/png" -> "Dots"; forced && st == "colour" -> "Colour 8 KB"; else -> "Greyscale" }
                if (bmp != null) Image(bmp.asImageBitmap(), caption.ifEmpty { rel },
                    Modifier.widthIn(max = 320.dp).fillMaxWidth().clip(RoundedCornerShape(4.dp)).background(Color.White)
                        // A tap opens the picture full screen (zoom there).
                        .clickable { large = true },
                    contentScale = ContentScale.FillWidth,
                    filterQuality = if (pic.mime == "image/png") FilterQuality.None else FilterQuality.Low)
                if (large && bmp != null) ZoomImage(bmp.asImageBitmap(), caption.ifEmpty { rel }, pic.mime == "image/png") { large = false }
                Text("${if (caption.isNotEmpty()) "$caption · " else ""}$stepName · " + "%.1f KB".format(pic.data.size / 1024f),
                    style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                if (!forced) {
                    // Only the next step: smallest first, one size up at a time.
                    val higher = steps.drop(if (top == null) 0 else steps.indexOf(top) + 1).take(1)
                    val working = higher.firstOrNull { val p = Pages.picture("${base.addr}/${it.id}/$rel"); p is Pages.Pic.Busy || p is Pages.Pic.Asking || Pages.queued(base.addr, "${it.id}/$rel") }
                    val wp = working?.let { Pages.picture("${base.addr}/${it.id}/$rel") }
                    when {
                        wp is Pages.Pic.Busy -> Column(Modifier.widthIn(max = 320.dp)) {
                            androidx.compose.material3.LinearProgressIndicator(progress = { wp.percent / 100f }, modifier = Modifier.fillMaxWidth())
                            Text("${working.name} · %d %% · %.1f of %.1f KB".format(wp.percent, wp.total * wp.percent / 100f / 1024f, wp.total / 1024f),
                                style = MaterialTheme.typography.labelSmall)
                        }
                        working != null -> Column(Modifier.widthIn(max = 320.dp)) {
                            Text("Asking for ${working.name}…", style = MaterialTheme.typography.labelSmall)
                            androidx.compose.material3.LinearProgressIndicator(Modifier.fillMaxWidth())
                        }
                        // The next steps, under the picture.
                        higher.isNotEmpty() -> androidx.compose.foundation.layout.FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            higher.forEach { s ->
                                val sp = Pages.picture("${base.addr}/${s.id}/$rel")
                                OutlinedButton(onClick = { Pages.requestPicture(base.addr, "${s.id}/$rel") }) {
                                    Text(if (sp is Pages.Pic.Failed) "${s.name}: try again" else if (lora) "${s.name} · ${s.time}" else s.name)
                                }
                            }
                        }
                    }
                }
            }
            else -> Column(
                Modifier.widthIn(max = 320.dp).fillMaxWidth().heightIn(min = 110.dp).border(1.dp, MaterialTheme.colorScheme.outlineVariant, RoundedCornerShape(6.dp)).padding(12.dp),
                horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp, Alignment.CenterVertically),
            ) {
                Text(caption.ifEmpty { rel })
                when (pic) {
                    Pages.Pic.Asking -> {
                        Text("Asking the node…", style = MaterialTheme.typography.bodySmall)
                        androidx.compose.material3.LinearProgressIndicator(Modifier.fillMaxWidth())
                    }
                    is Pages.Pic.Busy -> {
                        androidx.compose.material3.LinearProgressIndicator(progress = { pic.percent / 100f }, modifier = Modifier.fillMaxWidth())
                        Text("Receiving · %d %% · %.1f of %.1f KB".format(pic.percent, pic.total * pic.percent / 100f / 1024f, pic.total / 1024f),
                            style = MaterialTheme.typography.bodySmall)
                    }
                    null -> if (Pages.queued(base.addr, path)) Text("Waiting for the picture before it…", style = MaterialTheme.typography.bodySmall)
                        else OutlinedButton(onClick = { Pages.requestPicture(base.addr, path) }) { Text(loadText) }
                    else -> OutlinedButton(onClick = { Pages.requestPicture(base.addr, path) }) {
                        Text(if (pic is Pages.Pic.Failed) "Try again (${pic.why})" else loadText)
                    }
                }
            }
        }
    }
}


/** Full screen picture: pinch to zoom, drag to move, double-tap to zoom in
 *  and out; Back or Close closes. */
@Composable
private fun ZoomImage(img: androidx.compose.ui.graphics.ImageBitmap, name: String, pixels: Boolean, onClose: () -> Unit) {
    androidx.compose.ui.window.Dialog(onDismissRequest = onClose,
        properties = androidx.compose.ui.window.DialogProperties(usePlatformDefaultWidth = false)) {
        var scale by remember { mutableStateOf(1f) }
        var offset by remember { mutableStateOf(androidx.compose.ui.geometry.Offset.Zero) }
        Box(Modifier.fillMaxSize().background(Color.Black)
            .pointerInput(Unit) {
                detectTransformGestures { _, pan, zoom, _ ->
                    scale = (scale * zoom).coerceIn(1f, 8f)
                    offset = if (scale == 1f) androidx.compose.ui.geometry.Offset.Zero else offset + pan
                }
            }
            .pointerInput(Unit) {
                detectTapGestures(onDoubleTap = {
                    if (scale > 1f) { scale = 1f; offset = androidx.compose.ui.geometry.Offset.Zero } else scale = 2.5f
                })
            }) {
            Image(img, name, Modifier.fillMaxSize().graphicsLayer(scaleX = scale, scaleY = scale, translationX = offset.x, translationY = offset.y),
                contentScale = ContentScale.Fit, filterQuality = if (pixels || scale > 2f) FilterQuality.None else FilterQuality.Low)
            androidx.compose.material3.TextButton(onClick = onClose, Modifier.align(Alignment.TopEnd).statusBarsPadding().padding(8.dp)) {
                Text("Close", color = Color.White)
            }
        }
    }
}

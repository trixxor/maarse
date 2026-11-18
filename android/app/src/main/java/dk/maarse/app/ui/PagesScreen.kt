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
import androidx.compose.ui.unit.dp
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
                else -> Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 14.dp, vertical = 12.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (Pages.status.isNotEmpty()) Text(Pages.status, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
                    Markup(markup, cur, v)
                    androidx.compose.foundation.layout.Spacer(Modifier.height(24.dp))
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
private sealed class Block {
    data class Para(val text: String) : Block()
    data class Heading(val level: Int, val text: String) : Block()
    data class Item(val text: String) : Block()
    data class Quote(val text: String) : Block()
    data class Code(val text: String) : Block()
    data class Pic(val caption: String, val src: String, val style: String, val forced: Boolean) : Block()
    data object Rule : Block()
}

private val HEADING = Regex("""^(#{1,3})\s+(.*)$""")
private val ITEM = Regex("""^\s*[-*]\s+(.*)$""")
private val RULE = Regex("""^---+\s*$""")

/** Splits markup into blocks (the same rules as the web client's renderMarkup). */
private fun parseMarkup(markup: String): List<Block> {
    val out = mutableListOf<Block>()
    val para = StringBuilder()
    fun flush() { if (para.isNotEmpty()) { out += Block.Para(para.toString()); para.clear() } }
    var code: MutableList<String>? = null
    for (line in markup.replace("\r", "").split("\n")) {
        val c = code
        if (c != null) {
            if (line.startsWith("```")) { out += Block.Code(c.joinToString("\n")); code = null } else c += line
            continue
        }
        val h = HEADING.find(line)
        val pic = PIC_LINE.find(line)
        val item = ITEM.find(line)
        when {
            line.startsWith("```") -> { flush(); code = mutableListOf() }
            h != null -> { flush(); out += Block.Heading(h.groupValues[1].length, h.groupValues[2]) }
            line.isBlank() -> flush()
            line.startsWith("@title ") -> {}
            RULE.matches(line) -> { flush(); out += Block.Rule }
            pic != null -> { flush(); out += Block.Pic(pic.groupValues[1], pic.groupValues[2], pic.groupValues[3], pic.groupValues[4] == "!") }
            item != null -> { flush(); out += Block.Item(item.groupValues[1]) }
            line.startsWith(">") -> { flush(); out += Block.Quote(line.removePrefix(">").trimStart()) }
            else -> { if (para.isNotEmpty()) para.append(' '); para.append(line.trim()) }
        }
    }
    code?.let { out += Block.Code(it.joinToString("\n")) }
    flush()
    return out
}

/** The page's markup as Compose: headings, paragraphs, lists, quotes, code, rules and pictures. */
@Composable
private fun Markup(markup: String, base: Pages.Url, version: Int) {
    // Read here too: a child given an unchanged-looking version may be skipped.
    Pages.version.collectAsState().value
    val blocks = remember(markup) { parseMarkup(markup) }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        for (b in blocks) when (b) {
            is Block.Para -> Inline(b.text, base, MaterialTheme.typography.bodyLarge)
            is Block.Heading -> Inline(b.text, base, when (b.level) { 1 -> MaterialTheme.typography.headlineSmall; 2 -> MaterialTheme.typography.titleLarge; else -> MaterialTheme.typography.titleMedium }, bold = true)
            is Block.Item -> Row { Text("•  "); Inline(b.text, base, MaterialTheme.typography.bodyLarge) }
            is Block.Quote -> Box(Modifier.padding(start = 8.dp).background(MaterialTheme.colorScheme.surfaceContainer).padding(8.dp)) { Inline(b.text, base, MaterialTheme.typography.bodyLarge) }
            is Block.Code -> CodeBlock(b.text)
            is Block.Pic -> Picture(b.caption, b.src, b.style, base, version, b.forced)
            Block.Rule -> HorizontalDivider()
        }
    }
}

@Composable
private fun CodeBlock(t: String) {
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

/** **bold**, `code` and [text](link): links to maarse:// pages open here; links out are shown as text. */
@Composable
private fun Inline(text: String, base: Pages.Url, style: androidx.compose.ui.text.TextStyle, bold: Boolean = false) {
    val accent = MaterialTheme.colorScheme.primary
    val s: AnnotatedString = buildAnnotatedString {
        val re = Regex("""\*\*([^*]+)\*\*|`([^`]+)`|\[([^\]]+)\]\(([^)\s]+)\)""")
        var last = 0
        for (m in re.findAll(text)) {
            append(text.substring(last, m.range.first))
            when {
                m.groupValues[1].isNotEmpty() -> withStyle(SpanStyle(fontWeight = FontWeight.Bold)) { append(m.groupValues[1]) }
                m.groupValues[2].isNotEmpty() -> withStyle(SpanStyle(fontFamily = FontFamily.Monospace)) { append(m.groupValues[2]) }
                else -> {
                    val target = m.groupValues[4]
                    val p = Pages.parse(target, base)
                    if (p != null) withLink(LinkAnnotation.Clickable(p.toString(), TextLinkStyles(SpanStyle(color = accent))) { Pages.open(p.toString()) }) { append(m.groupValues[3]) }
                    else if (Regex("^https?://", RegexOption.IGNORE_CASE).containsMatchIn(target)) append("${m.groupValues[3]} <$target>")
                    else append(m.groupValues[3])
                }
            }
            last = m.range.last + 1
        }
        append(text.substring(last))
    }
    Text(s, style = style, fontWeight = if (bold) FontWeight.SemiBold else null)
}

// Pictures come in steps over LoRa, all made by the host as WebP:
// Greyscale (about 1.3 KB) first, then on request Colour 4 KB and Colour 8 KB.
// Tapping the picture offers the next steps. `forced` ("dots!" on the page):
// that copy on every route, no steps.
private data class PicStep(val id: String, val name: String, val time: String)
private val PIC_STEPS = listOf(PicStep("_c4", "Colour 4 KB", "about 35 s"), PicStep("_c8", "Colour 8 KB", "about 1 min"))

/** A picture: over LoRa it waits for a tap; over the internet it loads at once.
 *  `version` is passed so a picture redraws when Pages changes. */
@OptIn(androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
@Composable
private fun Picture(caption: String, src: String, style: String, base: Pages.Url, @Suppress("UNUSED_PARAMETER") version: Int, forced: Boolean = false) {
    // Read here too: a child given an unchanged-looking version may be skipped.
    Pages.version.collectAsState().value
    val st = when (style) { "color", "colour" -> "colour"; "dots" -> "dots"; else -> "grey" }
    val rel = src.trimStart('/').lowercase()
    val path = if (forced) "_f$st/$rel" else "_$st/$rel"
    val loadText = "Load picture · " + when {
        !forced -> "Greyscale · about 1.3 KB · 10–15 s"
        st == "dots" -> "dots, set by the page · 10–20 s"
        st == "colour" -> "Colour 8 KB, set by the page · about 1 min"
        else -> "Greyscale, set by the page · 10–15 s"
    } + " over LoRa"
    val key = "${base.addr}/$path"
    val lora = Pages.overLora(base.addr)
    // The best step that has come wins: Colour 8 KB, Colour 4 KB, then the first copy.
    val steps = if (forced) emptyList() else PIC_STEPS
    val top = steps.lastOrNull { Pages.picture("${base.addr}/${it.id}/$rel") is Pages.Pic.Got }
    val first = Pages.picture(key)
    val pic = top?.let { Pages.picture("${base.addr}/${it.id}/$rel") } ?: first
    var menu by remember(key) { mutableStateOf(false) }
    var large by remember(key) { mutableStateOf(false) }
    LaunchedEffect(key, lora) {
        if (!lora && pic == null && !Pages.queued(base.addr, path)) Pages.requestPicture(base.addr, path)
    }
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        when (pic) {
            is Pages.Pic.Got -> {
                val bmp = remember(pic) { BitmapFactory.decodeByteArray(pic.data, 0, pic.data.size) }
                val stepName = top?.name ?: when { pic.mime == "image/png" -> "Dots"; lora -> "Greyscale"; else -> "Original" }
                if (bmp != null) Image(bmp.asImageBitmap(), caption.ifEmpty { rel },
                    Modifier.widthIn(max = 320.dp).fillMaxWidth().clip(RoundedCornerShape(4.dp)).background(Color.White)
                        // Over LoRa a tap offers a better step; otherwise it shows the picture large.
                        .clickable { if (lora && !forced) menu = !menu else large = true },
                    contentScale = ContentScale.FillWidth,
                    filterQuality = if (pic.mime == "image/png") FilterQuality.None else FilterQuality.Low)
                if (large && bmp != null) androidx.compose.ui.window.Dialog(onDismissRequest = { large = false }) {
                    Image(bmp.asImageBitmap(), caption.ifEmpty { rel }, Modifier.fillMaxWidth().background(Color.White).clickable { large = false },
                        contentScale = ContentScale.FillWidth,
                        filterQuality = if (pic.mime == "image/png") FilterQuality.None else FilterQuality.Low)
                }
                Text("${if (caption.isNotEmpty()) "$caption · " else ""}$stepName · " + "%.1f KB".format(pic.data.size / 1024f),
                    style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                if (lora && !forced) {
                    val higher = steps.drop(if (top == null) 0 else steps.indexOf(top) + 1)
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
                        menu -> androidx.compose.foundation.layout.FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            higher.forEach { s ->
                                val sp = Pages.picture("${base.addr}/${s.id}/$rel")
                                OutlinedButton(onClick = { menu = false; Pages.requestPicture(base.addr, "${s.id}/$rel") }) {
                                    Text(if (sp is Pages.Pic.Failed) "${s.name}: try again" else "${s.name} · ${s.time}")
                                }
                            }
                            androidx.compose.material3.TextButton(onClick = { menu = false; large = true }) { Text("Show larger") }
                        }
                        higher.isNotEmpty() -> Text("Tap the picture for a better one", style = MaterialTheme.typography.labelSmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant)
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

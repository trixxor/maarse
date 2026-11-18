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

/** maarse:// pages from other nodes, with pictures (same markup as the web client). */
@Composable
fun PagesPage() {
    // Read here, so every change in Pages redraws this screen.
    @Suppress("UNUSED_VARIABLE") val v = Pages.version.collectAsState().value
    val cur = Pages.url
    var input by remember(cur) { mutableStateOf(cur?.toString() ?: "") }
    Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(horizontal = 12.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            androidx.compose.material3.IconButton(onClick = { Pages.back() }, enabled = Pages.history.isNotEmpty()) {
                androidx.compose.material3.Icon(androidx.compose.material.icons.Icons.AutoMirrored.Filled.ArrowBack, "Previous page")
            }
            OutlinedTextField(input, { input = it }, Modifier.weight(1f), singleLine = true, placeholder = { Text("maarse://address/path") },
                textStyle = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Go), keyboardActions = KeyboardActions(onGo = { Pages.open(input) }))
            androidx.compose.material3.IconButton(onClick = { Pages.open(input) }) {
                androidx.compose.material3.Icon(androidx.compose.material.icons.Icons.Default.Refresh, "Fetch")
            }
        }
        if (Pages.status.isNotEmpty()) Text(Pages.status, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (Pages.sharing.isNotEmpty()) {
            Text("Nodes sharing pages", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            val nodes = Node.nodes.value
            Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Pages.sharing.forEach { a ->
                    val name = nodes[a]?.name?.let { "~$it" } ?: a.take(8)
                    if (cur?.addr == a) Button(onClick = { Pages.open("maarse://$a/") }) { Text(name) }
                    else OutlinedButton(onClick = { Pages.open("maarse://$a/") }) { Text(name) }
                }
            }
        }
        if (cur == null) {
            Text("A maarse-host shares pages from the folder pages/ in its data folder (index.mu is the front page). " +
                "Write a node's address above. Pages are only text, headings, lists, links and pictures – never HTML.",
                style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            return@Column
        }
        val markup = Pages.markup(cur) ?: return@Column
        // The page's title, like a web page's <title>, and its favourite star.
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(Pages.title(markup).ifEmpty { "Untitled page" }, Modifier.weight(1f), style = MaterialTheme.typography.titleLarge, fontWeight = FontWeight.Bold)
            FavStar('p', cur.addr)
        }
        Markup(markup, cur, v)
    }
}

private val PIC_LINE = Regex("""^!\[([^\]]*)\]\(([^)\s]+)(?:\s+"(grey|dots|colour|color)")?\)\s*$""")

/** One block of a page, parsed from the markup. */
private sealed class Block {
    data class Para(val text: String) : Block()
    data class Heading(val level: Int, val text: String) : Block()
    data class Item(val text: String) : Block()
    data class Quote(val text: String) : Block()
    data class Code(val text: String) : Block()
    data class Pic(val caption: String, val src: String, val style: String) : Block()
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
            pic != null -> { flush(); out += Block.Pic(pic.groupValues[1], pic.groupValues[2], pic.groupValues[3]) }
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
    val blocks = remember(markup) { parseMarkup(markup) }
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        for (b in blocks) when (b) {
            is Block.Para -> Inline(b.text, base, MaterialTheme.typography.bodyLarge)
            is Block.Heading -> Inline(b.text, base, when (b.level) { 1 -> MaterialTheme.typography.headlineSmall; 2 -> MaterialTheme.typography.titleLarge; else -> MaterialTheme.typography.titleMedium }, bold = true)
            is Block.Item -> Row { Text("•  "); Inline(b.text, base, MaterialTheme.typography.bodyLarge) }
            is Block.Quote -> Box(Modifier.padding(start = 8.dp).background(MaterialTheme.colorScheme.surfaceContainer).padding(8.dp)) { Inline(b.text, base, MaterialTheme.typography.bodyLarge) }
            is Block.Code -> CodeBlock(b.text)
            is Block.Pic -> Picture(b.caption, b.src, b.style, base, version)
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

/** A picture: over LoRa it waits for a tap; over the internet it loads at once. */
@Composable
// `version` is passed so a picture redraws when Pages changes.
private fun Picture(caption: String, src: String, style: String, base: Pages.Url, @Suppress("UNUSED_PARAMETER") version: Int) {
    val st = when (style) { "color", "colour" -> "colour"; "dots" -> "dots"; else -> "grey" }
    val rel = src.trimStart('/').lowercase()
    val path = "_$st/$rel"
    val key = "${base.addr}/$path"
    // The better copy (asked for after the small one over LoRa) wins when it has come.
    val bestPath = "_best/$rel"
    val best = Pages.picture("${base.addr}/$bestPath")
    val small = Pages.picture(key)
    val pic = if (best is Pages.Pic.Got) best else small
    LaunchedEffect(key, Pages.overLora(base.addr)) {
        if (!Pages.overLora(base.addr) && pic == null && !Pages.queued(base.addr, path)) Pages.requestPicture(base.addr, path)
    }
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        when (pic) {
            is Pages.Pic.Got -> {
                val bmp = remember(pic) { BitmapFactory.decodeByteArray(pic.data, 0, pic.data.size) }
                if (bmp != null) Image(bmp.asImageBitmap(), caption.ifEmpty { rel },
                    Modifier.widthIn(max = 320.dp).fillMaxWidth().clip(RoundedCornerShape(4.dp)).background(Color.White),
                    contentScale = ContentScale.FillWidth,
                    filterQuality = if (pic.mime == "image/png") FilterQuality.None else FilterQuality.Low)
                Text("${if (caption.isNotEmpty()) "$caption · " else ""}%.1f KB".format(pic.data.size / 1024f) + if (best is Pages.Pic.Got) " · better picture" else "",
                    style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                // Over LoRa the small copy came first: a better one can be asked for.
                if (Pages.overLora(base.addr) && best !is Pages.Pic.Got) when {
                    best is Pages.Pic.Busy -> Column(Modifier.widthIn(max = 320.dp)) {
                        androidx.compose.material3.LinearProgressIndicator(progress = { best.percent / 100f }, modifier = Modifier.fillMaxWidth())
                        Text("Better picture · %d %% · %.1f of %.1f KB".format(best.percent, best.total * best.percent / 100f / 1024f, best.total / 1024f),
                            style = MaterialTheme.typography.labelSmall)
                    }
                    best is Pages.Pic.Asking || Pages.queued(base.addr, bestPath) -> Column(Modifier.widthIn(max = 320.dp)) {
                        Text("Asking for a better picture…", style = MaterialTheme.typography.labelSmall)
                        androidx.compose.material3.LinearProgressIndicator(Modifier.fillMaxWidth())
                    }
                    else -> OutlinedButton(onClick = { Pages.requestPicture(base.addr, bestPath) }) {
                        Text(if (best is Pages.Pic.Failed) "Better picture: try again (${best.why})" else "Get a better picture · colour, up to 8 KB · about 1 min")
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
                        else OutlinedButton(onClick = { Pages.requestPicture(base.addr, path) }) { Text("Load picture · smallest version · about 10–20 s over LoRa") }
                    else -> OutlinedButton(onClick = { Pages.requestPicture(base.addr, path) }) {
                        Text(if (pic is Pages.Pic.Failed) "Try again (${pic.why})" else "Load picture · smallest version · about 10–20 s over LoRa")
                    }
                }
            }
        }
    }
}

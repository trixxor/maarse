package dk.maarse.app.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.basicMarquee
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.ScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.InlineTextContent
import androidx.compose.foundation.text.appendInlineContent
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.Shadow
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.rotate as drawRotate
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.LinkAnnotation
import androidx.compose.ui.text.Placeholder
import androidx.compose.ui.text.PlaceholderVerticalAlign
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLinkStyles
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withLink
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.TextUnitType
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import dk.maarse.app.Cmd
import dk.maarse.app.Node
import dk.maarse.app.Pages
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

// Må:rse markup (MU) on Android: the same rules as the web client's renderer
// (maarse-client/index.html, renderMarkup). The whole look of a page comes
// from its .mu file; on a phone columns become one column, cards go two to a
// row, side pictures go full width and a text screen (@grid) is scaled to fit.

/** The page-wide settings (lines starting with @), outside code blocks. */
internal data class MuSettings(
    val bg: String? = null, val fg: String? = null, val link: String? = null, val font: String? = null,
    val grid: Int = 0, val ticker: String? = null, val lock: Int = 0, val lockedMe: String? = null,
)

internal fun muSettings(markup: String): MuSettings {
    var s = MuSettings()
    var fence = false
    for (line in markup.replace("\r", "").split("\n")) {
        if (line.startsWith("```")) { fence = !fence; continue }
        if (fence) continue
        val m = Regex("""^@([a-z-]+)(?:\s+(.*))?$""").find(line) ?: continue
        val v = m.groupValues[2].trim()
        s = when (m.groupValues[1]) {
            "bg" -> s.copy(bg = v); "fg" -> s.copy(fg = v); "link" -> s.copy(link = v); "font" -> s.copy(font = v)
            "grid" -> s.copy(grid = Regex("""^(\d+)x\d+$""").find(v)?.groupValues?.get(1)?.toIntOrNull()?.coerceIn(20, 132) ?: 0)
            "ticker" -> s.copy(ticker = v)
            "lockinfo" -> s.copy(lock = v.toIntOrNull() ?: 1)
            "locked-notice" -> s.copy(lockedMe = v)
            else -> s
        }
    }
    return s
}

private val HEX = Regex("^#(?:[0-9a-fA-F]{3}|[0-9a-fA-F]{4}|[0-9a-fA-F]{6}|[0-9a-fA-F]{8})$")
private class Pal(val fgL: Long, val bgL: Long, val fgD: Long, val bgD: Long)
private val PAL = mapOf(
    "red" to Pal(0xFFB3261E, 0xFFFBDAD6, 0xFFFF8A80, 0xFF4A1D1A), "orange" to Pal(0xFFB55400, 0xFFFDE2C8, 0xFFFFB46B, 0xFF4A2C10),
    "yellow" to Pal(0xFF8A6A00, 0xFFFFF0A8, 0xFFFFD95A, 0xFF443A0E), "green" to Pal(0xFF2E7D32, 0xFFD6EFD8, 0xFF7FD68A, 0xFF1B3A1F),
    "blue" to Pal(0xFF1F5FBF, 0xFFDBE6FB, 0xFF8AB4FF, 0xFF1B2B4A), "purple" to Pal(0xFF7A3DB0, 0xFFEBDCF7, 0xFFCDA2FF, 0xFF34234A),
    "grey" to Pal(0xFF5F6B66, 0xFFE4E9E6, 0xFFA3AFA9, 0xFF2B3430), "cyan" to Pal(0xFF0A8A99, 0xFFD3F4F7, 0xFF5FD8E6, 0xFF0F3A40),
    "magenta" to Pal(0xFFB0278F, 0xFFF9D6EF, 0xFFF08AD6, 0xFF45163A), "white" to Pal(0xFFFFFFFF, 0xFFFFFFFF, 0xFFFFFFFF, 0xFFFFFFFF),
    "black" to Pal(0xFF000000, 0xFF000000, 0xFF000000, 0xFF000000),
)

private fun hexColour(s: String): Color? {
    if (!HEX.matches(s)) return null
    var h = s.substring(1)
    if (h.length <= 4) h = h.map { "$it$it" }.joinToString("")
    val v = h.toLong(16)
    return if (h.length == 8) Color(((v and 0xff) shl 24) or (v shr 8)) else Color(0xFF000000 or v)
}

/** How colours are read on this page: light or dark palette, the page's text and link colours. */
internal class MuCtx(val light: Boolean, val ink: Color, val link: Color, val font: FontFamily?, val base: Pages.Url,
                     val clock: String, val onAnchor: (String) -> Unit, val grid: Boolean) {
    fun fg(name: String?): Color? = name?.let { hexColour(it) ?: PAL[if (it == "gray") "grey" else it.lowercase()]?.let { p -> Color(if (light) p.fgL else p.fgD) } }
    fun bg(name: String?): Color? = name?.let { hexColour(it) ?: PAL[if (it == "gray") "grey" else it.lowercase()]?.let { p -> Color(if (light) p.bgL else p.bgD) } }
}

// ---------- blocks ----------

internal sealed class MuBlock {
    data class Para(val text: String, val centre: Boolean = false, val drop: Boolean = false) : MuBlock()
    data class Heading(val level: Int, val text: String, val centre: Boolean, val slug: String) : MuBlock()
    data class Item(val text: String) : MuBlock()
    data class Quote(val lines: List<String>, val colour: String?, val fx: String, val boxed: Boolean) : MuBlock()
    data class Code(val text: String) : MuBlock()
    data class Pic(val caption: String, val src: String, val words: String) : MuBlock()
    data class Table(val t: Block.Table) : MuBlock()
    data class Rule(val double: Boolean) : MuBlock()
    data class Cols(val parts: List<List<MuBlock>>) : MuBlock()
    data class Cards(val n: Int, val cards: List<Pair<Int, List<MuBlock>>>) : MuBlock()
    data class Board(val style: String, val path: String?, val notes: List<Triple<Long, String, String>>) : MuBlock()
    data class GridRow(val text: String) : MuBlock()
}

private val PIC = Regex("""^!\[([^\]]*)\]\(([^)\s]+)(?:\s+"([^"]*)")?\)\s*$""")
private val QUOTE = Regex("""^>(?:!([#a-zA-Z0-9]*)(?:\s*\[([a-z0-9 ,.\-]*)\])?(?=\s|$))?\s?(.*)$""")
private val CENTRE = Regex("""^->\s*(.*?)\s*<-\s*$""")
internal fun muSlug(s: String) = s.lowercase().replace(Regex("""<-|->|\{[^:{}]*:|[{}*`~]"""), "").trim()
    .replace(Regex("[^a-z0-9æøå]+"), "-").trim('-')

internal fun muParse(lines: List<String>, grid: Boolean): List<MuBlock> {
    val out = mutableListOf<MuBlock>()
    val para = StringBuilder()
    var table: MutableList<String>? = null
    var quote: MutableList<String>? = null
    var qColour: String? = null; var qFx = ""; var qBoxed = false
    var drop = false
    fun flush() {
        if (para.isNotEmpty()) { out += MuBlock.Para(para.toString(), drop = drop); drop = false; para.clear() }
        table?.let { out += MuBlock.Table(parseTable(it)); table = null }
        quote?.let { out += MuBlock.Quote(it, qColour, qFx, qBoxed); quote = null }
    }
    var i = 0
    var code: MutableList<String>? = null
    while (i < lines.size) {
        val line = lines[i]
        val c = code
        if (c != null) { if (line.startsWith("```")) { out += MuBlock.Code(c.joinToString("\n")); code = null } else c += line; i++; continue }
        // Columns and cards: `::: 3`, `::: cards 4` … `+++ [2]` … `:::` (code blocks inside are skipped).
        val blk = Regex("""^:::\s*(cards)?\s*(\d)?\s*(dashed)?\s*$""", RegexOption.IGNORE_CASE).find(line)
        if (blk != null && line.trim() != ":::") {
            flush()
            var depth = 1; var j = i + 1; var fence = false
            while (j < lines.size) {
                val t = lines[j].trim()
                if (t.startsWith("```")) fence = !fence
                else if (!fence && Regex("""^:::\s*\S""").containsMatchIn(t)) depth++
                else if (!fence && t == ":::") { depth--; if (depth == 0) break }
                j++
            }
            val inside = lines.subList(i + 1, minOf(j, lines.size))
            val cards = blk.groupValues[1].isNotEmpty()
            val parts = mutableListOf<Pair<Int, MutableList<String>>>(1 to mutableListOf())
            var fence2 = false
            for (l in inside) {
                if (l.trim().startsWith("```")) fence2 = !fence2
                val m = if (fence2) null else Regex("""^\+\+\+\s*(\d)?\s*$""").find(l.trim())
                if (m != null) {
                    if (parts.size == 1 && parts[0].second.all { it.isBlank() }) parts[0] = (m.groupValues[1].toIntOrNull() ?: 1) to mutableListOf()
                    else parts += (m.groupValues[1].toIntOrNull() ?: 1) to mutableListOf()
                } else parts.last().second += l
            }
            out += if (cards) MuBlock.Cards(blk.groupValues[2].toIntOrNull() ?: 3, parts.map { it.first to muParse(it.second, grid) })
            else MuBlock.Cols(parts.map { muParse(it.second, grid) })
            i = j + 1; continue
        }
        if (table != null && !line.trimStart().startsWith("|")) flush()
        when {
            line.startsWith("```") -> { flush(); code = mutableListOf() }
            Regex("""^@board(\s|$)""").containsMatchIn(line) -> {
                flush()
                val style = Regex("""\b(list|slips|table|ads)\b""").find(line)?.value ?: "list"
                var path: String? = null
                val notes = mutableListOf<Triple<Long, String, String>>()
                var j = i + 1
                while (j < lines.size && Regex("""^@(boardpath|note)\s""").containsMatchIn(lines[j])) {
                    Regex("""^@boardpath\s+(\S+)""").find(lines[j])?.let { path = it.groupValues[1] }
                    Regex("""^@note\s+(\d+)\t([^\t]*)\t([^\t]*)\t(.*)$""").find(lines[j])?.let {
                        notes += Triple(it.groupValues[1].toLong(), it.groupValues[2].ifEmpty { it.groupValues[3] }, it.groupValues[4])
                    }
                    j++
                }
                out += MuBlock.Board(style, path, notes)
                i = j; continue
            }
            Regex("""^@dropcap\s*$""").matches(line) -> { flush(); drop = true }
            Regex("""^@[a-z-]+(\s|$)""").containsMatchIn(line) -> {}
            grid -> {
                val p = PIC.find(line)
                if (p != null) out += MuBlock.Pic(p.groupValues[1], p.groupValues[2], p.groupValues[3]) else out += MuBlock.GridRow(line)
            }
            else -> {
                val h = Regex("""^(#{1,3})\s+(.*)$""").find(line)
                val p = PIC.find(line)
                val q = QUOTE.find(line)
                val ce = CENTRE.find(line)
                when {
                    h != null -> {
                        flush()
                        val hc = CENTRE.find(h.groupValues[2])
                        val t = hc?.groupValues?.get(1) ?: h.groupValues[2]
                        out += MuBlock.Heading(h.groupValues[1].length, t, hc != null, muSlug(t))
                    }
                    line.isBlank() -> flush()
                    Regex("""^===+\s*$""").matches(line) -> { flush(); out += MuBlock.Rule(true) }
                    Regex("""^---+\s*$""").matches(line) -> { flush(); out += MuBlock.Rule(false) }
                    p != null -> { flush(); out += MuBlock.Pic(p.groupValues[1], p.groupValues[2], p.groupValues[3]) }
                    line.trimStart().startsWith("|") -> { if (table == null) { flush(); table = mutableListOf() }; table!! += line }
                    Regex("""^\s*[-*]\s+(.*)$""").find(line) != null -> { flush(); out += MuBlock.Item(Regex("""^\s*[-*]\s+(.*)$""").find(line)!!.groupValues[1]) }
                    q != null && line.startsWith(">") -> {
                        val boxed = line.startsWith(">!")
                        if (quote == null || boxed) { flush(); quote = mutableListOf(); qColour = q.groupValues[1].ifEmpty { null }; qFx = q.groupValues[2]; qBoxed = boxed }
                        quote!! += q.groupValues[3]
                    }
                    ce != null -> { flush(); out += MuBlock.Para(ce.groupValues[1], centre = true) }
                    else -> {
                        quote?.let { out += MuBlock.Quote(it, qColour, qFx, qBoxed); quote = null }
                        if (para.isNotEmpty()) para.append(' ')
                        para.append(line.trim())
                    }
                }
            }
        }
        i++
    }
    code?.let { out += MuBlock.Code(it.joinToString("\n")) }
    flush()
    return out
}

// ---------- inline ----------

/** Widgets drawn in a line of text: (kind, colour, value). */
internal data class MuWidget(val kind: String, val colour: Color, val value: String)

private val SIMPLE = Regex("""`([^`]+)`|\*\*(.+?)\*\*|~~(.+?)~~|\*([^*\s](?:[^*]*[^*\s])?)\*|\[([^\]]+)\]\(([^)\s]+)\)""")

private fun closeBrace(t: String, i: Int): Int {
    var d = 0
    for (j in i until t.length) { if (t[j] == '{') d++ else if (t[j] == '}') { d--; if (d == 0) return j } }
    return -1
}

internal fun AnnotatedString.Builder.mu(text: String, c: MuCtx, widgets: MutableList<MuWidget>, depth: Int = 0) {
    if (depth > 6) { append(text); return }
    var last = 0
    fun simple(s: String) {
        var l = 0
        for (m in SIMPLE.findAll(s)) {
            append(s.substring(l, m.range.first))
            val g = m.groupValues
            when {
                g[1].isNotEmpty() -> withStyle(SpanStyle(fontFamily = FontFamily.Monospace, background = Color(0x22888888))) { append(g[1]) }
                g[2].isNotEmpty() -> withStyle(SpanStyle(fontWeight = FontWeight.Bold)) { mu(g[2], c, widgets, depth + 1) }
                g[3].isNotEmpty() -> withStyle(SpanStyle(textDecoration = TextDecoration.LineThrough)) { mu(g[3], c, widgets, depth + 1) }
                g[4].isNotEmpty() -> withStyle(SpanStyle(fontStyle = FontStyle.Italic)) { mu(g[4], c, widgets, depth + 1) }
                else -> link(g[5], g[6], c, widgets, depth)
            }
            l = m.range.last + 1
        }
        append(s.substring(l))
    }
    var i = 0
    while (i < text.length) {
        if (text[i] != '{' || text.substring(last, i).count { it == '`' } % 2 == 1) { i++; continue }
        val j = closeBrace(text, i)
        if (j < 0) break
        val body = text.substring(i + 1, j)
        // A brace we do not know stays as its text.
        if (!knownBrace(body)) { i++; continue }
        simple(text.substring(last, i))
        brace(body, c, widgets, depth)
        last = j + 1; i = j + 1
    }
    simple(text.substring(last))
}

private fun AnnotatedString.Builder.link(label: String, target: String, c: MuCtx, widgets: MutableList<MuWidget>, depth: Int) {
    val style = TextLinkStyles(SpanStyle(color = c.link, textDecoration = TextDecoration.Underline))
    if (target.startsWith("#")) { withLink(LinkAnnotation.Clickable(target, style) { c.onAnchor(muSlug(target.drop(1))) }) { mu(label, c, widgets, depth + 1) }; return }
    val p = Pages.parse(target, c.base)
    when {
        p != null -> withLink(LinkAnnotation.Clickable(p.toString(), style) { Pages.open(p.toString()) }) { mu(label, c, widgets, depth + 1) }
        Regex("^https?://", RegexOption.IGNORE_CASE).containsMatchIn(target) -> append("$label <$target>")
        else -> append(label)
    }
}

private val BRACE_NAMES = setOf("big", "huge", "small", "outline", "flash", "blink", "tilt", "right", "glow", "gradient", "marker", "hand", "stamp", "spark", "ring", "compass", "signal")
private fun knownBrace(body: String): Boolean {
    if (body == "clock" || body == "live") return true
    val k = body.indexOf(':'); if (k < 0) return false
    val head = body.substring(0, k).trim()
    if (!Regex("""^[!#a-zA-Z0-9 .\-]{1,60}$""").matches(head)) return false
    return head.split(Regex("\\s+"))[0].lowercase() in BRACE_NAMES || Regex("""^([#a-zA-Z0-9]*)(?:!([#a-zA-Z0-9]+))?$""").matches(head)
}

/** One {head:text}; false when it is not one we know (then it shows as text). */
private fun AnnotatedString.Builder.brace(body: String, c: MuCtx, widgets: MutableList<MuWidget>, depth: Int): Boolean {
    if (body == "clock") { withStyle(SpanStyle(fontFamily = FontFamily.Monospace)) { append(c.clock) }; return true }
    if (body == "live") { widgets += MuWidget("live", Color(0xFF2ECC71), ""); appendInlineContent("w${widgets.size - 1}", "●"); return true }
    val k = body.indexOf(':')
    if (k < 0) return false
    val head = body.substring(0, k).trim(); val txt = body.substring(k + 1)
    if (!Regex("""^[!#a-zA-Z0-9 .\-]{1,60}$""").matches(head)) return false
    val w = head.split(Regex("\\s+")); val name = w[0].lowercase()
    fun inner(st: SpanStyle) = withStyle(st) { mu(txt, c, widgets, depth + 1) }
    when (name) {
        "big" -> inner(SpanStyle(fontSize = 1.8.em, fontWeight = FontWeight.ExtraBold))
        "huge" -> inner(SpanStyle(fontSize = 2.6.em, fontWeight = FontWeight.Black))
        "small" -> inner(SpanStyle(fontSize = 0.78.em, letterSpacing = 0.04.em))
        "outline" -> inner(SpanStyle(drawStyle = Stroke(width = 3f)))
        "flash", "blink", "tilt", "right" -> inner(SpanStyle())
        "glow" -> { val col = c.fg(w.getOrNull(1)) ?: c.ink; inner(SpanStyle(color = col, shadow = Shadow(col, blurRadius = 18f))) }
        "gradient" -> { val a = c.fg(w.getOrNull(1)); val b = c.fg(w.getOrNull(2)); inner(if (a != null && b != null) SpanStyle(brush = Brush.horizontalGradient(listOf(a, b))) else SpanStyle()) }
        "marker" -> inner(SpanStyle(background = (c.fg(w.getOrNull(1)) ?: Color(0xFFFFE14D)).copy(alpha = 0.8f)))
        "hand" -> inner(SpanStyle(color = c.fg(w.getOrNull(1)) ?: Color.Unspecified, fontFamily = FontFamily.Cursive, fontStyle = FontStyle.Italic))
        "stamp" -> inner(SpanStyle(color = c.fg(w.getOrNull(1)) ?: Color(0xFFE0102F), fontWeight = FontWeight.Black, letterSpacing = 0.12.em))
        "spark", "ring", "compass", "signal" -> {
            val col = c.fg(w.getOrNull(1)) ?: c.ink
            widgets += MuWidget(name, col, txt.trim())
            appendInlineContent("w${widgets.size - 1}", "[$name]")
        }
        else -> {
            // Colours: {red:…} {!red:…} {#f80:…} {!#00a:…} {#fff!#00a:…}
            val cm = Regex("""^([#a-zA-Z0-9]*)(?:!([#a-zA-Z0-9]+))?$""").find(head) ?: return false
            val fg = cm.groupValues[1].takeIf { it.isNotEmpty() }?.let { c.fg(it) }
            val bg = cm.groupValues[2].takeIf { it.isNotEmpty() }?.let { c.bg(it) }
            if ((cm.groupValues[1].isNotEmpty() && fg == null) || (cm.groupValues[2].isNotEmpty() && bg == null)) { inner(SpanStyle()); return true }
            inner(SpanStyle(color = fg ?: Color.Unspecified, background = bg ?: Color.Unspecified))
        }
    }
    return true
}

/** The inline widgets as InlineTextContent, keyed w0, w1, … */
private fun widgetContent(widgets: List<MuWidget>): Map<String, InlineTextContent> = widgets.mapIndexed { i, w ->
    val (pw, ph) = when (w.kind) { "spark" -> 6.em to 1.4.em; "ring" -> 2.4.em to 2.4.em; "compass" -> 2.em to 2.em; "signal" -> 1.5.em to 1.em; else -> 0.7.em to 0.7.em }
    "w$i" to InlineTextContent(Placeholder(pw, ph, PlaceholderVerticalAlign.TextCenter)) { MuWidgetView(w) }
}.toMap()

@Composable
private fun MuWidgetView(w: MuWidget) {
    when (w.kind) {
        "spark" -> Canvas(Modifier.fillMaxSize()) {
            val pts = w.value.split(Regex("[\\s,]+")).mapNotNull { it.toFloatOrNull() }.take(60)
            if (pts.size < 2) return@Canvas
            val mx = pts.max(); val mn = pts.min(); val r = (mx - mn).takeIf { it > 0 } ?: 1f
            val path = Path()
            pts.forEachIndexed { k, v -> val x = k / (pts.size - 1f) * size.width; val y = size.height - 2f - (v - mn) / r * (size.height - 4f); if (k == 0) path.moveTo(x, y) else path.lineTo(x, y) }
            val fill = Path().apply { addPath(path); lineTo(size.width, size.height); lineTo(0f, size.height); close() }
            drawPath(fill, w.colour.copy(alpha = 0.18f)); drawPath(path, w.colour, style = Stroke(width = 4f))
        }
        "ring" -> Canvas(Modifier.fillMaxSize()) {
            val pct = (w.value.toFloatOrNull() ?: 0f).coerceIn(0f, 100f)
            val sw = size.minDimension * 0.12f
            drawArc(w.colour.copy(alpha = 0.18f), 0f, 360f, false, style = Stroke(sw), topLeft = Offset(sw / 2, sw / 2), size = Size(size.width - sw, size.height - sw))
            drawArc(w.colour, -90f, pct * 3.6f, false, style = Stroke(sw), topLeft = Offset(sw / 2, sw / 2), size = Size(size.width - sw, size.height - sw))
        }
        "compass" -> Canvas(Modifier.fillMaxSize()) {
            val deg = (w.value.toFloatOrNull() ?: 0f)
            drawCircle(w.colour.copy(alpha = 0.4f), style = Stroke(3f))
            drawRotate(deg) { drawPath(Path().apply { moveTo(center.x, size.height * 0.1f); lineTo(center.x + size.width * 0.09f, center.y); lineTo(center.x - size.width * 0.09f, center.y); close() }, w.colour) }
        }
        "signal" -> Canvas(Modifier.fillMaxSize()) {
            val n = (w.value.toFloatOrNull() ?: 0f).toInt().coerceIn(0, 5)
            val bw = size.width / 7f
            for (k in 0 until 5) { val h = size.height * (0.3f + k * 0.175f); drawRect(if (k < n) Color(0xFF2ECC71) else w.colour.copy(alpha = 0.2f), Offset(k * bw * 1.4f, size.height - h), Size(bw, h)) }
        }
        else -> Canvas(Modifier.fillMaxSize()) { drawCircle(w.colour) }
    }
}

/** A line of MU: text, links, colours, effects and widgets. A line that is one {tilt N:…} is turned. */
@Composable
internal fun MuText(text: String, c: MuCtx, style: TextStyle, modifier: Modifier = Modifier, align: TextAlign? = null, bold: Boolean = false) {
    // {right:…} at the top level: the rest left, that part right.
    val right = Regex("""\{right:((?:[^{}]|\{[^{}]*(?:\{[^{}]*\}[^{}]*)*\})*)\}""").find(text)
    if (right != null) {
        // Both sides share the width; on a narrow screen each wraps on its own side.
        Row(modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            MuText(text.removeRange(right.range).trim(), c, style, Modifier.weight(1f))
            MuText(right.groupValues[1], c, style, Modifier.weight(1f), align = TextAlign.End)
        }
        return
    }
    val tilt = Regex("""^\{tilt (-?[\d.]+):(.*)\}$""").find(text.trim())
    val widgets = mutableListOf<MuWidget>()
    val s = buildAnnotatedString { mu(text, c, widgets) }
    Text(s, modifier.then(if (tilt != null) Modifier.padding(vertical = 6.dp).rotate(tilt.groupValues[1].toFloatOrNull()?.coerceIn(-20f, 20f) ?: 0f) else Modifier),
        // Line height follows the biggest letters in the line ({big:}, {huge:}), never a fixed one.
        style = style.copy(lineHeight = TextUnit.Unspecified), color = c.ink, fontFamily = c.font, fontWeight = if (bold) FontWeight.SemiBold else null, textAlign = align,
        inlineContent = widgetContent(widgets))
}

// ---------- the page ----------

private fun muBrush(bg: String?, c: (String) -> Color?): Brush? {
    val w = bg?.split(Regex("\\s+"))?.filter { it.isNotEmpty() } ?: return null
    if (w.isEmpty()) return null
    val parts = w[0].split("..")
    val a = c(parts[0]) ?: return null
    val b = parts.getOrNull(1)?.let(c)
    return if (b != null) Brush.verticalGradient(listOf(a, b)) else Brush.verticalGradient(listOf(a, a))
}

/** A whole page in MU. */
@Composable
internal fun MuPage(markup: String, base: Pages.Url, version: Int, scroll: ScrollState) {
    Pages.version.value
    val st = remember(markup) { muSettings(markup) }
    val blocks = remember(markup) { muParse(markup.replace("\r", "").split("\n"), st.grid > 0) }
    var clock by remember { mutableStateOf(java.text.SimpleDateFormat("HH:mm:ss", java.util.Locale.ROOT).format(java.util.Date())) }
    LaunchedEffect(markup) { while (true) { delay(1000); clock = java.text.SimpleDateFormat("HH:mm:ss", java.util.Locale.ROOT).format(java.util.Date()) } }
    val themeInk = MaterialTheme.colorScheme.onSurface
    val themeLight = MaterialTheme.colorScheme.surface.luminance() >= 0.5f
    val bgFirst = st.bg?.split(Regex("[\\s.]+"))?.firstOrNull()?.let { hexColour(it) }
    val light = bgFirst?.let { it.luminance() > 0.5f } ?: themeLight
    val ink = st.fg?.let { hexColour(it) } ?: themeInk
    val link = st.link?.let { hexColour(it) } ?: MaterialTheme.colorScheme.primary
    val font = when (st.font) { "serif" -> FontFamily.Serif; "sans" -> FontFamily.SansSerif; "mono" -> FontFamily.Monospace; else -> null }
        ?: if (st.grid > 0) FontFamily.Monospace else null
    val anchors = remember(markup) { HashMap<String, Float>() }
    var top by remember { mutableStateOf(0f) }
    val scope = rememberCoroutineScope()
    val ctx = MuCtx(light, ink, link, font, base, clock, { slug -> anchors[slug]?.let { y -> scope.launch { scroll.animateScrollTo((scroll.value + y - top).toInt().coerceAtLeast(0)) } } }, st.grid > 0)
    val brush = muBrush(st.bg) { hexColour(it) }
    CompositionLocalProvider(LocalContentColor provides ink) {
        Column(Modifier.fillMaxWidth().then(if (brush != null) Modifier.background(brush, RoundedCornerShape(8.dp)) else Modifier)
            .onGloballyPositioned { top = it.boundsInRoot().top - scroll.value }
            .padding(if (brush != null) 12.dp else 0.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            st.ticker?.let { t ->
                Box(Modifier.fillMaxWidth().background(Color(if (light) 0xFFFBDAD6 else 0xFF4A1D1A)).padding(vertical = 4.dp)) {
                    MuText(t, ctx, MaterialTheme.typography.bodyMedium.copy(fontWeight = FontWeight.Bold), Modifier.basicMarquee(iterations = Int.MAX_VALUE))
                }
            }
            if (st.grid > 0) MuGrid(blocks, ctx, st.grid, version) else MuBlocks(blocks, ctx, version, anchors)
            st.lockedMe?.takeIf { Regex("^[0-9a-f]{32}$").matches(it) }?.let { me ->
                val cb = LocalClipboardManager.current
                OutlinedButton(onClick = { cb.setText(AnnotatedString(me)) }) { Text("Copy my address") }
            }
        }
    }
}

/** A text screen (`@grid 80x25`): every row keeps its spaces; the font shrinks so the row fits the width. */
@Composable
private fun MuGrid(blocks: List<MuBlock>, c: MuCtx, cols: Int, version: Int) {
    BoxWithConstraints(Modifier.fillMaxWidth()) {
        val density = LocalDensity.current
        val px = with(density) { maxWidth.toPx() }
        val sp = with(density) { (px / (cols * 0.6f)).toSp().value }.coerceIn(4f, 18f)
        val style = TextStyle(fontFamily = FontFamily.Monospace, fontSize = TextUnit(sp, TextUnitType.Sp), lineHeight = TextUnit(sp * 1.18f, TextUnitType.Sp))
        Column {
            for (b in blocks) when (b) {
                is MuBlock.GridRow -> MuText(b.text.ifEmpty { " " }, c, style.copy(), Modifier, null)
                is MuBlock.Board -> MuBoard(b, c)
                is MuBlock.Pic -> Picture(b.caption, b.src, picStyle(b.words), c.base, version, Regex("""\b(grey|dots|colour|color)!""").containsMatchIn(b.words.lowercase()), b.words)
                else -> {}
            }
        }
    }
}

private fun picStyle(words: String) = Regex("""\b(grey|dots|colour|color)!?""").find(words.lowercase())?.groupValues?.get(1) ?: "grey"

@Composable
private fun MuBlocks(blocks: List<MuBlock>, c: MuCtx, version: Int, anchors: HashMap<String, Float>) {
    val body = MaterialTheme.typography.bodyLarge
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        for (b in blocks) when (b) {
            is MuBlock.Para -> if (b.drop && b.text.length > 1) {
                Row { MuText(b.text.take(1), c, body.copy(fontSize = 3.em, fontWeight = FontWeight.ExtraBold, lineHeight = 1.em)); MuText(b.text.drop(1), c, body, Modifier.padding(start = 4.dp)) }
            } else MuText(b.text, c, body, if (b.centre) Modifier.fillMaxWidth() else Modifier, if (b.centre) TextAlign.Center else null)
            is MuBlock.Heading -> MuText(b.text, c, when (b.level) { 1 -> MaterialTheme.typography.headlineSmall; 2 -> MaterialTheme.typography.titleLarge; else -> MaterialTheme.typography.titleMedium },
                Modifier.then(if (b.centre) Modifier.fillMaxWidth() else Modifier).onGloballyPositioned { anchors[b.slug] = it.boundsInRoot().top },
                if (b.centre) TextAlign.Center else null, bold = true)
            is MuBlock.Item -> Row { Text("•  ", color = c.ink); MuText(b.text, c, body) }
            is MuBlock.Quote -> MuQuote(b, c)
            is MuBlock.Table -> PageTable(b.t, c.base) { t, head, align -> MuText(t, c, MaterialTheme.typography.bodyMedium, Modifier.fillMaxWidth(), align, bold = head) }
            is MuBlock.Code -> MuCode(b.text, c)
            is MuBlock.Pic -> Picture(b.caption, b.src, picStyle(b.words), c.base, version, Regex("""\b(grey|dots|colour|color)!""").containsMatchIn(b.words.lowercase()), b.words)
            is MuBlock.Rule -> if (b.double) Column(verticalArrangement = Arrangement.spacedBy(2.dp)) { Box(Modifier.fillMaxWidth().size(1.dp).background(c.ink)); Box(Modifier.fillMaxWidth().size(1.dp).background(c.ink)) }
                else Box(Modifier.fillMaxWidth().padding(vertical = 4.dp).size(1.dp).background(c.ink.copy(alpha = 0.35f)))
            // A phone: columns become one column.
            is MuBlock.Cols -> for (p in b.parts) MuBlocks(p, c, version, anchors)
            is MuBlock.Cards -> MuCards(b, c, version, anchors)
            is MuBlock.Board -> MuBoard(b, c)
            is MuBlock.GridRow -> MuText(b.text, c, body)
        }
    }
}

@Composable
private fun MuQuote(b: MuBlock.Quote, c: MuCtx) {
    val text = b.lines.joinToString("\n")
    val body = MaterialTheme.typography.bodyLarge
    val fx = b.fx.split(',').map { it.trim().split(Regex("\\s+")) }
    val tilt = fx.firstOrNull { it[0] == "tilt" }?.getOrNull(1)?.toFloatOrNull()?.coerceIn(-10f, 10f) ?: 0f
    val shadow = fx.any { it[0] == "shadow" }
    val rules = fx.any { it[0] == "rules" }
    val frame = fx.any { it[0] == "frame" }
    val bar = c.fg(b.colour) ?: c.ink.copy(alpha = 0.4f)
    val fill = if (b.boxed) c.bg(b.colour) else null
    var m = Modifier.fillMaxWidth().padding(vertical = 2.dp).rotate(tilt)
    m = when {
        rules -> m.drawBehind { drawRect(c.ink, size = Size(size.width, 3.dp.toPx())); drawRect(c.ink, Offset(0f, size.height - 3.dp.toPx()), Size(size.width, 3.dp.toPx())) }.padding(vertical = 10.dp)
        frame -> m.border(1.dp, c.ink).padding(10.dp)
        shadow -> m.offset(0.dp, 0.dp).shadow(0.dp).drawBehind { drawRect(Color(0xFF111111), Offset(6.dp.toPx(), 6.dp.toPx()), size) }.background(fill ?: Color.Transparent).border(2.dp, Color(0xFF111111)).padding(10.dp)
        fill != null -> m.background(fill, RoundedCornerShape(topEnd = 6.dp, bottomEnd = 6.dp)).drawBehind { drawRect(bar, size = Size(3.dp.toPx(), size.height)) }.padding(start = 13.dp, end = 10.dp, top = 6.dp, bottom = 6.dp)
        else -> m.drawBehind { drawRect(bar, size = Size(3.dp.toPx(), size.height)) }.padding(start = 12.dp, top = 4.dp, bottom = 4.dp)
    }
    Box(m) { MuText(text, c, body, Modifier.fillMaxWidth(), if (rules) TextAlign.Center else null) }
}

@Composable
private fun MuCode(t: String, c: MuCtx) {
    val cb = LocalClipboardManager.current
    var copied by remember { mutableStateOf(false) }
    Box {
        CodeBlock(t)
        Text(if (copied) "copied" else "copy", style = MaterialTheme.typography.labelSmall, color = c.ink.copy(alpha = 0.7f),
            modifier = Modifier.align(androidx.compose.ui.Alignment.TopEnd).padding(4.dp)
                .border(1.dp, c.ink.copy(alpha = 0.3f), RoundedCornerShape(4.dp))
                .clickable { cb.setText(AnnotatedString(t)); copied = true }.padding(horizontal = 6.dp, vertical = 1.dp))
    }
}

/** Cards: two to a row on a phone; a card two or more wide takes the whole row. */
@Composable
private fun MuCards(b: MuBlock.Cards, c: MuCtx, version: Int, anchors: HashMap<String, Float>) {
    val per = minOf(b.n, 2)
    val rows = mutableListOf<MutableList<Pair<Int, List<MuBlock>>>>()
    for (card in b.cards) {
        val wide = card.first >= per
        if (wide || rows.isEmpty() || rows.last().size >= per || rows.last().any { it.first >= per }) rows += mutableListOf(card) else rows.last() += card
    }
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        for (r in rows) Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            for (card in r) Box(Modifier.weight(1f).background(c.ink.copy(alpha = 0.06f), RoundedCornerShape(12.dp))
                .border(1.dp, c.ink.copy(alpha = 0.18f), RoundedCornerShape(12.dp)).padding(10.dp)) { MuBlocks(card.second, c, version, anchors) }
            if (r.size < per && r.none { it.first >= per }) Box(Modifier.weight(1f)) {}
        }
    }
}

/** A notice board: the newest notes (plain text) and a line to write one. */
@Composable
private fun MuBoard(b: MuBlock.Board, c: MuCtx) {
    val small = MaterialTheme.typography.bodySmall
    val fmt = remember { java.text.SimpleDateFormat("d MMM HH:mm", java.util.Locale.ENGLISH) }
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        if (b.notes.isEmpty()) Text("No notes yet. Be the first.", style = small, color = c.ink.copy(alpha = 0.6f))
        for ((at, name, text) in b.notes) {
            val slip = b.style == "slips"
            Column(Modifier.fillMaxWidth().then(if (slip) Modifier.background(Color.White).padding(8.dp) else Modifier.padding(vertical = 2.dp))) {
                Text(buildAnnotatedString { withStyle(SpanStyle(fontWeight = FontWeight.Bold)) { append(name); append("  ") }; append(text) },
                    color = if (slip) Color(0xFF111111) else c.ink, fontFamily = if (slip) FontFamily.Monospace else c.font, style = MaterialTheme.typography.bodyMedium)
                Text(fmt.format(java.util.Date(at * 1000)), style = small, color = (if (slip) Color(0xFF111111) else c.ink).copy(alpha = 0.55f))
            }
        }
        val path = b.path ?: return@Column
        var note by remember { mutableStateOf("") }
        var msg by remember { mutableStateOf("") }
        Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            // In the page's own colours (a light paper page has dark text).
            OutlinedTextField(note, { if (it.toByteArray().size <= 100) note = it }, Modifier.weight(1f),
                placeholder = { Text("Write a note (100 characters)", color = c.ink.copy(alpha = 0.5f)) }, singleLine = true,
                colors = androidx.compose.material3.OutlinedTextFieldDefaults.colors(focusedTextColor = c.ink, unfocusedTextColor = c.ink,
                    focusedBorderColor = c.ink, unfocusedBorderColor = c.ink.copy(alpha = 0.5f), cursorColor = c.ink))
            OutlinedButton(onClick = {
                val t = note.trim()
                if (t.isNotEmpty() && Node.send(Cmd.postBoard(Node.req(), c.base.addr, path, t))) {
                    note = ""; msg = "Sent. The page reloads in a moment…"
                    android.os.Handler(android.os.Looper.getMainLooper()).postDelayed({ Pages.reload() }, 6000)
                } else if (t.isNotEmpty()) msg = "Not connected"
            }) { Text("Post") }
        }
        if (msg.isNotEmpty()) Text(msg, style = small, color = c.ink.copy(alpha = 0.6f))
    }
}


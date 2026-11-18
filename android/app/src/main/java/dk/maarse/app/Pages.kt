package dk.maarse.app

import kotlinx.coroutines.flow.MutableStateFlow

/**
 * maarse:// pages: the same model as the web client. A page is Må:rse markup
 * sent as an ordinary file (raw deflate, MIME [PAGE_MIME], the path as name).
 * A picture on a page (`![caption](file.jpg "grey")`) is asked for like a
 * page with the style as a first path segment (`_grey/file.jpg`); over LoRa
 * the host sends a small copy (grey, dots or colour), over the internet the
 * original.
 */
object Pages {
    const val PAGE_MIME = "text/x-maarse-page;deflate"

    /** A page, or a picture on a page (`_style/file`): Pages only, never a chat. */
    fun isPageFile(mime: String, name: String) = mime == PAGE_MIME || (name.startsWith("_") && '/' in name)

    data class Url(val addr: String, val path: String) {
        override fun toString() = "maarse://$addr/" + if (path == "index") "" else path
        val key get() = "$addr/$path"
    }

    sealed class Pic {
        data class Got(val data: ByteArray, val mime: String) : Pic()
        data class Failed(val why: String) : Pic()
        /** Asked for; nothing has come yet. */
        data object Asking : Pic()
        /** Coming: `percent` done of `total` bytes. */
        data class Busy(val percent: Int, val total: Int) : Pic()
    }

    private class InFile(val key: String, val pic: Boolean, val mime: String, val buf: ByteArray)

    /** Bumped on every change, so the screen redraws. */
    val version = MutableStateFlow(0)
    var url: Url? = null; private set
    var status = ""; private set
    val history = ArrayDeque<Url>()
    /** Pages to go forward to (after Back), like a browser. */
    val forwardStack = ArrayDeque<Url>()
    /** The current page is being fetched (the bar under the toolbar moves). */
    var loading = false; private set
    /** Size (compressed) and time of the page shown. */
    var pageBytes = 0; private set
    var pageAt = 0L; private set
    /** Recently visited pages, newest first (url, title). */
    val recent = ArrayList<Pair<Url, String>>()
    private val cache = LinkedHashMap<String, String>()
    private val pics = HashMap<String, Pic>()
    private val waits = HashSet<String>()
    private val files = HashMap<Long, InFile>()
    /** Nodes reached over LoRa (pictures then wait for a tap). */
    private val lora = HashMap<String, Boolean>()

    private fun bump() { version.value++ }

    fun parse(s: String, base: Url? = null): Url? {
        val t = s.trim()
        if (t.startsWith("/") && base != null) return Url(base.addr, t.trim('/').ifEmpty { "index" })
        val m = Regex("^([0-9a-fA-F]{32})(/.*)?$").find(t.removePrefix("maarse://").removePrefix("MAARSE://"))
        if (m == null && base != null && Regex("^[a-z0-9_][a-z0-9_./-]*$").matches(t)) return Url(base.addr, t.trimEnd('/').ifEmpty { "index" })
        if (m == null) return null
        return Url(m.groupValues[1].lowercase(), m.groupValues[2].trim('/').ifEmpty { "index" })
    }

    /** Nodes whose announces say they share pages (CAP_PAGES 0x20). */
    val sharing: LinkedHashSet<String> by lazy { LinkedHashSet(runCatching { Node.prefs.pageNodes }.getOrDefault(emptyList())) }
    fun noteCaps(addr: String, caps: Int) { if (caps and 0x20 != 0) noteSharing(addr) }
    private fun noteSharing(addr: String) {
        if (sharing.add(addr)) { runCatching { Node.prefs.pageNodes = sharing.toList() }; bump() }
    }

    /** Over LoRa pictures wait for a tap. A route not known yet counts as LoRa
     *  until the page has come; then it is asked again. */
    private fun askRoute(addr: String) = Node.queryFileLimit(addr) { l -> lora[addr] = l == null || l.viaLora || l.mailbox; bump() }

    fun markup(u: Url): String? = cache[u.key]

    /** A page's title, like <title> on a web page: a line "@title …" (not
     *  shown on the page), else the first heading. */
    fun title(markup: String): String =
        (Regex("(?m)^@title\\s+(.+)$").find(markup) ?: Regex("(?m)^#{1,3}\\s+(.+)$").find(markup))
            ?.groupValues?.get(1)
            // Markup is dropped: `-> x <-` centring, {red:…} colours, *, ~ and backticks.
            ?.replace(Regex("""^->\s*(.*?)\s*<-\s*$"""), "$1")?.replace(Regex("""\{!?[a-z]+:([^{}]+)\}"""), "$1")
            ?.replace(Regex("[*`~]"), "")?.trim()?.take(80) ?: ""
    /** The title of a node's front page, if we have it. */
    fun siteTitle(addr: String): String = cache["$addr/index"]?.let { title(it) } ?: ""

    /** Favourite users ("u") and pages ("p"), kept in the app's settings. */
    val favUsers: MutableList<String> by lazy { runCatching { Node.prefs.favUsers.toMutableList() }.getOrDefault(mutableListOf()) }
    val favPages: MutableList<String> by lazy { runCatching { Node.prefs.favPages.toMutableList() }.getOrDefault(mutableListOf()) }
    fun isFav(kind: Char, addr: String) = (if (kind == 'u') favUsers else favPages).contains(addr)
    fun toggleFav(kind: Char, addr: String) {
        val l = if (kind == 'u') favUsers else favPages
        if (!l.remove(addr)) l.add(addr)
        runCatching { if (kind == 'u') Node.prefs.favUsers = favUsers else Node.prefs.favPages = favPages }
        bump()
    }
    fun picture(key: String): Pic? = pics[key]
    fun overLora(addr: String) = lora[addr] != false

    fun open(input: String, push: Boolean = true) {
        val u = parse(input, url) ?: run { status = "Invalid address. Write maarse://<32 hex characters>/path"; bump(); return }
        url?.let { if (push && it != u) { history.addLast(it); forwardStack.clear() } }
        url = u
        loading = true
        // No answer in a minute: stop the bar and say so.
        android.os.Handler(android.os.Looper.getMainLooper()).postDelayed({
            if (loading && url == u && u.key in waits) { loading = false; status = "No answer from the node (is it online, and does it have pages?)"; bump() }
        }, 60_000)
        if (!lora.containsKey(u.addr)) askRoute(u.addr)
        status = if (cache.containsKey(u.key)) "From this session's copy – fetching a new one…" else "Fetching…"
        waits.add(u.key)
        if (!Node.send(Cmd.requestPage(Node.req(), u.addr, u.path))) status = "Not connected"
        bump()
    }

    fun back() { history.removeLastOrNull()?.let { u -> url?.let { forwardStack.addLast(it) }; open(u.toString(), push = false) } }
    fun forward() { forwardStack.removeLastOrNull()?.let { u -> url?.let { history.addLast(it) }; open(u.toString(), push = false) } }
    fun reload() { url?.let { open(it.toString(), push = false) } }
    /** The browser's start page (bookmarks, nodes sharing pages, recent). */
    fun home() { url?.let { history.addLast(it); forwardStack.clear() }; url = null; loading = false; status = ""; bump() }

    /** Pictures waiting for the one before them (one at a time per node). */
    private val queue = ArrayDeque<Pair<String, String>>()

    private fun busy(addr: String) = waits.any { it.startsWith("$addr/_") } || files.values.any { it.pic && it.key.startsWith("$addr/") }

    fun queued(addr: String, path: String) = queue.contains(addr to path)

    private fun next() {
        val q = queue.firstOrNull { !busy(it.first) } ?: return
        queue.remove(q)
        requestPicture(q.first, q.second)
    }

    /** Asks for a picture; `path` is `_style/file`. One at a time per node:
     *  several big files to one recipient at once overfill the host's send
     *  queue, and over LoRa they would only share the air. */
    fun requestPicture(addr: String, path: String) {
        val key = "$addr/$path"
        if (key in waits) return
        if (busy(addr)) { if (!queued(addr, path)) queue.addLast(addr to path); bump(); return }
        waits.add(key)
        pics[key] = Pic.Asking
        if (!Node.send(Cmd.requestPage(Node.req(), addr, path))) { waits.remove(key); pics[key] = Pic.Failed("not connected") }
        // No answer in two minutes: say so, so it can be tried again.
        android.os.Handler(android.os.Looper.getMainLooper()).postDelayed({
            if (key in waits) { waits.remove(key); pics[key] = Pic.Failed("no answer"); next(); bump() }
        }, 120_000)
        bump()
    }

    /** A file is coming: ours if we asked for exactly this from exactly that node. */
    fun onIncoming(ev: Ev.FileIncoming): Boolean {
        val key = "${ev.from}/${ev.name}"
        if (key !in waits) return false
        val pic = ev.name.startsWith("_")
        if (!pic && ev.mime != PAGE_MIME) return false
        if (pic && ev.mime != PAGE_MIME && !ev.mime.startsWith("image/")) return false
        if (ev.size <= 0 || ev.size > 2 * 1024 * 1024) return false
        waits.remove(key)
        files[ev.fileId] = InFile(key, pic, ev.mime, ByteArray(ev.size))
        if (pic) pics[key] = Pic.Busy(0, ev.size)
        bump()
        return true
    }

    fun onChunk(ev: Ev.FileChunk): Boolean {
        val f = files[ev.fileId] ?: return false
        val n = minOf(ev.data.size, f.buf.size - ev.offset)
        if (ev.offset >= 0 && n > 0) System.arraycopy(ev.data, 0, f.buf, ev.offset, n)
        return true
    }

    fun onProgress(ev: Ev.FileProgress): Boolean {
        val f = files[ev.fileId] ?: return false
        val pct = 100 * ev.done / maxOf(1, ev.total)
        if (f.pic) pics[f.key] = Pic.Busy(pct, f.buf.size) else if (url?.key == f.key) status = "Receiving… $pct %"
        bump()
        return true
    }

    fun onDone(ev: Ev.FileDone): Boolean {
        val f = files.remove(ev.fileId) ?: return false
        when {
            !ev.ok && f.pic -> pics[f.key] = Pic.Failed(ev.text)
            !ev.ok -> { status = "Error: ${ev.text}"; if (url?.key == f.key) loading = false }
            f.pic && f.mime == PAGE_MIME -> pics[f.key] = Pic.Failed("missing on the node")
            f.pic -> pics[f.key] = Pic.Got(f.buf, f.mime)
            else -> runCatching { inflate(f.buf) }.onSuccess { m ->
                cache[f.key] = m
                while (cache.size > 30) cache.remove(cache.keys.first())
                val from = f.key.substringBefore('/')
                noteSharing(from)
                // Asked now and again in a moment: the route (a DHT relay,
                // say) is often learned just after the page has come.
                askRoute(from)
                android.os.Handler(android.os.Looper.getMainLooper()).postDelayed({ askRoute(from) }, 5000)
                if (url?.key == f.key) {
                    status = ""; loading = false; pageBytes = f.buf.size; pageAt = System.currentTimeMillis()
                    url?.let { u -> recent.removeAll { it.first == u }; recent.add(0, u to title(m)); while (recent.size > 20) recent.removeAt(recent.lastIndex) }
                }
            }.onFailure { status = "Could not unpack the page" }
        }
        if (f.pic) next()
        bump()
        return true
    }

    private fun inflate(b: ByteArray): String {
        val inf = java.util.zip.Inflater(true)
        inf.setInput(b)
        val out = java.io.ByteArrayOutputStream()
        val buf = ByteArray(4096)
        while (!inf.finished()) {
            val n = inf.inflate(buf)
            if (n == 0 && (inf.needsInput() || inf.needsDictionary())) break
            out.write(buf, 0, n)
            if (out.size() > 200_000) error("too large")
        }
        inf.end()
        return out.toString(Charsets.UTF_8.name())
    }
}

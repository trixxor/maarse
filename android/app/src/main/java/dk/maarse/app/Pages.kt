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

    data class Url(val addr: String, val path: String) {
        override fun toString() = "maarse://$addr/" + if (path == "index") "" else path
        val key get() = "$addr/$path"
    }

    sealed class Pic {
        data class Got(val data: ByteArray, val mime: String) : Pic()
        data class Failed(val why: String) : Pic()
        data class Busy(val percent: Int) : Pic()
    }

    private class InFile(val key: String, val pic: Boolean, val mime: String, val buf: ByteArray)

    /** Bumped on every change, so the screen redraws. */
    val version = MutableStateFlow(0)
    var url: Url? = null; private set
    var status = ""; private set
    val history = ArrayDeque<Url>()
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
    fun picture(key: String): Pic? = pics[key]
    fun overLora(addr: String) = lora[addr] != false

    fun open(input: String, push: Boolean = true) {
        val u = parse(input, url) ?: run { status = "Invalid address. Write maarse://<32 hex characters>/path"; bump(); return }
        url?.let { if (push && it != u) history.addLast(it) }
        url = u
        if (!lora.containsKey(u.addr)) askRoute(u.addr)
        status = if (cache.containsKey(u.key)) "From this session's copy – fetching a new one…" else "Fetching…"
        waits.add(u.key)
        if (!Node.send(Cmd.requestPage(Node.req(), u.addr, u.path))) status = "Not connected"
        bump()
    }

    fun back() { history.removeLastOrNull()?.let { open(it.toString(), push = false) } }

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
        pics[key] = Pic.Busy(0)
        if (!Node.send(Cmd.requestPage(Node.req(), addr, path))) pics[key] = Pic.Failed("not connected")
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
        if (f.pic) pics[f.key] = Pic.Busy(pct) else if (url?.key == f.key) status = "Receiving… $pct %"
        bump()
        return true
    }

    fun onDone(ev: Ev.FileDone): Boolean {
        val f = files.remove(ev.fileId) ?: return false
        when {
            !ev.ok && f.pic -> pics[f.key] = Pic.Failed(ev.text)
            !ev.ok -> status = "Error: ${ev.text}"
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
                if (url?.key == f.key) status = "Fetched (${f.buf.size} bytes compressed)"
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

package dk.maarse.app

import android.content.Context
import android.os.Handler
import android.os.Looper
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

/** Connection state shown at the top of every screen. */
data class Conn(val mode: Mode, val state: State, val text: String) {
    enum class State { OFF, CONNECTING, ONLINE }
}

/** What the node has told us about itself. */
data class Me(val addr: String, val maxText: Int, val maxFile: Long, val repeater: Boolean, val version: Int, val name: String = "")

/**
 * The app's state and the one connection to a node. Lives as long as the
 * app process; [NodeService] keeps the process alive in the background.
 * All calls on the main thread.
 */
object Node : LinkListener {
    private lateinit var app: Context
    lateinit var db: Db; private set
    lateinit var prefs: Prefs; private set
    private val main = Handler(Looper.getMainLooper())
    private var link: Link? = null
    private var nextReq = 1
    private var retryMs = 2000L
    private val pending = HashMap<Int, Long>() // request -> message row
    private var histReq = 0x7000

    val conn = MutableStateFlow(Conn(Mode.BLE, Conn.State.OFF, "Not connected"))
    val me = MutableStateFlow<Me?>(null)
    val channels = MutableStateFlow<Map<String, String>>(emptyMap())
    val nodes = MutableStateFlow<Map<String, NodeEntry>>(emptyMap())
    /** Every announce passed on to us (Heard), newest first: (address, hops, time). */
    val announces = MutableStateFlow<List<Triple<String, Int, Long>>>(emptyList())
    private var infoAt = 0L
    val nicknames = MutableStateFlow<Map<String, String>>(emptyMap())
    /** Our node's public key and the contacts' keys (hex), for safety numbers. */
    val myKey = MutableStateFlow<String?>(null)
    val keys = MutableStateFlow<Map<String, String>>(emptyMap())
    /** Verified contacts: address -> the key that was verified. */
    val verified = MutableStateFlow<Map<String, String>>(emptyMap())
    val stats = MutableStateFlow<Map<Int, Int>>(emptyMap())
    val radio = MutableStateFlow<RadioCfg?>(null)
    val regions = MutableStateFlow<Ev.Regions?>(null)
    val sharing = MutableStateFlow<Ev.Sharing?>(null)
    /** Site of the node (fixed nodes: antenna height, gain, owner). */
    val site = MutableStateFlow<Ev.Site?>(null)
    val neighbors = MutableStateFlow<List<Neighbor>>(emptyList())
    val alerts = MutableStateFlow<List<Ev.Alert>>(emptyList())
    val dismissed = MutableStateFlow<Set<String>>(emptySet())
    val traces = MutableStateFlow<List<Ev.Trace>>(emptyList())
    val log = MutableStateFlow<List<String>>(emptyList())
    /** Linked devices: the person this node belongs to (hosts only). */
    val person = MutableStateFlow<Ev.Person?>(null)
    /** Shown once in a dialog: the 24 words of a new person. */
    val phrase = MutableStateFlow<String?>(null)
    /** Shown once in a dialog: the 6-word code of a backup just made. */
    val backupCode = MutableStateFlow<String?>(null)
    /** Shown in a dialog: a device was linked to our person. */
    val linked = MutableStateFlow<Ev.DeviceLinked?>(null)
    /** The last error from the node, for the screen that caused it. */
    val lastError = MutableStateFlow<String?>(null)
    /** Bumped when messages change; screens reload from the database. */
    val msgVersion = MutableStateFlow(0L)
    val unread = MutableStateFlow<Map<String, Int>>(emptyMap())
    /** Backbones the host is linked to (backbone menu). */
    val backbones = MutableStateFlow<List<Ev.Backbone>>(emptyList())
    private var backbonesNext = emptyList<Ev.Backbone>()
    /** Trace sent at (dest → ms), for the round trip time in the backbone menu. */
    val traceSent = java.util.concurrent.ConcurrentHashMap<String, Long>()
    val traceRtt = MutableStateFlow<Map<String, Long>>(emptyMap())
    /** The conversation on screen (no notification for it). */
    var openConv: String? = null
    /** History kept by the host (phone node or remote host). */
    var hostHistory = false; private set
    private val _status = MutableStateFlow("")
    val status: StateFlow<String> = _status.asStateFlow()

    fun post(r: () -> Unit) { main.post(r) }

    fun init(ctx: Context) {
        if (::app.isInitialized) return
        app = ctx.applicationContext
        db = Db(app)
        prefs = Prefs(app)
        nicknames.value = prefs.nicknames
        keys.value = prefs.keys
        verified.value = prefs.verified
        dismissed.value = prefs.dismissedAlerts
        channels.value = db.channels()
        nodes.value = db.nodes().associateBy { it.addr }
        conn.value = Conn(prefs.mode, Conn.State.OFF, "Not connected")
        Notify.channels(app)
        Calls.init(app)
        // Placeholders an older version imported from the host's history.
        runCatching { db.writableDatabase.delete("msg", "text = ? AND file IS NULL", arrayOf("📎 file (not kept in the history)")) }
    }

    fun label(addr: String): String =
        nicknames.value[addr] ?: nodes.value[addr]?.name?.let { "~$it" } ?: shortAddr(addr)

    fun convTitle(conv: String): String {
        val (k, a) = conv.split(":", limit = 2)
        return if (k == "ch") "#" + (channels.value[a] ?: shortAddr(a)) else label(a)
    }

    fun note(s: String) {
        val t = java.text.SimpleDateFormat("HH:mm:ss", java.util.Locale.ROOT).format(java.util.Date())
        log.update { (it + "$t  $s").takeLast(300) }
        android.util.Log.i("maarse-app", s)
    }

    // ---------- connecting ----------

    /** Connect in the chosen mode, and keep doing so until [stop]. */
    fun start(mode: Mode) {
        stopLink()
        prefs.mode = mode
        prefs.running = true
        retryMs = 2000
        NodeService.start(app)
        if (mode == Mode.PHONE || mode == Mode.SERVER) HostService.start(app, mode == Mode.SERVER)
        else HostService.stop(app)
        val radio = prefs.bleAddress
        if (mode == Mode.PHONE && radio != null && ModemBridge.enabled(app)) { ModemBridge.stop(); ModemBridge.start(app, radio) } else ModemBridge.stop()
        connect()
    }

    fun stop() {
        prefs.running = false
        ModemBridge.stop()
        stopLink()
        if (prefs.mode == Mode.PHONE || prefs.mode == Mode.SERVER) HostService.stop(app)
        NodeService.stop(app)
        conn.value = Conn(prefs.mode, Conn.State.OFF, "Not connected")
    }

    /** Use the chosen radio as the phone node's LoRa modem (or stop). */
    fun setModemRadio(on: Boolean) {
        ModemBridge.configure(app, on)
        ModemBridge.stop()
        restartHost()
    }

    /** Restart the phone's host, e.g. after its settings changed. */
    fun restartHost() {
        if (prefs.mode != Mode.PHONE && prefs.mode != Mode.SERVER) return
        stopLink()
        HostService.stop(app)
        main.postDelayed({ if (prefs.running) start(prefs.mode) }, 1500)
    }

    private fun stopLink() {
        main.removeCallbacks(retry)
        main.removeCallbacks(poll)
        link?.let { link = null; it.close() }
        me.value = null
    }

    private val retry = Runnable { connect() }

    private fun connect() {
        if (!prefs.running || link != null) return
        val mode = prefs.mode
        HostFiles.takeServerStop(app)?.let { note(it); _status.value = it }
        conn.value = Conn(mode, Conn.State.CONNECTING, "Connecting…")
        link = when (mode) {
            Mode.BLE -> {
                val a = prefs.bleAddress
                if (a == null) { conn.value = Conn(mode, Conn.State.OFF, "Choose a radio in Settings"); prefs.running = false; null }
                else runCatching { BleLink(app, a, this) }.getOrElse {
                    conn.value = Conn(mode, Conn.State.OFF, "Bluetooth: ${it.message}"); null
                }
            }
            Mode.PHONE -> {
                val key = HostFiles.accessKey(app)
                if (key == null) { scheduleRetry(); conn.value = Conn(mode, Conn.State.CONNECTING, "Starting the node…"); null }
                else WsLink("ws://127.0.0.1:${HostFiles.PORT}/ws", "This phone", this, key)
            }
            Mode.REMOTE -> {
                val origin = remoteOrigin()
                if (origin == null) {
                    conn.value = Conn(mode, Conn.State.OFF, "Enter a valid host address in Settings")
                    prefs.running = false
                    null
                } else {
                    val url = origin.newBuilder().addPathSegment("ws").build()
                    WsLink(url.toString(), "Host ${origin.host}", this, prefs.remoteKey.trim())
                }
            }
            // The server has no chat client; the app only shows its state.
            Mode.SERVER -> { conn.value = Conn(mode, Conn.State.ONLINE, "Server running"); null }
        }
    }

    private fun scheduleRetry() {
        main.removeCallbacks(retry)
        main.postDelayed(retry, retryMs)
        retryMs = minOf(retryMs * 2, 30_000)
    }

    fun send(frame: ByteArray): Boolean {
        val l = link ?: return false
        if (conn.value.state != Conn.State.ONLINE) return false
        l.send(frame)
        return true
    }

    fun req(): Int { nextReq = (nextReq % 0x6fff) + 1; return nextReq }

    override fun onOpen(link: Link) {
        if (link !== this.link) return
        retryMs = 2000
        conn.value = Conn(prefs.mode, Conn.State.ONLINE, link.name)
        note("Connected via ${link.name}")
        hostHistory = false
        link.send(Cmd.getInfo())
        link.send(Cmd.getName())
        link.send(Cmd.setTime())
        link.send(Cmd.getStats())
        link.send(Cmd.getRegions())
        link.send(Cmd.getSharing())
        link.send(Cmd.getSite())
        // Only a radio has radio settings (a host answers with an error).
        if (prefs.mode == Mode.BLE) link.send(Cmd.getRadio())
        person.value = null
        if (prefs.mode != Mode.BLE) link.send(Cmd.getPerson())
        // The radio bridge may have stopped (e.g. app process restarted): bring it back.
        val radio = prefs.bleAddress
        if (prefs.mode == Mode.PHONE) note("Radio as modem: ${if (ModemBridge.enabled(app)) "on" else "off"}, radio ${radio ?: "none"}, bridge ${if (ModemBridge.isRunning) "running" else "stopped"}")
        if (prefs.mode == Mode.PHONE && ModemBridge.enabled(app) && !ModemBridge.isRunning) {
            if (radio == null) note("Radio as modem is on, but no radio is chosen") else ModemBridge.start(app, radio)
        }
        main.removeCallbacks(poll)
        main.postDelayed(poll, 15_000)
    }

    override fun onClosed(link: Link, why: String) {
        if (link !== this.link) return
        this.link = null
        me.value = null
        note("Connection closed: $why")
        conn.value = Conn(prefs.mode, if (prefs.running) Conn.State.CONNECTING else Conn.State.OFF, why)
        // The phone's host exits on a settings restart: start it again.
        if (prefs.running && prefs.mode == Mode.PHONE) HostService.start(app, false)
        if (prefs.running) scheduleRetry()
    }

    /** Status every 15 s (5 s while a status screen is open). */
    var fastPoll = false
    private val poll = object : Runnable {
        override fun run() {
            send(Cmd.getStats())
            if (fastPoll) send(Cmd.getNeighbors())
            updateOffline()
            if (System.currentTimeMillis() - lastPosition > 10 * 60_000) updatePosition()
            main.postDelayed(this, if (fastPoll) 5000 else 15_000)
        }
    }

    // ---------- sending ----------

    fun maxText(conv: String) = if (conv.startsWith("ch:")) 190 else (me.value?.maxText ?: 148)

    fun sendText(conv: String, text: String): String? {
        val t = text.trim()
        if (t.isEmpty()) return null
        if (t.toByteArray().size > maxText(conv)) return "The message is too long"
        // No way out (airplane mode, no radio): keep it and send it when
        // there is one, instead of pretending it went.
        if (offline.value || conn.value.state != Conn.State.ONLINE) {
            db.insert(Msg(0, conv, true, me.value?.addr ?: "", t, System.currentTimeMillis(), "queued", 0, 0))
            bump()
            return null
        }
        val id = db.insert(Msg(0, conv, true, me.value?.addr ?: "", t, System.currentTimeMillis(), "sending…", 0, 0))
        transmit(id, conv, t)
        bump()
        return null
    }

    private fun transmit(row: Long, conv: String, t: String) {
        val (k, a) = conv.split(":", limit = 2)
        val r = req()
        pending[r] = row
        send(if (k == "ch") Cmd.sendChannel(r, a, t) else Cmd.sendDirect(r, a, t))
    }

    /** Messages written while offline go out once there is a way. */
    private fun flushQueued() {
        if (offline.value || conn.value.state != Conn.State.ONLINE) return
        val q = db.queued()
        if (q.isEmpty()) return
        q.forEach { m -> db.setStatus(m.id, "sending…"); transmit(m.id, m.conv, m.text) }
        note("Sent ${q.size} message(s) written while offline")
        bump()
    }

    /**
     * No way to reach anyone: no internet on the phone (or the host has no
     * links) and no radio. A radio over Bluetooth always counts as a way.
     */
    val offline = MutableStateFlow(false)
    val offlineWhy = MutableStateFlow("")

    private fun updateOffline() {
        val mode = prefs.mode
        val s = stats.value
        val net = runCatching {
            val cm = app.getSystemService(android.net.ConnectivityManager::class.java)
            cm.getNetworkCapabilities(cm.activeNetwork)?.hasCapability(android.net.NetworkCapabilities.NET_CAPABILITY_INTERNET) == true
        }.getOrDefault(true)
        val off = when (mode) {
            Mode.BLE -> false
            Mode.PHONE -> !ModemBridge.enabled(app) && (!net || (s[5] ?: 1) == 0)
            Mode.REMOTE, Mode.SERVER -> !net
        }
        offlineWhy.value = when {
            !off -> ""
            !net -> "No internet" + (if (mode == Mode.PHONE) " and no radio" else "") + ". Messages wait and go out when you are back online."
            else -> "The phone has internet, but no Må:rse node answers (the backbone servers cannot be reached). Messages wait and go out when one does."
        }
        if (off != offline.value) offline.value = off
        if (!off) flushQueued()
    }

    // ---------- the phone's position, for sharing ----------

    fun hasLocation(ctx: Context) =
        ctx.checkSelfPermission(android.Manifest.permission.ACCESS_COARSE_LOCATION) == android.content.pm.PackageManager.PERMISSION_GRANTED ||
            ctx.checkSelfPermission(android.Manifest.permission.ACCESS_FINE_LOCATION) == android.content.pm.PackageManager.PERMISSION_GRANTED

    private var lastPosition = 0L
    private var positionRequest = 0L

    fun setPrecision(prec: Int) {
        positionRequest++
        val sh = sharing.value
        send(Cmd.setSharing(prec, ((sh?.lat ?: 0.0) * 1e7).toInt(), ((sh?.lon ?: 0.0) * 1e7).toInt(), sh?.telemetry ?: false))
        send(Cmd.getSharing())
    }

    /** The phone's location to the node (only while sharing is on). */
    @android.annotation.SuppressLint("MissingPermission")
    fun updatePosition(ctx: Context = app) {
        val sh = sharing.value ?: return
        if (sh.prec == 0 || !hasLocation(ctx)) return
        // A fixed node keeps the position its owner set for the site.
        if ((site.value?.heightDm ?: 0) > 0) return
        lastPosition = System.currentTimeMillis()
        val request = ++positionRequest
        val lm = ctx.getSystemService(android.location.LocationManager::class.java)
        val use = { l: android.location.Location? ->
            if (l != null) main.post {
                val cur = sharing.value ?: return@post
                if (request != positionRequest || cur.prec == 0) return@post
                val lat = (l.latitude * 1e7).toInt()
                val lon = (l.longitude * 1e7).toInt()
                if (lat == (cur.lat * 1e7).toInt() && lon == (cur.lon * 1e7).toInt()) return@post
                send(Cmd.setSharing(cur.prec, lat, lon, cur.telemetry))
                send(Cmd.getSharing())
            }
        }
        val providers = lm.getProviders(true)
        providers.mapNotNull { runCatching { lm.getLastKnownLocation(it) }.getOrNull() }.maxByOrNull { it.time }?.let(use)
        if (android.os.Build.VERSION.SDK_INT >= 30) {
            val p = listOf(android.location.LocationManager.FUSED_PROVIDER, android.location.LocationManager.GPS_PROVIDER, android.location.LocationManager.NETWORK_PROVIDER).firstOrNull { it in providers }
            if (p != null) runCatching { lm.getCurrentLocation(p, null, app.mainExecutor) { use(it) } }
        }
    }

    // ---------- files and voice messages ----------

    private class Incoming(val row: Long, val buf: ByteArray, val mime: String, val name: String, val from: String)
    private val pendingFiles = HashMap<Int, Long>() // request -> row
    private val fileOut = HashMap<Long, Long>()     // file ID -> row
    private val fileIn = HashMap<Long, Incoming>()

    private fun attachDir() = java.io.File(app.filesDir, "attachments").apply { mkdirs() }

    fun fileLabel(mime: String, name: String, data: ByteArray?): String = when {
        mime == Voice.MIME -> "Voice message" + (data?.let { " · ${Voice.seconds(it)} s" } ?: "")
        mime.startsWith("image/") -> "Photo"
        else -> "$name (${((data?.size ?: 0) + 1023) / 1024} KB)"
    }

    /** Largest file the node takes: over a radio 8 KB (LoRa), on a host its limit. */
    fun fileMax(): Int {
        val m = me.value?.maxFile?.toInt()?.takeIf { it > 0 } ?: 8192
        return if (prefs.mode == Mode.BLE) minOf(m, 8192) else m
    }

    private var fileLimitWaiter: ((Ev.FileLimit?) -> Unit)? = null

    /**
     * Asks the node how a file reaches [dest] (internet, LoRa, or unknown
     * route / post office). A radio node does not know the command: then
     * [cb] gets null after a short wait. Main thread.
     */
    fun queryFileLimit(dest: String, cb: (Ev.FileLimit?) -> Unit) {
        if (prefs.mode == Mode.BLE) return cb(null)
        var done = false
        val finish = { ev: Ev.FileLimit? -> if (!done) { done = true; fileLimitWaiter = null; cb(ev) } }
        fileLimitWaiter = { ev -> finish(ev?.takeIf { it.dest == dest }) }
        main.postDelayed({ finish(null) }, 1500)
        if (!send(Cmd.getFileLimit(dest))) finish(null)
    }

    /** Sends a file (a picture is made smaller to fit). Returns an error or null. */
    fun sendFile(conv: String, data0: ByteArray, mime0: String, name0: String, maxBytes: Int? = null): String? {
        if (!conv.startsWith("dm:")) return "Files and voice messages go to one person, not to a channel"
        if (offline.value || conn.value.state != Conn.State.ONLINE) return "Not connected — files are not queued; send it when you are online"
        val max = maxBytes?.let { minOf(it, fileMax()) } ?: fileMax()
        var (data, mime, name) = Triple(data0, mime0, name0)
        // Pictures are made smaller like in other messengers: at most about
        // 150 KB (and the node's limit, 8 KB over LoRa), so they go quickly.
        val picMax = minOf(max, 150_000)
        if (data.size > picMax && mime.startsWith("image/")) {
            data = Images.shrink(data, picMax) ?: return "The picture could not be made small enough (${max / 1024} KB)"
            mime = "image/webp"; name = name.substringBeforeLast('.') + ".webp"
        }
        if (data.size > max) return "Too large: ${(data.size + 1023) / 1024} KB, at most ${max / 1024} KB here" +
            (if (prefs.mode == Mode.BLE) " (over LoRa)" else "")
        val file = java.io.File(attachDir(), "${System.currentTimeMillis()}_${name.replace(Regex("[^A-Za-z0-9._-]"), "_")}")
        file.writeBytes(data)
        val row = db.insert(Msg(0, conv, true, me.value?.addr ?: "", fileLabel(mime, name, data), System.currentTimeMillis(), "sending…", 0, 0))
        db.setFile(row, file.path, mime)
        val r = req()
        pendingFiles[r] = row
        val dest = conv.substringAfter(":")
        send(Cmd.fileBegin(r, dest, data.size, mime, name))
        var o = 0
        while (o < data.size) { send(Cmd.fileData(r, o, data.copyOfRange(o, minOf(data.size, o + 240)))); o += 240 }
        send(Cmd.fileEnd(r))
        bump()
        return null
    }

    private fun onFileEvent(ev: Ev) {
        when (ev) {
            is Ev.FileStarted -> pendingFiles.remove(ev.req)?.let { row -> fileOut[ev.fileId] = row; setStatus(row, "sending 0/${ev.chunks}") }
            // Pages and pictures we asked for go to the Pages screen, not a chat.
            is Ev.FileIncoming -> if (!Pages.onIncoming(ev)) {
                if (ev.size <= 0 || ev.size > 16 * 1024 * 1024) return
                // A page or page picture we did not (or no longer) wait
                // for: never a file in a chat.
                if (Pages.isPageFile(ev.mime, ev.name)) return
                heard(ev.from) { it }
                val conv = "dm:${ev.from}"
                val row = db.insert(Msg(0, conv, false, ev.from, fileLabel(ev.mime, ev.name, null) + " — receiving…", System.currentTimeMillis(), "", 0, 0))
                db.setFile(row, null, ev.mime)
                fileIn[ev.fileId] = Incoming(row, ByteArray(ev.size), ev.mime, ev.name, ev.from)
                bump()
            }
            is Ev.FileChunk -> if (!Pages.onChunk(ev)) fileIn[ev.fileId]?.let { f ->
                val n = minOf(ev.data.size, f.buf.size - ev.offset)
                if (ev.offset >= 0 && n > 0) System.arraycopy(ev.data, 0, f.buf, ev.offset, n)
            }
            is Ev.FileProgress -> if (!ev.outgoing && Pages.onProgress(ev)) Unit else if (ev.outgoing) fileOut[ev.fileId]?.let { setStatus(it, "sending ${ev.done}/${ev.total}") }
                else fileIn[ev.fileId]?.let { f ->
                    db.setFile(f.row, null, f.mime, fileLabel(f.mime, f.name, null) + " — receiving ${100 * ev.done / maxOf(1, ev.total)} %"); bump()
                }
            // A file kept in the node's file mailbox keeps its file id (in the
            // msgId column), so it can be cancelled and marked delivered later.
            is Ev.FileDone -> if (!ev.outgoing && Pages.onDone(ev)) Unit else if (ev.outgoing) (fileOut.remove(ev.fileId) ?: db.byMsgId(ev.fileId))?.let { row ->
                when {
                    // "cancelled" or "cancelled by the recipient"
                    !ev.ok -> setStatus(row, if (ev.text.startsWith("cancelled")) ev.text else "error: ${ev.text}")
                    ev.text.contains("put in mailbox") -> setStatus(row, "filebox", ev.fileId)
                    else -> setStatus(row, "delivered")
                }
            } else fileIn.remove(ev.fileId)?.let { f ->
                if (!ev.ok) {
                    val why = if (ev.text.startsWith("cancelled")) ev.text else "not received (${ev.text})"
                    db.setFile(f.row, null, f.mime, fileLabel(f.mime, f.name, null) + " — $why"); bump(); return
                }
                val file = java.io.File(attachDir(), "${System.currentTimeMillis()}_${f.name.replace(Regex("[^A-Za-z0-9._-]"), "_")}")
                file.writeBytes(f.buf)
                val label = fileLabel(f.mime, f.name, f.buf)
                db.setFile(f.row, file.path, f.mime, label)
                bump()
                val conv = "dm:${f.from}"
                if (openConv != conv) {
                    unread.update { it + (conv to (it[conv] ?: 0) + 1) }
                    if (prefs.notifyMessages) Notify.message(app, conv, label(f.from), label)
                }
            }
            else -> {}
        }
    }

    /** Can this message still be stopped (a file on its way, or waiting)? */
    fun cancellable(m: Msg) =
        if (m.mine) m.status == "queued" || m.status.startsWith("sending") || m.status == "filebox" || (m.file != null && m.status == "sent")
        else fileIn.values.any { it.row == m.id } // a file on its way to us

    /** Stops a file on its way (the host stops sending) or a waiting message, and removes it. */
    fun cancel(m: Msg) {
        // Incoming: the node drops what it has and asks the sender to stop.
        if (!m.mine) {
            fileIn.entries.firstOrNull { it.value.row == m.id }?.let { (fileId, f) ->
                send(Cmd.cancelFile(fileId))
                fileIn.remove(fileId)
                db.setFile(f.row, null, f.mime, fileLabel(f.mime, f.name, null) + " — cancelled"); bump()
            }
            return
        }
        fileOut.entries.firstOrNull { it.value == m.id }?.let { (fileId, _) -> send(Cmd.cancelFile(fileId)); fileOut.remove(fileId) }
        // Kept in the node's file mailbox: its file id is in msgId.
        if (m.status == "filebox" && m.msgId != 0L) send(Cmd.cancelFile(m.msgId))
        pendingFiles.entries.removeAll { it.value == m.id }
        pending.entries.removeAll { it.value == m.id }
        deleteMessage(m)
    }

    /** Removes a message here and from the host's history (not from the other person's phone). */
    fun deleteMessage(m: Msg) {
        m.file?.let { runCatching { java.io.File(it).delete() } }
        db.delete(m.id)
        if (hostHistory && m.file == null)
            send(Cmd.histDelete(m.conv.substringAfter(":"), if (m.mine) 1 else 0, m.ts / 1000, m.text.toByteArray()))
        bump()
    }

    /** Deletes a whole conversation here and in the host's history. */
    fun deleteConversation(conv: String) {
        db.messages(conv, 100_000).forEach { it.file?.let { f -> runCatching { java.io.File(f).delete() } } }
        db.deleteConv(conv)
        if (hostHistory) send(Cmd.histDelete(conv.substringAfter(":"), 2, 0, ByteArray(0)))
        unread.update { it - conv }
        bump()
    }

    fun joinChannel(name: String) { send(Cmd.join(name.trim().removePrefix("#"))) }

    /** The safety number with `addr`, or null while a key is missing. */
    fun safetyNumber(addr: String): String? {
        val mine = myKey.value ?: return null
        val theirs = keys.value[addr] ?: return null
        return Safety.number(mine.unhex(), theirs.unhex())
    }

    /** Verified, and the key is still the one that was verified? */
    fun isVerified(addr: String): Boolean = verified.value[addr]?.let { it == keys.value[addr] } == true

    /** Verified earlier, but the contact's key has changed since. */
    fun keyChanged(addr: String): Boolean = verified.value[addr]?.let { it != keys.value[addr] } == true

    fun setVerified(addr: String, on: Boolean) {
        val m = verified.value.toMutableMap()
        val k = keys.value[addr]
        if (on && k != null) m[addr] = k else m.remove(addr)
        verified.value = m
        prefs.verified = m
    }

    fun setNickname(addr: String, nick: String) {
        val m = nicknames.value.toMutableMap()
        if (nick.isBlank()) m.remove(addr) else m[addr] = nick.trim()
        nicknames.value = m
        prefs.nicknames = m
    }

    fun markRead(conv: String) {
        if ((unread.value[conv] ?: 0) > 0) unread.update { it - conv }
        // Always tell the host, also when nothing is unread here: messages
        // that arrive while the chat is open never count as unread in the
        // app, but the host counts them until it is told, and its count
        // came back as "61 unread" the next time the app opened.
        if (hostHistory) send(Cmd.histMarkRead(conv.substringAfter(":"), 0))
        Notify.cancel(app, conv)
    }

    fun dismissAlert(a: Ev.Alert) {
        dismissed.update { it + a.key }
        prefs.dismissedAlerts = dismissed.value
    }

    fun trace(dest: String) { traceSent[dest] = System.currentTimeMillis(); send(Cmd.traceRoute(req(), dest)) }

    fun personSetup(op: Int, text: String = "") { lastError.value = null; send(Cmd.personSetup(req(), op, text)) }
    fun linkDevice(device: String) { lastError.value = null; send(Cmd.linkDevice(req(), device)) }

    // ---------- the account backup file on the host ----------

    private val http by lazy { okhttp3.OkHttpClient() }

    data class MeshCoreAccess(val text: String = "", val allowed: Boolean = false, val key: String? = null,
        val busy: Boolean = false, val results: List<String> = emptyList(), val messages: List<String> = emptyList(),
        val unread: Int = 0, val lastId: Long = 0, val contacts: List<String> = emptyList(),
        val direct: MeshCoreDirect? = null, val channels: List<String> = emptyList(), val channel: MeshCoreDirect? = null,
        val repeaterKey: String = "", val repeaterStatus: String = "", val totalUnread: Int = 0,
        val repeaterSeq: Long = 0, val repeaterKind: String = "", val repeaterData: String = "",
        val conversations: List<McConversation> = emptyList(), val nodes: List<McNode> = emptyList(), val scanning: Boolean = false,
        val items: List<McMessage> = emptyList(), val scan: McScan = McScan(), val radio: McRadio = McRadio())
    /** One row of the MeshCore conversation list; kind is public, channel or direct. */
    data class McConversation(val kind: String, val id: String, val title: String, val last: String, val mine: Boolean,
                              val unread: Int, val time: Long)
    /** A MeshCore node heard by the user's radio; type 1 chat, 2 repeater, 3 room, 4 sensor. */
    data class McNode(val key: String, val name: String, val type: Int, val hops: Int, val snr: Double, val rssi: Double,
                      val seen: Long, val count: Int, val favorite: Boolean)
    /** One request to the backbone. A waiting caller gets the new state when the answer is in (null if it failed). */
    class McTask(val payload: String, val done: kotlinx.coroutines.CompletableDeferred<MeshCoreAccess?>? = null)
    private val mcQueue = ArrayDeque<McTask>()
    private var mcCurrent: McTask? = null

    /** Like [mcAsk], but waits for the answer. */
    suspend fun mcCall(payload: String): MeshCoreAccess? {
        val done = kotlinx.coroutines.CompletableDeferred<MeshCoreAccess?>()
        main.post { if (mcQueue.size < 40) { mcQueue.addLast(McTask(payload, done)); mcPump() } else done.complete(null) }
        return kotlinx.coroutines.withTimeoutOrNull(40_000) { done.await() }
    }

    /** The backbone takes one request at a time, so MeshCore requests wait in line. Any payload is signed with operation 9. */
    fun mcAsk(payload: String) {
        if (mcQueue.size < 20 && mcQueue.none { it.payload == payload && it.done == null }) mcQueue.addLast(McTask(payload))
        mcPump()
    }
    private fun mcPump() {
        if (meshcoreSocket != null || mcQueue.isEmpty()) return
        val address = prefs.meshcoreBackbone
        if (address.isEmpty()) { mcQueue.clear(); return }
        val task = mcQueue.removeFirst()
        mcCurrent = task
        checkMeshcoreAccess(address, 9, task.payload, background = true)
        if (meshcoreSocket == null) { mcCurrent = null; task.done?.complete(null) }   // it could not start
    }
    data class MeshCoreDirect(val contact: String, val messages: List<String>, val unread: Int = 0, val lastId: Long = 0,
                              val items: List<McMessage> = emptyList())
    /** A message in a MeshCore conversation. repeats: repeaters that passed our message on (-1: not known); delivered: acknowledged. */
    data class McMessage(val id: Long, val text: String, val mine: Boolean, val ts: Long, val repeats: Int = -1,
                         val delivered: Boolean = false, val snr: Double = 0.0)
    /** Scan progress from the radio: waiting for it to start, listening now, nodes that answered so far. */
    /** Outgoing MeshCore flood settings: path hash size (1-3 bytes) and region scope ("" for none). */
    data class McRadio(val hashSize: Int = 1, val scope: String = "")
    data class McScan(val waiting: Boolean = false, val active: Boolean = false, val found: Int = 0)
    /** Can MeshCore be used now? state 0 not tried, 1 connecting, 2 ready, 3 not available (note says why). */
    data class McLink(val state: Int = 0, val note: String = "")
    val mcLink = MutableStateFlow(McLink())
    val meshcoreAccess = MutableStateFlow(MeshCoreAccess())
    private data class MeshCoreAttempt(val request: Int, val operation: Int, val payload: String, val socket: okhttp3.WebSocket,
                                       var received: Int = 0, var proof: ByteArray? = null)
    private var meshcoreSocket: okhttp3.WebSocket? = null
    private var meshcoreAttempt: MeshCoreAttempt? = null

    private var meshcoreDenied = false

    private var mcNextPoll = 0L
    private var mcSentName = ""

    /** Called every few seconds. Tries MeshCore as soon as the node is up; a failure is retried after 10 s. */
    fun pollMeshcoreUnread() {
        val address = prefs.meshcoreBackbone
        if (address.isEmpty()) return
        if (conn.value.state != Conn.State.ONLINE) {
            mcLink.value = McLink(3, "Your node is not connected, so MeshCore cannot be reached.")
            mcNextPoll = 0
            return
        }
        if (person.value?.state != PersonState.MEMBER) {
            mcLink.value = McLink(3, meshcoreAccountNote().orEmpty())
            return
        }
        if (meshcoreSocket != null || System.currentTimeMillis() < mcNextPoll) return
        mcNextPoll = System.currentTimeMillis() + 10_000L
        checkMeshcoreAccess(address, 5, background = true)
    }

    /** The Maarse name is also the MeshCore node's name. */
    private fun syncMeshcoreName() {
        val name = me.value?.name?.trim().orEmpty().take(31)
        if (name.isNotEmpty() && name != mcSentName && name.toByteArray().size <= 31) {
            mcSentName = name
            mcAsk("set-name:${name.toByteArray().hex()}")
        }
    }

    /** MeshCore proves access with the account, so a device without one cannot ask. */
    fun meshcoreAccountNote(): String? = when (person.value?.state) {
        null, PersonState.MEMBER -> null
        PersonState.AWAITING -> "MeshCore needs an account. This device is waiting to be linked: finish that on your master device first."
        else -> "MeshCore needs an account. Create one, or link this device to the account on your other device, first (Settings → Account and devices)."
    }

    fun checkMeshcoreAccess(address: String, operation: Int, payload: String = "", background: Boolean = false) {
        meshcoreAccountNote()?.let { note ->
            mcLink.value = McLink(3, note)
            if (!background) meshcoreAccess.value = MeshCoreAccess(note)
            return
        }
        val origin = address.toHttpUrlOrNull()?.takeIf { it.isHttps && it.username.isEmpty() && it.password.isEmpty() }
        if (origin == null) {
            if (!background) meshcoreAccess.value = MeshCoreAccess("Enter the HTTPS address of the backbone.")
            return
        }
        prefs.meshcoreBackbone = origin.toString()
        if (mcLink.value.state != 2) mcLink.value = McLink(1)
        meshcoreSocket?.cancel()
        meshcoreAttempt = null
        if (!background) {
            meshcoreDenied = false
            meshcoreAccess.update { it.copy(text = "Checking access...", busy = true) }
        }
        val url = origin.newBuilder().encodedPath("/meshcore/session").query(null).fragment(null).build()
        val listener = object : okhttp3.WebSocketListener() {
            override fun onMessage(socket: okhttp3.WebSocket, text: String) {
                main.post {
                    if (meshcoreSocket !== socket) return@post
                    val json = runCatching { org.json.JSONObject(text) }.getOrNull() ?: run { socket.cancel(); return@post }
                    val challenge = json.optString("challenge")
                    if (json.optInt("version") == 1 && challenge.matches(Regex("[0-9a-f]{64}")) && meshcoreAttempt == null) {
                        val request = req()
                        meshcoreAttempt = MeshCoreAttempt(request, operation, payload, socket)
                        android.util.Log.i("MaarseMeshCore", "challenge received, asking the node for a proof")
                        send(Cmd.meshcoreProof(request, operation, challenge.unhex(), payload))
                    } else if (json.has("allowed") && (meshcoreAttempt?.received ?: 0) > 0) {
                        val allowed = json.optBoolean("allowed")
                        // An older backbone refuses the new list requests: keep what we have instead of acting as if access was lost.
                        val sent = meshcoreAttempt?.payload.orEmpty()
                        if (!allowed && background && meshcoreAccess.value.allowed &&
                            sent.isNotEmpty()) {
                            // An older backbone refuses requests it does not know (new lists, sending settings).
                            meshcoreSocket = null
                            meshcoreAttempt = null
                            meshcoreAccess.update { it.copy(text = "The backbone server does not support this yet. Update the server.") }
                            socket.close(1000, null)
                            mcCurrent?.done?.complete(null)
                            mcCurrent = null
                            main.post { mcPump() }
                            return@post
                        }
                        meshcoreDenied = !allowed
                        val gateway = json.optJSONObject("gateway")
                        mcLink.value = when {
                            !allowed -> McLink(3, "This account has no MeshCore access on the backbone.")
                            gateway != null && !gateway.optBoolean("connected") ->
                                McLink(3, "Reached the backbone, but the MeshCore gateway is not connected to it.")
                            else -> McLink(2)
                        }
                        mcNextPoll = System.currentTimeMillis() + if (!allowed) 60_000L else if (mcLink.value.state == 2) 30_000L else 10_000L
                        if (mcLink.value.state == 2) syncMeshcoreName()
                        val key = json.optString("publicKey").takeIf { it.matches(Regex("[0-9a-f]{64}")) }
                        val results = json.optJSONArray("results")?.let { array ->
                            List(array.length()) { array.optString(it) }
                        }.orEmpty()
                        val status = json.optString("error").ifEmpty {
                            when {
                                json.optString("queued") == "announce" -> "Announcement queued."
                                json.optString("queued") in listOf("public-send", "direct-send", "channel-send") -> "Message queued."
                                json.optString("queued") == "scan" || json.optBoolean("scanning") -> "Scan in progress. Fetch results in about 10 seconds."
                                json.has("results") -> if (results.isEmpty()) "No nodes found." else "Scan complete."
                                allowed -> "MeshCore access granted."
                                else -> "MeshCore access is unavailable for this account."
                            }
                        }
                        meshcoreSocket = null
                        meshcoreAttempt = null
                        val previous = meshcoreAccess.value
                        val messages = json.optJSONArray("messages")?.let { array ->
                            List(array.length()) { index ->
                                val message = array.getJSONObject(index)
                                (if (message.optBoolean("mine")) "You: " else "") + message.optString("text")
                            }
                        }
                        val items = json.optJSONArray("messages")?.let { array ->
                            List(array.length()) { index ->
                                val o = array.getJSONObject(index)
                                McMessage(o.optLong("id"), o.optString("text"), o.optBoolean("mine"), o.optLong("timestamp"),
                                    if (o.has("repeats")) o.optInt("repeats") else -1, o.optBoolean("delivered"), o.optDouble("snr", 0.0))
                            }
                        }
                        val contacts = json.optJSONArray("contacts")?.let { array -> List(array.length()) { array.optString(it) } }
                            ?: previous.contacts
                        // The overview lists the joined channels too: without that the open channel was dropped on the next poll.
                        val channels = json.optJSONArray("channels")?.let { array -> List(array.length()) { array.optString(it) } }
                            ?: json.optJSONArray("conversations")?.let { array ->
                                (0 until array.length()).map { array.getJSONObject(it) }.filter { it.optString("kind") == "channel" }.map { it.optString("id") }
                            } ?: previous.channels
                        val channelName = json.optString("channel").takeIf { it.matches(Regex("[a-z0-9-]{1,24}")) }
                        val activeChannel = previous.channel?.takeIf { it.contact in channels }
                        val channel = if (channelName == null) activeChannel else {
                            val old = activeChannel?.takeIf { it.contact == channelName }
                            MeshCoreDirect(channelName, messages ?: old?.messages.orEmpty(),
                                json.optInt("unread", old?.unread ?: 0), json.optLong("lastId", old?.lastId ?: 0), items ?: old?.items.orEmpty())
                        }
                        val contact = json.optString("contact").takeIf { it.matches(Regex("[0-9a-f]{64}")) }
                        val direct = if (contact == null) previous.direct else {
                            val old = previous.direct?.takeIf { it.contact == contact }
                            MeshCoreDirect(contact, messages ?: old?.messages.orEmpty(),
                                json.optInt("unread", old?.unread ?: 0), json.optLong("lastId", old?.lastId ?: 0), items ?: old?.items.orEmpty())
                        }
                        meshcoreAccess.value = if (allowed) MeshCoreAccess(if (background) previous.text else status, true, key,
                            results = if (background) previous.results else results,
                            messages = if (contact == null && channelName == null) messages ?: previous.messages else previous.messages,
                            items = if (contact == null && channelName == null) items ?: previous.items else previous.items,
                            radio = json.optJSONObject("radio")?.let { McRadio(it.optInt("hashSize", 1).coerceIn(1, 3), it.optString("scope")) } ?: previous.radio,
                            scan = json.optJSONObject("scan")?.let { McScan(it.optBoolean("waiting"), it.optBoolean("active"), it.optInt("found")) } ?: previous.scan,
                            unread = if (contact == null && channelName == null) json.optInt("unread", previous.unread) else previous.unread,
                            lastId = if (contact == null && channelName == null) json.optLong("lastId", previous.lastId) else previous.lastId,
                            contacts = contacts, direct = direct, channels = channels, channel = channel,
                            repeaterKey = json.optString("repeater", previous.repeaterKey),
                            totalUnread = json.optInt("totalUnread", previous.totalUnread),
                            conversations = json.optJSONArray("conversations")?.let { array -> List(array.length()) {
                                val o = array.getJSONObject(it)
                                McConversation(o.optString("kind"), o.optString("id"), o.optString("title"), o.optString("last"),
                                    o.optBoolean("mine"), o.optInt("unread"), o.optLong("time"))
                            } } ?: previous.conversations,
                            nodes = json.optJSONArray("nodes")?.let { array -> List(array.length()) {
                                val o = array.getJSONObject(it)
                                McNode(o.optString("key"), o.optString("name"), o.optInt("type"), o.optInt("hops"), o.optDouble("snr"),
                                    o.optDouble("rssi"), o.optLong("seen"), o.optInt("count"), o.optBoolean("favorite"))
                            } } ?: previous.nodes,
                            scanning = if (json.has("nodes")) json.optBoolean("scanning") else previous.scanning,
                            repeaterSeq = json.optLong("repeaterSeq", previous.repeaterSeq),
                            repeaterKind = if (json.has("repeaterKind")) json.optString("repeaterKind") else previous.repeaterKind,
                            repeaterData = if (json.has("repeaterData")) json.optString("repeaterData") else previous.repeaterData,
                            repeaterStatus = if (json.has("repeater")) json.optString("repeaterStatus").ifEmpty {
                                if (json.has("queued")) "Request queued." else ""
                            } else previous.repeaterStatus) else MeshCoreAccess(if (background) "" else status)
                        socket.close(1000, null)
                        mcCurrent?.done?.complete(meshcoreAccess.value)
                        mcCurrent = null
                        main.post { mcPump() }
                    }
                }
            }
            override fun onOpen(socket: okhttp3.WebSocket, response: okhttp3.Response) {
                android.util.Log.i("MaarseMeshCore", "connected to the backbone: HTTP ${response.code}")
            }
            override fun onFailure(socket: okhttp3.WebSocket, t: Throwable, response: okhttp3.Response?) {
                android.util.Log.w("MaarseMeshCore", "the connection failed: ${t.javaClass.simpleName}: ${t.message} (HTTP ${response?.code})")
                main.post { finishMeshcoreAccess(socket, background) }
            }
            override fun onClosed(socket: okhttp3.WebSocket, code: Int, reason: String) {
                android.util.Log.w("MaarseMeshCore", "the server closed the connection: $code $reason")
                main.post { finishMeshcoreAccess(socket, background) }
            }
        }
        val socket = http.newWebSocket(okhttp3.Request.Builder().url(url).build(), listener)
        meshcoreSocket = socket
        main.postDelayed({
            if (meshcoreSocket === socket) {
                android.util.Log.w("MaarseMeshCore", "timed out after 12 s: challenge received=${meshcoreAttempt != null}, proof bytes from the node=${meshcoreAttempt?.received}")
                socket.cancel(); finishMeshcoreAccess(socket, background)
            }
        }, 12_000)
    }

    private fun finishMeshcoreAccess(socket: okhttp3.WebSocket, background: Boolean) {
        if (meshcoreSocket !== socket) return
        meshcoreSocket = null
        meshcoreAttempt = null
        if (!background) meshcoreAccess.value = MeshCoreAccess("Could not verify access. Check the backbone address and node connection.")
        mcLink.value = McLink(3, "Could not connect to the backbone. Check its address and your connection.")
        mcCurrent?.done?.complete(null)
        mcCurrent = null
        mcNextPoll = System.currentTimeMillis() + 10_000L
        main.post { mcPump() }
    }

    private fun acceptMeshcoreProof(event: Ev.MeshCoreProof) {
        val attempt = meshcoreAttempt ?: return
        if (event.req != attempt.request || event.offset != attempt.received || event.total !in 326..1024 ||
            event.offset + event.bytes.size > event.total) return
        val proof = attempt.proof ?: ByteArray(event.total).also { attempt.proof = it }
        if (proof.size != event.total) return
        event.bytes.copyInto(proof, event.offset)
        attempt.received += event.bytes.size
        if (attempt.received != event.total) return
        android.util.Log.i("MaarseMeshCore", "proof complete (${event.total} bytes), sending it to the server")
        attempt.socket.send(org.json.JSONObject()
            .put("operation", attempt.payload.ifEmpty { listOf("access", "ensure", "announce", "scan", "scan-results", "public-history", "public-send", "public-read", "direct-list", "", "", "", "channel-list")[attempt.operation] })
            .put("device", proof.copyOfRange(0, 64).hex())
            .put("certificate", proof.copyOfRange(64, 193).hex())
            .put("signature", proof.copyOfRange(193, 257).hex())
            .put("list", proof.copyOfRange(257, proof.size).hex()).toString())
    }

    private fun remoteOrigin(): okhttp3.HttpUrl? {
        val address = prefs.remoteAddr.trim()
        val url = when {
            address.startsWith("wss://", ignoreCase = true) -> "https://" + address.substring(6)
            address.startsWith("ws://", ignoreCase = true) -> "http://" + address.substring(5)
            "://" in address -> address
            else -> "http://$address"
        }.toHttpUrlOrNull() ?: return null
        return url.takeIf {
            it.username.isEmpty() && it.password.isEmpty() && it.encodedPath == "/" &&
                it.query == null && it.fragment == null
        }
    }

    /** Where the host's HTTP API is (origin, access key): the phone's own node or the home computer; none for a radio alone. */
    fun hostBase(): Pair<String, String>? = when (prefs.mode) {
        Mode.PHONE -> HostFiles.accessKey(app)?.let { "http://127.0.0.1:${HostFiles.PORT}" to it }
        Mode.REMOTE -> remoteOrigin()?.let { it.toString().trimEnd('/') to prefs.remoteKey.trim() }
        else -> null
    }

    private fun remoteRequest(path: String): okhttp3.Request.Builder {
        val origin = requireNotNull(remoteOrigin()) { "Invalid host address" }
        val url = requireNotNull(origin.resolve(path))
        return okhttp3.Request.Builder().url(url).apply {
            val key = prefs.remoteKey.trim()
            if (key.isNotEmpty()) header("Authorization", "Bearer $key")
        }
    }

    /** The backup the host just wrote (phone: its folder; elsewhere: over HTTP). */
    fun readBackup(done: (ByteArray?) -> Unit) {
        Thread {
            val b = runCatching {
                if (prefs.mode == Mode.REMOTE) http.newCall(remoteRequest("/api/account-backup").build()).execute().use { r -> if (r.isSuccessful) r.body?.bytes() else null }
                else java.io.File(HostFiles.dataDir(app, false), "account-backup.maarse").readBytes()
            }.getOrNull()
            main.post { done(b) }
        }.start()
    }

    /** Puts a backup file on the host, then restores from it with the code. */
    fun restoreBackup(file: ByteArray, code: String) {
        lastError.value = null
        Thread {
            val ok = runCatching {
                if (prefs.mode == Mode.REMOTE) {
                    val body = okhttp3.RequestBody.create(null, file)
                    http.newCall(remoteRequest("/api/account-restore").post(body).build()).execute().use { it.isSuccessful }
                } else {
                    java.io.File(HostFiles.dataDir(app, false), "account-restore.maarse").writeBytes(file); true
                }
            }.getOrDefault(false)
            main.post {
                if (ok) send(Cmd.personSetup(req(), PersonOp.RESTORE_BACKUP, code.trim().lowercase()))
                else lastError.value = "Could not hand the backup file to the host"
            }
        }.start()
    }

    private fun bump() { msgVersion.update { it + 1 } }

    private fun setStatus(row: Long, s: String, msgId: Long? = null) { db.setStatus(row, s, msgId); bump() }

    private fun incoming(conv: String, from: String, text: String, hops: Int, notifyTitle: String) {
        db.insert(Msg(0, conv, false, from, text, System.currentTimeMillis(), "", 0, hops))
        bump()
        if (openConv != conv) {
            unread.update { it + (conv to (it[conv] ?: 0) + 1) }
            val ch = conv.startsWith("ch:")
            if (if (ch) prefs.notifyChannels else prefs.notifyMessages) Notify.message(app, conv, notifyTitle, text)
        }
    }

    private fun heard(addr: String, f: (NodeEntry) -> NodeEntry) {
        if (addr == me.value?.addr) return
        val n = f(nodes.value[addr] ?: NodeEntry(addr)).copy(lastHeard = System.currentTimeMillis())
        nodes.update { it + (addr to n) }
        db.saveNode(n)
    }

    // ---------- events ----------

    override fun onFrame(link: Link, frame: ByteArray) {
        if (link !== this.link) return
        when (val ev = decodeEvent(frame)) {
            is Ev.Info -> {
                infoAt = System.currentTimeMillis()
                me.value = Me(ev.addr, ev.maxText, ev.maxFile, ev.repeater, ev.version, me.value?.name ?: "")
                myKey.value = ev.identity
                note("Node ${shortAddr(ev.addr)} (protocol v${ev.version}, ${ev.maxText} bytes per message)")
            }
            is Ev.Name -> me.update { it?.copy(name = ev.name) }
            is Ev.Joined -> {
                channels.update { it + (ev.channel to ev.name) }
                db.saveChannel(ev.channel, ev.name)
            }
            is Ev.Heard -> {
                ev.identity?.let { k ->
                    if (keys.value[ev.addr] != k) {
                        keys.value = keys.value + (ev.addr to k)
                        prefs.keys = keys.value
                    }
                }
                // Right after connecting the node lists every node it knows
                // (as Heard with 0 hops); those are not new announces.
                if (ev.addr != me.value?.addr && System.currentTimeMillis() - infoAt > 3000) announces.update { l ->
                    val now = System.currentTimeMillis()
                    // The same node again within a minute only moves its line to the top.
                    val rest = l.filterNot { it.first == ev.addr && now - it.third < 60_000 }
                    (listOf(Triple(ev.addr, ev.hops, now)) + rest).take(200)
                }
                heard(ev.addr) { it.copy(hops = ev.hops) }
            }
            is Ev.Route -> heard(ev.dest) { it.copy(hops = ev.hops) }
            is Ev.NodeInfo -> heard(ev.addr) {
                ev.caps?.let { Pages.noteCaps(ev.addr, it) }
                it.copy(hops = ev.hops, name = ev.name ?: it.name, mv = ev.mv ?: it.mv, usb = ev.usb ?: it.usb, lat = ev.lat ?: it.lat, lon = ev.lon ?: it.lon)
            }
            is Ev.Sent -> pending.remove(ev.req)?.let { setStatus(it, "sent", ev.msgId) }
            is Ev.Acked -> db.byMsgId(ev.msgId)?.let { setStatus(it, "delivered") }
            is Ev.Deposited -> db.byMsgId(ev.msgId)?.let { setStatus(it, "mailbox") }
            is Ev.CarryAcked -> db.byMsgId(ev.msgId)?.let { setStatus(it, "carried") }
            is Ev.FileStarted, is Ev.FileIncoming, is Ev.FileChunk, is Ev.FileProgress, is Ev.FileDone -> onFileEvent(ev)
            is Ev.Call -> Calls.onData(ev.from, ev.data)
            is Ev.Backbone -> {
                if (ev.link == 0) { backbones.value = backbonesNext; backbonesNext = emptyList() }
                else backbonesNext = backbonesNext + ev
            }
            is Ev.Deleted -> {
                val conv = convOf(ev.conv)
                if (ev.flags and 2 != 0) db.deleteConv(conv)
                else db.deleteLike(conv, ev.flags and 1 != 0, String(ev.content), ev.unix * 1000)
                bump()
            }
            is Ev.Error -> {
                pending.remove(ev.req)?.let { setStatus(it, "error: ${ev.text}") }
                pendingFiles.remove(ev.req)?.let { setStatus(it, "error: ${ev.text}") }
                if (ev.code == 9) Calls.failed(ev.text)
                lastError.value = ev.text
                note("Error: ${ev.text}")
            }
            is Ev.FileLimit -> fileLimitWaiter?.invoke(ev)
            is Ev.Person -> {
                if (person.value?.person != ev.person || ev.state != PersonState.MEMBER) {
                    meshcoreSocket?.cancel()
                    meshcoreSocket = null
                    meshcoreAttempt = null
                    meshcoreDenied = false
                    meshcoreAccess.value = MeshCoreAccess()
                }
                person.value = ev
            }
            is Ev.PersonPhrase -> phrase.value = ev.phrase
            is Ev.Sensor -> Sensors.onEvent(ev)
            is Ev.PersonBackup -> backupCode.value = ev.code
            is Ev.MeshCoreProof -> acceptMeshcoreProof(ev)
            is Ev.DeviceLinked -> { linked.value = ev; note("Device linked: ${shortAddr(ev.device)} (${ev.words})") }
            is Ev.Direct -> {
                heard(ev.from) { it.copy(hops = ev.hops) }
                incoming("dm:${ev.from}", ev.from, ev.text, ev.hops, label(ev.from))
            }
            is Ev.Channel -> incoming("ch:${ev.channel}", ev.from, ev.text, ev.hops,
                "#${channels.value[ev.channel] ?: "channel"} · ${label(ev.from)}")
            is Ev.Stats -> stats.update { old ->
                // The firmware sends two frames (the first is full): merge by key.
                val fresh = ev.items.any { it.first == 1 }
                val m = if (fresh) old.filterKeys { it in 60..70 || it >= 93 }.toMutableMap() else old.toMutableMap()
                ev.items.forEach { (k, v) -> m[k] = v }
                m
            }.also { updateOffline() }
            is Ev.Radio -> radio.value = ev.cfg
            is Ev.Regions -> regions.value = ev
            is Ev.Sharing -> {
                val previous = sharing.value
                sharing.value = ev
                if (ev.prec > 0 && previous?.prec != ev.prec) updatePosition()
            }
            is Ev.Site -> site.value = ev
            is Ev.Neighbors -> neighbors.update { if (ev.part == 0) ev.items else it + ev.items }
            is Ev.Trace -> {
                traceSent.remove(ev.dest)?.let { t -> traceRtt.update { m -> m + (ev.dest to (System.currentTimeMillis() - t)) } }
                traces.update { (listOf(ev) + it).take(10) }
            }
            is Ev.Alert -> onAlert(ev)
            is Ev.Log -> note(ev.text)
            is Ev.IdentityChanged -> note("The node's identity changed: ${shortAddr(ev.addr)}")
            is Ev.HistoryInfo -> {
                hostHistory = true
                note("History on the host: ${ev.count} messages (kept for ${ev.days} days)")
                link.send(Cmd.histGet(++histReq and 0xffff, null, 0, 200))
            }
            // A bad history item must never take the app down.
            is Ev.HistoryItem -> runCatching { onHistory(ev) }.onFailure { note("History item skipped: ${it.message}") }
            is Ev.HistoryEnd -> if (ev.more && ev.conv.all { it == '0' }) link.send(Cmd.histGet(++histReq and 0xffff, null, ev.oldest, 200))
            is Ev.ReadMark -> unread.update { u -> u.filterKeys { !it.endsWith(ev.conv) } + (if (ev.unread > 0) mapOf(convOf(ev.conv) to ev.unread) else emptyMap()) }
            else -> {}
        }
    }

    private fun convOf(addr: String) = if (channels.value.containsKey(addr)) "ch:$addr" else "dm:$addr"

    private val HIST_STATUS = listOf("", "sent", "delivered", "mailbox", "carried")

    /** The host keeps every message: import into the app's database (once, by the host's id). */
    private fun onHistory(ev: Ev.HistoryItem) {
        // Files: the app keeps them itself; the host's history has only their names.
        if (ev.kind == 3) return
        val conv = (if (ev.kind == 2) "ch:" else "dm:") + ev.conv
        val text = String(ev.content)
        val st = if (ev.outgoing) HIST_STATUS.getOrElse(ev.st) { "sent" } else ""
        // A message this app sent already has a row; the host's copy only updates it.
        val existing = if (ev.outgoing && ev.msgId != 0L) db.byMsgId(ev.msgId) else null
        if (existing != null) { db.setStatus(existing, st); bump(); return }
        if (db.adopt(ev.id, conv, ev.outgoing, ev.from, text, ev.ts * 1000)) return
        val row = db.insert(Msg(0, conv, ev.outgoing, ev.from, text, ev.ts * 1000, st, ev.msgId, ev.hops), ev.id)
        if (row > 0) bump()
    }

    private fun onAlert(a: Ev.Alert) {
        alerts.update { l -> (l.filter { it.key != a.key } + a).takeLast(50) }
        val what = (Names.ALERT_STATUS.getOrElse(a.status) { "" }.let { if (it.isEmpty()) "" else "$it: " }) +
            (if (a.kind == 2) "Cancelled" else Names.ALERT_EVENTS.getOrElse(a.event) { "Alert" })
        note("Alert – $what: ${a.text.take(80)}")
        Notify.alert(app, a, what, alertLoud(a))
    }

    fun alertCancelledOrUpdated(a: Ev.Alert): Ev.Alert? =
        alerts.value.firstOrNull { it.issuer == a.issuer && it.kind != 0 && it.reference == a.serial }

    /** Loud only for an actual, severe or extreme, still valid alert. */
    fun alertLoud(a: Ev.Alert) = a.kind != 2 && a.status == 0 && a.severity >= 2 &&
        alertCancelledOrUpdated(a) == null && System.currentTimeMillis() < a.endsMs
}

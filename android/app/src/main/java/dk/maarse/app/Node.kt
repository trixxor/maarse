package dk.maarse.app

import android.content.Context
import android.os.Handler
import android.os.Looper
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update

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
    val nicknames = MutableStateFlow<Map<String, String>>(emptyMap())
    val stats = MutableStateFlow<Map<Int, Int>>(emptyMap())
    val radio = MutableStateFlow<RadioCfg?>(null)
    val regions = MutableStateFlow<Ev.Regions?>(null)
    val sharing = MutableStateFlow<Ev.Sharing?>(null)
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
        if (mode == Mode.PHONE && radio != null && ModemBridge.enabled(app)) ModemBridge.start(app, radio) else ModemBridge.stop()
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
                else WsLink("ws://127.0.0.1:${HostFiles.PORT}/ws?token=$key", "This phone", this)
            }
            Mode.REMOTE -> {
                val addr = prefs.remoteAddr.trim().removePrefix("http://").removePrefix("ws://").trimEnd('/')
                if (addr.isEmpty()) { conn.value = Conn(mode, Conn.State.OFF, "Enter the host's address in Settings"); prefs.running = false; null }
                else WsLink("ws://$addr/ws?token=${prefs.remoteKey.trim()}", "Host $addr", this)
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
        // Only a radio has radio settings (a host answers with an error).
        if (prefs.mode == Mode.BLE) link.send(Cmd.getRadio())
        person.value = null
        if (prefs.mode != Mode.BLE) link.send(Cmd.getPerson())
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

    fun setPrecision(ctx: Context, prec: Int) {
        val sh = sharing.value
        send(Cmd.setSharing(prec, ((sh?.lat ?: 0.0) * 1e7).toInt(), ((sh?.lon ?: 0.0) * 1e7).toInt(), sh?.telemetry ?: false))
        send(Cmd.getSharing())
        if (prec > 0) updatePosition(ctx)
    }

    /** The phone's location to the node (only while sharing is on). */
    @android.annotation.SuppressLint("MissingPermission")
    fun updatePosition(ctx: Context = app) {
        val sh = sharing.value ?: return
        if (sh.prec == 0 || !hasLocation(ctx)) return
        lastPosition = System.currentTimeMillis()
        val lm = ctx.getSystemService(android.location.LocationManager::class.java)
        val use = { l: android.location.Location? ->
            if (l != null) main.post {
                val cur = sharing.value ?: return@post
                send(Cmd.setSharing(cur.prec, (l.latitude * 1e7).toInt(), (l.longitude * 1e7).toInt(), cur.telemetry))
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
    private fun fileMax(): Int {
        val m = me.value?.maxFile?.toInt()?.takeIf { it > 0 } ?: 8192
        return if (prefs.mode == Mode.BLE) minOf(m, 8192) else m
    }

    /** Sends a file (a picture is made smaller to fit). Returns an error or null. */
    fun sendFile(conv: String, data0: ByteArray, mime0: String, name0: String): String? {
        if (!conv.startsWith("dm:")) return "Files and voice messages go to one person, not to a channel"
        if (offline.value || conn.value.state != Conn.State.ONLINE) return "Not connected — files are not queued; send it when you are online"
        val max = fileMax()
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
            is Ev.FileIncoming -> {
                if (ev.size <= 0 || ev.size > 16 * 1024 * 1024) return
                heard(ev.from) { it }
                val conv = "dm:${ev.from}"
                val row = db.insert(Msg(0, conv, false, ev.from, fileLabel(ev.mime, ev.name, null) + " — receiving…", System.currentTimeMillis(), "", 0, 0))
                db.setFile(row, null, ev.mime)
                fileIn[ev.fileId] = Incoming(row, ByteArray(ev.size), ev.mime, ev.name, ev.from)
                bump()
            }
            is Ev.FileChunk -> fileIn[ev.fileId]?.let { f ->
                val n = minOf(ev.data.size, f.buf.size - ev.offset)
                if (ev.offset >= 0 && n > 0) System.arraycopy(ev.data, 0, f.buf, ev.offset, n)
            }
            is Ev.FileProgress -> if (ev.outgoing) fileOut[ev.fileId]?.let { setStatus(it, "sending ${ev.done}/${ev.total}") }
                else fileIn[ev.fileId]?.let { f ->
                    db.setFile(f.row, null, f.mime, fileLabel(f.mime, f.name, null) + " — receiving ${100 * ev.done / maxOf(1, ev.total)} %"); bump()
                }
            is Ev.FileDone -> if (ev.outgoing) fileOut.remove(ev.fileId)?.let { row ->
                setStatus(row, when { !ev.ok -> "error: ${ev.text}"; ev.text.contains("mailbox") -> "mailbox"; else -> "delivered" })
            } else fileIn.remove(ev.fileId)?.let { f ->
                if (!ev.ok) { db.setFile(f.row, null, f.mime, fileLabel(f.mime, f.name, null) + " — not received (${ev.text})"); bump(); return }
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

    fun joinChannel(name: String) { send(Cmd.join(name.trim().removePrefix("#"))) }

    fun setNickname(addr: String, nick: String) {
        val m = nicknames.value.toMutableMap()
        if (nick.isBlank()) m.remove(addr) else m[addr] = nick.trim()
        nicknames.value = m
        prefs.nicknames = m
    }

    fun markRead(conv: String) {
        if ((unread.value[conv] ?: 0) == 0) return
        unread.update { it - conv }
        if (hostHistory) send(Cmd.histMarkRead(conv.substringAfter(":"), 0))
        Notify.cancel(app, conv)
    }

    fun dismissAlert(a: Ev.Alert) {
        dismissed.update { it + a.key }
        prefs.dismissedAlerts = dismissed.value
    }

    fun trace(dest: String) { send(Cmd.traceRoute(req(), dest)) }

    fun personSetup(op: Int, text: String = "") { lastError.value = null; send(Cmd.personSetup(req(), op, text)) }
    fun linkDevice(device: String) { lastError.value = null; send(Cmd.linkDevice(req(), device)) }

    // ---------- the account backup file on the host ----------

    private val http by lazy { okhttp3.OkHttpClient() }

    private fun remoteUrl(path: String): String {
        val addr = prefs.remoteAddr.trim().removePrefix("http://").removePrefix("ws://").trimEnd('/')
        return "http://$addr$path?token=${prefs.remoteKey.trim()}"
    }

    /** The backup the host just wrote (phone: its folder; elsewhere: over HTTP). */
    fun readBackup(done: (ByteArray?) -> Unit) {
        Thread {
            val b = runCatching {
                if (prefs.mode == Mode.REMOTE) http.newCall(okhttp3.Request.Builder().url(remoteUrl("/api/account-backup")).build()).execute().use { r -> if (r.isSuccessful) r.body?.bytes() else null }
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
                    http.newCall(okhttp3.Request.Builder().url(remoteUrl("/api/account-restore")).post(body).build()).execute().use { it.isSuccessful }
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
                me.value = Me(ev.addr, ev.maxText, ev.maxFile, ev.repeater, ev.version, me.value?.name ?: "")
                note("Node ${shortAddr(ev.addr)} (protocol v${ev.version}, ${ev.maxText} bytes per message)")
            }
            is Ev.Name -> me.update { it?.copy(name = ev.name) }
            is Ev.Joined -> {
                channels.update { it + (ev.channel to ev.name) }
                db.saveChannel(ev.channel, ev.name)
            }
            is Ev.Heard -> heard(ev.addr) { it.copy(hops = ev.hops) }
            is Ev.Route -> heard(ev.dest) { it.copy(hops = ev.hops) }
            is Ev.NodeInfo -> heard(ev.addr) {
                it.copy(hops = ev.hops, name = ev.name ?: it.name, mv = ev.mv ?: it.mv, usb = ev.usb ?: it.usb, lat = ev.lat ?: it.lat, lon = ev.lon ?: it.lon)
            }
            is Ev.Sent -> pending.remove(ev.req)?.let { setStatus(it, "sent", ev.msgId) }
            is Ev.Acked -> db.byMsgId(ev.msgId)?.let { setStatus(it, "delivered") }
            is Ev.Deposited -> db.byMsgId(ev.msgId)?.let { setStatus(it, "mailbox") }
            is Ev.CarryAcked -> db.byMsgId(ev.msgId)?.let { setStatus(it, "carried") }
            is Ev.FileStarted, is Ev.FileIncoming, is Ev.FileChunk, is Ev.FileProgress, is Ev.FileDone -> onFileEvent(ev)
            is Ev.Call -> Calls.onData(ev.from, ev.data)
            is Ev.Error -> {
                pending.remove(ev.req)?.let { setStatus(it, "error: ${ev.text}") }
                pendingFiles.remove(ev.req)?.let { setStatus(it, "error: ${ev.text}") }
                if (ev.code == 9) Calls.failed(ev.text)
                lastError.value = ev.text
                note("Error: ${ev.text}")
            }
            is Ev.Person -> person.value = ev
            is Ev.PersonPhrase -> phrase.value = ev.phrase
            is Ev.PersonBackup -> backupCode.value = ev.code
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
            is Ev.Sharing -> sharing.value = ev
            is Ev.Neighbors -> neighbors.update { if (ev.part == 0) ev.items else it + ev.items }
            is Ev.Trace -> traces.update { (listOf(ev) + it).take(10) }
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

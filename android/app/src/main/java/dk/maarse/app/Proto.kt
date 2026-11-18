package dk.maarse.app

// The companion protocol (see maarse-core/src/companion.rs): the same frames
// over Bluetooth (Nordic UART) and over WebSocket to maarse-host.
//
//   length (u16, little endian) | type (u8) | content
//
// Commands have type < 0x80; replies and events >= 0x80.

import java.io.ByteArrayOutputStream
import java.util.UUID

object Nus {
    val SERVICE: UUID = UUID.fromString("6e400001-b5a3-f393-e0a9-e50e24dcca9e")
    val RX: UUID = UUID.fromString("6e400002-b5a3-f393-e0a9-e50e24dcca9e") // we write here
    val TX: UUID = UUID.fromString("6e400003-b5a3-f393-e0a9-e50e24dcca9e") // the node notifies
    val CCCD: UUID = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb")
}

const val MAX_FRAME = 300

fun ByteArray.hex(): String = joinToString("") { "%02x".format(it.toInt() and 0xff) }
fun String.unhex(): ByteArray = chunked(2).map { it.toInt(16).toByte() }.toByteArray()
fun shortAddr(h: String) = h.take(8)

private class W {
    val b = ByteArrayOutputStream()
    fun u8(v: Int) = apply { b.write(v and 0xff) }
    fun u16(v: Int) = apply { u8(v); u8(v shr 8) }
    fun u32(v: Long) = apply { u16((v and 0xffff).toInt()); u16(((v shr 16) and 0xffff).toInt()) }
    fun u64(v: Long) = apply { for (i in 0 until 8) u8((v shr (8 * i)).toInt()) }
    fun bytes(v: ByteArray) = apply { b.write(v) }
    fun text(s: String) = apply { b.write(s.toByteArray()) }
    fun addr(h: String) = apply { b.write(h.unhex()) }
}

private fun frame(kind: Int, body: W.() -> Unit = {}): ByteArray {
    val w = W().u8(kind)
    w.body()
    val inner = w.b.toByteArray()
    return byteArrayOf((inner.size and 0xff).toByte(), (inner.size shr 8).toByte()) + inner
}

/** Commands from the app to the node. */
object Cmd {
    fun getInfo() = frame(0x01)
    fun sendDirect(req: Int, dest: String, text: String) = frame(0x02) { u16(req); addr(dest); text(text) }
    fun sendChannel(req: Int, ch: String, text: String) = frame(0x03) { u16(req); addr(ch); text(text) }
    fun join(name: String) = frame(0x04) { text(name) }
    fun announce() = frame(0x05)
    fun getStats() = frame(0x09)
    fun setRadio(c: RadioCfg) = frame(0x0a) {
        u32(c.freq); u32(c.bw); u8(c.sf); u8(c.cr); u8(c.power); u16(c.duty); u16(c.preamble); u8(c.announce)
    }
    fun getRadio() = frame(0x0b)
    fun remote(req: Int, dest: String, inner: ByteArray) = frame(0x0c) { u16(req); addr(dest); bytes(inner.copyOfRange(2, inner.size)) }
    fun getAdmins() = frame(0x0e)
    fun setTime() = frame(0x0f) { u32(System.currentTimeMillis() / 1000) }
    fun setRole(r: Int) = frame(0x10) { u8(r) }
    fun setRegions(strict: Boolean, names: String) = frame(0x11) { u8(if (strict) 1 else 0); text(names) }
    fun getRegions() = frame(0x12)
    fun joinPrivate(key: ByteArray, name: String) = frame(0x13) { bytes(key); text(name) }
    fun traceRoute(req: Int, dest: String) = frame(0x20) { u16(req); addr(dest) }
    fun getNeighbors() = frame(0x21)
    fun setSharing(prec: Int, latE7: Int, lonE7: Int, tel: Boolean) = frame(0x22) {
        u8(prec); u32(latE7.toLong()); u32(lonE7.toLong()); u8(if (tel) 1 else 0)
    }
    fun getSharing() = frame(0x23)
    fun setName(name: String) = frame(0x24) { text(name) }
    fun getName() = frame(0x25)
    fun setModemKey(pass: String) = frame(0x2d) { text(pass) }
    fun sendAlert(req: Int, m: AlertForm, text: String) = frame(0x2c) {
        u16(req); u8(m.status); u8(m.kind); u8(m.event); u8(m.severity); u16(m.expires); u32(m.reference); u8(0); text(text)
    }
    // History on maarse-host (maarse-host/src/history.rs).
    fun histGet(req: Int, conv: String?, before: Long, limit: Int) = frame(0x30) {
        u16(req); if (conv != null) addr(conv) else bytes(ByteArray(16)); u64(before); u8(limit)
    }
    fun histMarkRead(conv: String, upTo: Long) = frame(0x31) { addr(conv); u64(upTo) }
    /** maarse-host: the backbones it is linked to (answer: Ev.Backbone per link, then link 0). */
    fun getBackbones() = frame(0x39) {}
    /** flags: 1 outgoing, 2 the whole conversation. */
    fun histDelete(conv: String, flags: Int, unix: Long, content: ByteArray) = frame(0x33) { addr(conv); u8(flags); u32(unix); bytes(content) }
    // Files (the same frames as the web client's sendFile).
    fun fileBegin(req: Int, dest: String, size: Int, mime: String, name: String) = frame(0x06) {
        val m = mime.toByteArray().copyOf(minOf(mime.toByteArray().size, 60))
        u16(req); addr(dest); u32(size.toLong()); u8(m.size); bytes(m); bytes(name.toByteArray().let { it.copyOf(minOf(it.size, 60)) })
    }
    fun fileData(req: Int, offset: Int, data: ByteArray) = frame(0x07) { u16(req); u32(offset.toLong()); bytes(data) }
    fun fileEnd(req: Int) = frame(0x08) { u16(req) }
    fun cancelFile(fileId: Long) = frame(0x38) { u64(fileId) }
    fun getFileLimit(dest: String) = frame(0x14) { addr(dest) }
    /** Asks a node for a maarse:// page or a picture on one (0x26). */
    fun requestPage(req: Int, dest: String, path: String) = frame(0x26) { u16(req); addr(dest); text(path) }
    /** Call signalling and audio (0x27): content belongs to the clients, see Calls.kt. */
    fun sendCall(dest: String, data: ByteArray) = frame(0x27) { addr(dest); bytes(data) }
    // Linked devices (maarse-host only).
    fun linkDevice(req: Int, device: String) = frame(0x35) { u16(req); addr(device) }
    fun getPerson() = frame(0x36)
    fun personSetup(req: Int, op: Int, text: String = "") = frame(0x37) { u16(req); u8(op); text(text) }
}

/** `PersonSetup` operations and `Person` states (maarse-core companion.rs). */
object PersonOp { const val CREATE = 1; const val RESTORE = 2; const val AWAIT_LINK = 3; const val LEAVE = 4; const val CANCEL = 5; const val REMOVE = 6; const val BACKUP = 7; const val RESTORE_BACKUP = 8; const val SYNC = 9 }
object PersonState { const val NONE = 0; const val AWAITING = 1; const val MEMBER = 2 }

data class RadioCfg(val freq: Long, val bw: Long, val sf: Int, val cr: Int, val power: Int, val duty: Int, val preamble: Int, val announce: Int)
data class AlertForm(val status: Int, val kind: Int, val event: Int, val severity: Int, val expires: Int, val reference: Long)
data class Neighbor(val id: Int, val prefix: String, val snr: Int, val rssi: Int, val ageS: Long, val count: Int)

/** Replies and events from the node. */
sealed class Ev {
    data class Info(val addr: String, val repeater: Boolean, val version: Int, val maxText: Int, val maxFile: Long) : Ev()
    data class Sent(val req: Int, val msgId: Long) : Ev()
    data class Direct(val from: String, val hops: Int, val text: String) : Ev()
    data class Channel(val channel: String, val from: String, val hops: Int, val text: String) : Ev()
    data class Acked(val msgId: Long) : Ev()
    data class Route(val dest: String, val hops: Int) : Ev()
    data class Heard(val addr: String, val hops: Int) : Ev()
    data class Joined(val channel: String, val name: String) : Ev()
    data class Log(val text: String) : Ev()
    data class Error(val req: Int, val code: Int, val text: String) : Ev()
    data class Deposited(val msgId: Long) : Ev()
    data class CarryAcked(val msgId: Long) : Ev()
    data class Stats(val items: List<Pair<Int, Int>>) : Ev()
    data class Radio(val cfg: RadioCfg) : Ev()
    data class Regions(val strict: Boolean, val names: String) : Ev()
    data class RemoteReply(val from: String, val inner: Ev?) : Ev()
    data class Trace(val req: Int, val dest: String, val path: List<Int>, val out: List<Int>, val back: List<Int>) : Ev()
    data class Neighbors(val part: Int, val last: Boolean, val items: List<Neighbor>) : Ev()
    data class Sharing(val prec: Int, val lat: Double, val lon: Double, val telemetry: Boolean) : Ev()
    data class Name(val name: String) : Ev()
    data class NodeInfo(val addr: String, val hops: Int, val name: String?, val mv: Int?, val usb: Boolean?, val lat: Double?, val lon: Double?, val caps: Int? = null) : Ev()
    data class IdentityChanged(val addr: String) : Ev()
    data class Alert(
        val issuer: String, val serial: Long, val issued: Long, val status: Int, val kind: Int, val event: Int,
        val severity: Int, val expires: Int, val reference: Long, val area: Triple<Double, Double, Double>?,
        val hops: Int, val trust: Int, val name: String, val text: String,
    ) : Ev() {
        val key get() = "$issuer:$serial"
        val endsMs get() = (issued + maxOf(1, expires) * 60L) * 1000
    }
    data class HistoryItem(
        val id: Long, val ts: Long, val outgoing: Boolean, val live: Boolean, val kind: Int, val st: Int,
        val hops: Int, val msgId: Long, val conv: String, val from: String, val content: ByteArray,
    ) : Ev()
    data class HistoryEnd(val req: Int, val conv: String, val count: Int, val more: Boolean, val oldest: Long) : Ev()
    data class ReadMark(val conv: String, val upTo: Long, val unread: Int) : Ev()
    data class HistoryInfo(val days: Int, val count: Long) : Ev()
    data class Person(
        val state: Int, val master: Boolean, val person: String, val index: Int, val version: Long,
        val devices: List<Pair<String, Boolean>>, val words: String,
    ) : Ev()
    data class DeviceLinked(val device: String, val index: Int, val words: String) : Ev()
    data class PersonPhrase(val phrase: String) : Ev()
    data class PersonBackup(val code: String) : Ev()
    data class FileIncoming(val fileId: Long, val from: String, val size: Int, val mime: String, val name: String) : Ev()
    data class FileChunk(val fileId: Long, val offset: Int, val data: ByteArray) : Ev()
    data class FileProgress(val fileId: Long, val done: Int, val total: Int, val outgoing: Boolean) : Ev()
    data class FileDone(val fileId: Long, val ok: Boolean, val outgoing: Boolean, val text: String) : Ev()
    data class FileStarted(val req: Int, val fileId: Long, val chunks: Int) : Ev()
    data class FileLimit(val dest: String, val max: Long, val viaLora: Boolean, val mailbox: Boolean) : Ev()
    data class Call(val from: String, val data: ByteArray) : Ev()
    /** flags: 1 server, 2 radio, 4 post office, 8 relay, 16 call test. link 0 ends the list. */
    data class Backbone(val link: Int, val flags: Int, val upSecs: Long, val addr: String, val version: String, val name: String, val regions: Int) : Ev()
    data class Deleted(val conv: String, val flags: Int, val unix: Long, val content: ByteArray) : Ev()
    data class Unknown(val kind: Int) : Ev()
}

private class Rd(val f: ByteArray) {
    fun u8(o: Int) = if (o < f.size) f[o].toInt() and 0xff else 0
    fun s8(o: Int) = if (o < f.size) f[o].toInt() else 0
    fun u16(o: Int) = u8(o) or (u8(o + 1) shl 8)
    fun s16(o: Int) = u16(o).toShort().toInt()
    fun u32(o: Int): Long = (u16(o).toLong() or (u16(o + 2).toLong() shl 16))
    fun i32(o: Int): Int = u32(o).toInt()
    fun u64(o: Int): Long { var v = 0L; for (i in 7 downTo 0) v = (v shl 8) or u8(o + i).toLong(); return v }
    fun addr(o: Int) = if (o + 16 <= f.size) f.copyOfRange(o, o + 16).hex() else ""
    fun txt(o: Int, end: Int = f.size) = if (o < end) String(f, o, end - o) else ""
}

fun decodeEvent(f: ByteArray): Ev {
    if (f.isEmpty()) return Ev.Unknown(0)
    val r = Rd(f)
    return when (val t = r.u8(0)) {
        0x81 -> Ev.Info(r.addr(1), r.u8(17) != 0, r.u8(18), r.u8(19), if (f.size >= 24) r.u32(20) else 0)
        0x82 -> Ev.Sent(r.u16(1), r.u64(3))
        0x83 -> Ev.Direct(r.addr(1), r.u8(17), r.txt(18))
        0x84 -> Ev.Channel(r.addr(1), r.addr(17), r.u8(33), r.txt(34))
        0x85 -> Ev.Acked(r.u64(1))
        0x86 -> Ev.Route(r.addr(1), r.u8(17))
        0x87 -> Ev.Heard(r.addr(1), r.u8(17))
        0x88 -> Ev.Joined(r.addr(1), r.txt(17))
        0x8e -> Ev.Log(r.txt(1))
        0x8f -> Ev.Error(r.u16(1), r.u8(3), r.txt(4))
        0x90 -> Ev.Deposited(r.u64(1))
        0x91 -> Ev.Stats((1 until f.size - 4 step 5).map { r.u8(it) to r.i32(it + 1) })
        0x92 -> Ev.Radio(RadioCfg(r.u32(1), r.u32(5), r.u8(9), r.u8(10), r.s8(11), r.u16(12), r.u16(14), r.u8(16)))
        0x93 -> Ev.RemoteReply(r.addr(1), if (f.size > 17) decodeEvent(f.copyOfRange(17, f.size)) else null)
        0x95 -> Ev.Regions(r.u8(1) != 0, r.txt(2))
        0xa0 -> {
            val k = r.u8(19)
            val ids = (0 until k).map { r.u16(20 + 2 * it) }
            val m = r.u8(20 + 2 * k)
            val o = 21 + 2 * k
            Ev.Trace(r.u16(1), r.addr(3), ids, (o until minOf(o + m, f.size)).map { r.s8(it) }, (o + m until f.size).map { r.s8(it) })
        }
        0xa1 -> Ev.Neighbors(r.u8(1), r.u8(2) != 0, (3..f.size - 15 step 15).map { o ->
            Neighbor(r.u16(o), f.copyOfRange(o + 2, o + 6).hex(), r.s8(o + 6), r.s16(o + 7), r.u32(o + 9), r.u16(o + 13))
        })
        0xa2 -> Ev.Sharing(r.u8(1), r.i32(2) / 1e7, r.i32(6) / 1e7, r.u8(10) != 0)
        0xa3 -> nodeInfo(r.addr(1), r.u8(17), f.copyOfRange(minOf(18, f.size), f.size))
        0xa4 -> Ev.Name(r.txt(1))
        0xa7 -> Ev.CarryAcked(r.u64(1))
        0xac -> alert(r)
        0xb4 -> Ev.IdentityChanged(r.addr(1))
        0x89 -> { val ml = r.u8(29); Ev.FileIncoming(r.u64(1), r.addr(9), r.u32(25).toInt(), r.txt(30, minOf(30 + ml, f.size)), r.txt(minOf(30 + ml, f.size))) }
        0x8a -> Ev.FileChunk(r.u64(1), r.u32(9).toInt(), f.copyOfRange(minOf(13, f.size), f.size))
        0x8b -> Ev.FileProgress(r.u64(1), r.u16(9), r.u16(11), r.u8(13) != 0)
        0x8c -> Ev.FileDone(r.u64(1), r.u8(9) != 0, r.u8(10) != 0, r.txt(11))
        0x8d -> Ev.FileStarted(r.u16(1), r.u64(3), r.u16(11))
        0x96 -> Ev.FileLimit(r.addr(1), r.u32(17), r.u8(21) and 1 != 0, r.u8(21) and 2 != 0)
        0xba -> {
            val vl = r.u8(24); val nl = r.u8(25 + vl)
            fun s(o: Int, n: Int) = if (o + n <= f.size) String(f, o, n) else ""
            Ev.Backbone(r.u16(1), r.u8(3), r.u32(4), r.addr(8), s(25, vl), s(26 + vl, nl), r.u8(26 + vl + nl))
        }
        0xb9 -> Ev.Deleted(r.addr(1), r.u8(17), r.u32(18), f.copyOfRange(minOf(22, f.size), f.size))
        0xa5 -> Ev.Call(r.addr(1), f.copyOfRange(minOf(17, f.size), f.size))
        0xb5 -> Ev.DeviceLinked(r.addr(1), r.u8(17), r.txt(18))
        0xb6 -> {
            val n = r.u8(24)
            Ev.Person(r.u8(1), r.u8(2) and 1 != 0, r.addr(3), r.u8(19), r.u32(20),
                (0 until n).map { r.addr(25 + 17 * it) to (r.u8(41 + 17 * it) != 0) }, r.txt(25 + 17 * n))
        }
        0xb7 -> Ev.PersonPhrase(r.txt(1))
        0xb8 -> Ev.PersonBackup(r.txt(1))
        0xb0 -> Ev.HistoryItem(r.u64(1), r.u32(9), r.u8(13) and 1 != 0, r.u8(13) and 2 != 0, r.u8(14), r.u8(15),
            r.u8(16), r.u64(17), r.addr(25), r.addr(41), f.copyOfRange(minOf(57, f.size), f.size))
        0xb1 -> Ev.HistoryEnd(r.u16(1), r.addr(3), r.u8(19), r.u8(20) != 0, r.u64(21))
        0xb2 -> Ev.ReadMark(r.addr(1), r.u64(17), r.u16(25))
        0xb3 -> Ev.HistoryInfo(r.u16(2), r.u32(4))
        else -> Ev.Unknown(t)
    }
}

/** App data fields as in maarse-core::appdata (0x03 position, 0x04 telemetry, 0x05 name). */
private fun nodeInfo(addr: String, hops: Int, b: ByteArray): Ev.NodeInfo {
    var name: String? = null; var mv: Int? = null; var usb: Boolean? = null; var lat: Double? = null; var lon: Double? = null; var caps: Int? = null
    var i = 0
    while (i + 1 < b.size) {
        val t = b[i].toInt() and 0xff; val n = b[i + 1].toInt() and 0xff
        val d = Rd(b.copyOfRange(i + 2, minOf(i + 2 + n, b.size)))
        if (t == 0x03 && n >= 9) { lat = d.i32(1) / 1e7; lon = d.i32(5) / 1e7 }
        if (t == 0x04 && n >= 10) { mv = d.u16(0); usb = d.u8(2) and 1 != 0 }
        if (t == 0x05 && n > 0) name = d.txt(0).filter { it >= ' ' && it != '\u007f' }.trim()
        if (t == 0x07 && n >= 1) caps = d.u8(0)
        i += 2 + n
    }
    return Ev.NodeInfo(addr, hops, name, mv, usb, lat, lon, caps)
}

private fun alert(r: Rd): Ev.Alert {
    var o = 36
    var area: Triple<Double, Double, Double>? = null
    if (r.u8(35) == 1) { area = Triple(r.i32(36) / 1e5, r.i32(40) / 1e5, r.u16(44) / 10.0); o = 46 }
    val n = r.u8(o + 2)
    return Ev.Alert(r.addr(1), r.u32(17), r.u32(21), r.u8(25), r.u8(26), r.u8(27), r.u8(28), r.u16(29), r.u32(31),
        area, r.u8(o), r.u8(o + 1), r.txt(o + 3, minOf(o + 3 + n, r.f.size)), r.txt(minOf(o + 3 + n, r.f.size)))
}

/** Splits a byte stream (Bluetooth) into frames. */
class FrameDecoder {
    private var buf = ByteArray(0)
    fun push(bytes: ByteArray): List<ByteArray> {
        buf += bytes
        val out = mutableListOf<ByteArray>()
        while (buf.size >= 2) {
            val len = (buf[0].toInt() and 0xff) or ((buf[1].toInt() and 0xff) shl 8)
            if (len == 0 || len > MAX_FRAME) { buf = buf.copyOfRange(1, buf.size); continue }
            if (buf.size < 2 + len) break
            out += buf.copyOfRange(2, 2 + len)
            buf = buf.copyOfRange(2 + len, buf.size)
        }
        return out
    }
}

// ---------- names for what the node reports ----------

object Names {
    val ROLES = listOf("Client", "Repeater", "Relay", "Anchor", "Auto")
    val ROLE_HELP = listOf(
        "Does not forward. For a phone's radio in a town with repeaters.",
        "Forwards for others. For fixed nodes on a mast or roof.",
        "Forest mode: forwards and carries messages (store-carry-forward) where there are no repeaters.",
        "Repeater that splits long paths and answers on behalf of others.",
        "Handheld: becomes a relay by itself when no repeater has been heard for 2 minutes.",
    )
    val ALERT_EVENTS = listOf("Information", "Evacuate", "Take shelter", "Fire", "Flood", "Drinking water",
        "Chemical release", "Power outage", "Severe weather", "Missing person")
    val ALERT_SEVERITY = listOf("Minor", "Moderate", "Severe", "Extreme")
    val ALERT_STATUS = listOf("", "Exercise", "Test", "System")
    val ENERGY = listOf("Critical", "Low", "Ok", "Full")
}

/** A status key: group, label and how the value is shown. */
data class StatKey(val group: String, val label: String, val unit: String = "")

val STATS: Map<Int, StatKey> = mapOf(
    1 to StatKey("General", "Uptime", "time"), 7 to StatKey("General", "Role", "role"),
    2 to StatKey("General", "Known nodes"), 3 to StatKey("General", "Known routes"),
    4 to StatKey("General", "Channels"), 5 to StatKey("General", "Links (TCP/serial)"), 6 to StatKey("General", "Connected clients"),
    45 to StatKey("Radio", "Frequency", "MHz"), 47 to StatKey("Radio", "Bandwidth", "kHz"), 46 to StatKey("Radio", "Spreading factor", "SF"),
    48 to StatKey("Radio", "Transmit power", "dBm"), 34 to StatKey("Radio", "Latest RSSI", "dBm"), 35 to StatKey("Radio", "Latest SNR", "dB"),
    36 to StatKey("Radio", "Average RSSI", "dBm"), 37 to StatKey("Radio", "Average SNR", "dB"), 50 to StatKey("Radio", "Weakest RSSI", "dBm"),
    51 to StatKey("Radio", "Strongest RSSI", "dBm"), 30 to StatKey("Radio", "Packets received"), 32 to StatKey("Radio", "Packets sent"),
    42 to StatKey("Radio", "Data received", "bytes"), 43 to StatKey("Radio", "Data sent", "bytes"), 40 to StatKey("Radio", "Total airtime", "ms"),
    41 to StatKey("Radio", "Duty cycle used (last hour)", "permille"), 44 to StatKey("Radio", "Transmit queue"),
    38 to StatKey("Radio", "Channel busy (CAD)"), 39 to StatKey("Radio", "Deferred due to duty cycle"),
    31 to StatKey("Radio", "Receive errors (CRC/header)"), 33 to StatKey("Radio", "Transmit errors"), 52 to StatKey("Radio", "Packets dropped"),
    53 to StatKey("Radio", "Radio driver stage"), 54 to StatKey("Radio", "Radio loop restarts"),
    49 to StatKey("Power and diagnostics", "Battery", "mV"), 85 to StatKey("Power and diagnostics", "Battery (approx.)", "%"),
    86 to StatKey("Power and diagnostics", "Chip temperature", "temp"), 80 to StatKey("Power and diagnostics", "Noise floor (estimated)", "dBm"),
    81 to StatKey("Power and diagnostics", "Energy status", "energy"), 82 to StatKey("Power and diagnostics", "USB power", "usb"),
    83 to StatKey("Power and diagnostics", "Direct neighbours"), 84 to StatKey("Power and diagnostics", "Sharing position", "precision"),
    87 to StatKey("Power and diagnostics", "Free stack", "bytes"), 88 to StatKey("Power and diagnostics", "Free heap", "bytes"),
    89 to StatKey("Power and diagnostics", "Lowest free heap", "bytes"), 90 to StatKey("Power and diagnostics", "Crashes"),
    91 to StatKey("Power and diagnostics", "Last panic line"), 92 to StatKey("Power and diagnostics", "Last panic file (hash)"),
    10 to StatKey("Routing", "Packets received"), 11 to StatKey("Routing", "Invalid packets"), 12 to StatKey("Routing", "Duplicates"),
    13 to StatKey("Routing", "Forwarded for others"), 14 to StatKey("Routing", "Originated"), 15 to StatKey("Routing", "Delivered to me"),
    16 to StatKey("Routing", "Acknowledgements received"), 17 to StatKey("Routing", "Routes found"), 18 to StatKey("Routing", "Route requests"),
    19 to StatKey("Routing", "Hops retransmitted"), 20 to StatKey("Routing", "Floods cancelled"), 77 to StatKey("Routing", "Without proof of work"),
    93 to StatKey("Routing", "Hellos sent"), 94 to StatKey("Routing", "Key requests"), 95 to StatKey("Routing", "Local route repairs"),
    99 to StatKey("Routing", "Dropped by quota"),
    96 to StatKey("Carry and contacts", "Messages carried for others"), 98 to StatKey("Carry and contacts", "Mobile mode", "bool"),
    97 to StatKey("Carry and contacts", "Pinned contacts"), 100 to StatKey("Carry and contacts", "Waiting for a key"),
    101 to StatKey("Carry and contacts", "Outbox"),
    60 to StatKey("Mailbox and files", "Recipients with mail"), 61 to StatKey("Mailbox and files", "Messages in the mailbox"),
    64 to StatKey("Mailbox and files", "Messages without acknowledgement"), 69 to StatKey("Mailbox and files", "Put in others' mailbox"),
    70 to StatKey("Mailbox and files", "Delivered from the mailbox"), 62 to StatKey("Mailbox and files", "Files being sent"),
    63 to StatKey("Mailbox and files", "Files being received"),
    67 to StatKey("Network", "Packets in"), 68 to StatKey("Network", "Packets out"), 65 to StatKey("Network", "Data in", "bytes"),
    66 to StatKey("Network", "Data out", "bytes"), 71 to StatKey("Network", "Errors"), 102 to StatKey("Network", "Wi-Fi modem fell back to a node", "bool"),
    72 to StatKey("DHT", "Contacts"), 73 to StatKey("DHT", "Records stored"), 74 to StatKey("DHT", "Lookups"),
    75 to StatKey("DHT", "Found"), 76 to StatKey("DHT", "Direct gateways"),
)
val STAT_GROUPS = listOf("General", "Carry and contacts", "Mailbox and files", "Routing", "Radio", "Power and diagnostics", "Network", "DHT")

val PRECISION = listOf("Off", "About 10 km", "About 1 km", "Precise")

fun formatStat(k: Int, v: Int): String {
    val unit = STATS[k]?.unit ?: ""
    return when (unit) {
        "time" -> { val h = v / 3600; val m = v / 60 % 60; if (h >= 24) "${h / 24} d ${h % 24} h" else if (h > 0) "$h h $m min" else "$m min ${v % 60} s" }
        "role" -> Names.ROLES.getOrElse(v) { "$v" }
        "energy" -> Names.ENERGY.getOrElse(v) { "$v" }
        "bool" -> if (v != 0) "yes" else "no"
        "usb" -> if (v < 0) "unknown" else if (v != 0) "yes" else "no"
        "MHz" -> "%.3f MHz".format(v / 1000.0)
        "SF" -> "SF$v"
        "ms" -> if (v < 10000) "$v ms" else "%.1f s".format(v / 1000.0)
        "permille" -> "%.1f %%".format(v / 10.0)
        "temp" -> "%.1f °C".format(v / 10.0)
        "%" -> "$v %"
        "precision" -> PRECISION.getOrElse(v) { "$v" }
        "bytes" -> if (v < 10240) "$v B" else if (v < 10485760) "%.1f KB".format(v / 1024.0) else "%.1f MB".format(v / 1048576.0)
        "" -> "$v"
        else -> "$v $unit"
    }
}

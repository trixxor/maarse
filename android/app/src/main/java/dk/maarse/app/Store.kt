package dk.maarse.app

import android.content.ContentValues
import android.content.Context
import android.database.sqlite.SQLiteDatabase
import android.database.sqlite.SQLiteOpenHelper
import org.json.JSONObject

/** How the app reaches a node. */
enum class Mode(val title: String, val help: String) {
    BLE("Radio over Bluetooth", "The app talks to your Må:rse radio. The radio is the node."),
    PHONE("Phone as node", "The phone runs maarse-host: its own identity on the internet backbone, mail kept by post offices."),
    REMOTE("Host elsewhere", "Follow a maarse-host on another machine (home PC, over VPN) with its access key."),
    SERVER("Server on this phone", "An old phone on Wi-Fi and a charger as a backbone node and post office. Stops when Wi-Fi or power goes."),
}

/** A message in a conversation. `conv` is "dm:<address>" or "ch:<channel address>". */
data class Msg(
    val id: Long,
    val conv: String,
    val mine: Boolean,
    val from: String,
    val text: String,
    val ts: Long,
    val status: String,
    val msgId: Long,
    val hops: Int,
)

/** A node we have heard of. */
data class NodeEntry(
    val addr: String,
    val name: String? = null,
    val hops: Int? = null,
    val lastHeard: Long = 0,
    val mv: Int? = null,
    val usb: Boolean? = null,
    val lat: Double? = null,
    val lon: Double? = null,
)

class Db(ctx: Context) : SQLiteOpenHelper(ctx, "maarse.db", null, 1) {
    override fun onCreate(db: SQLiteDatabase) {
        db.execSQL("""CREATE TABLE msg (id INTEGER PRIMARY KEY AUTOINCREMENT, conv TEXT NOT NULL, mine INTEGER NOT NULL,
            sender TEXT NOT NULL, text TEXT NOT NULL, ts INTEGER NOT NULL, status TEXT NOT NULL, msg_id INTEGER NOT NULL,
            hops INTEGER NOT NULL, hist_id INTEGER UNIQUE)""")
        db.execSQL("CREATE INDEX msg_conv ON msg(conv, id)")
        db.execSQL("CREATE INDEX msg_msgid ON msg(msg_id)")
        db.execSQL("""CREATE TABLE node (addr TEXT PRIMARY KEY, name TEXT, hops INTEGER, last_heard INTEGER NOT NULL,
            mv INTEGER, usb INTEGER, lat REAL, lon REAL)""")
        db.execSQL("CREATE TABLE channel (addr TEXT PRIMARY KEY, name TEXT NOT NULL)")
    }

    override fun onUpgrade(db: SQLiteDatabase, old: Int, new: Int) {}

    fun insert(m: Msg, histId: Long? = null): Long {
        val v = ContentValues().apply {
            put("conv", m.conv); put("mine", if (m.mine) 1 else 0); put("sender", m.from); put("text", m.text)
            put("ts", m.ts); put("status", m.status); put("msg_id", m.msgId); put("hops", m.hops)
            if (histId != null) put("hist_id", histId)
        }
        return writableDatabase.insertWithOnConflict("msg", null, v, SQLiteDatabase.CONFLICT_IGNORE)
    }

    /** A message the app already has from a live event: give it the host's history id instead of a second row. */
    fun adopt(histId: Long, conv: String, mine: Boolean, from: String, text: String, ts: Long): Boolean {
        val id = readableDatabase.rawQuery(
            "SELECT id FROM msg WHERE hist_id IS NULL AND conv = ? AND mine = ? AND sender = ? AND text = ? AND ABS(ts - ?) < 600000 LIMIT 1",
            arrayOf(conv, if (mine) "1" else "0", from, text, "$ts"),
        ).use { if (it.moveToFirst()) it.getLong(0) else null } ?: return false
        writableDatabase.update("msg", ContentValues().apply { put("hist_id", histId) }, "id = ?", arrayOf("$id"))
        return true
    }

    fun setStatus(id: Long, status: String, msgId: Long? = null) {
        val v = ContentValues().apply { put("status", status); if (msgId != null) put("msg_id", msgId) }
        writableDatabase.update("msg", v, "id = ?", arrayOf("$id"))
    }

    fun byMsgId(msgId: Long): Long? =
        readableDatabase.rawQuery("SELECT id FROM msg WHERE msg_id = ? AND mine = 1 ORDER BY id DESC LIMIT 1", arrayOf("$msgId")).use {
            if (it.moveToFirst()) it.getLong(0) else null
        }

    fun messages(conv: String, limit: Int = 300): List<Msg> =
        readableDatabase.rawQuery("SELECT * FROM (SELECT * FROM msg WHERE conv = ? ORDER BY id DESC LIMIT $limit) ORDER BY ts, id", arrayOf(conv)).use { c ->
            buildList { while (c.moveToNext()) add(row(c)) }
        }

    /** The newest message of every conversation. */
    fun lastPerConv(): List<Msg> =
        readableDatabase.rawQuery("SELECT * FROM msg WHERE id IN (SELECT MAX(id) FROM msg GROUP BY conv) ORDER BY ts DESC", null).use { c ->
            buildList { while (c.moveToNext()) add(row(c)) }
        }

    fun queued(): List<Msg> = readableDatabase.rawQuery("SELECT * FROM msg WHERE mine = 1 AND status = 'queued' ORDER BY id", null).use { c ->
        buildList { while (c.moveToNext()) add(row(c)) }
    }

    fun deleteConv(conv: String) { writableDatabase.delete("msg", "conv = ?", arrayOf(conv)) }

    private fun row(c: android.database.Cursor) = Msg(
        c.getLong(c.getColumnIndexOrThrow("id")), c.getString(c.getColumnIndexOrThrow("conv")),
        c.getInt(c.getColumnIndexOrThrow("mine")) != 0, c.getString(c.getColumnIndexOrThrow("sender")),
        c.getString(c.getColumnIndexOrThrow("text")), c.getLong(c.getColumnIndexOrThrow("ts")),
        c.getString(c.getColumnIndexOrThrow("status")), c.getLong(c.getColumnIndexOrThrow("msg_id")),
        c.getInt(c.getColumnIndexOrThrow("hops")),
    )

    fun saveNode(n: NodeEntry) {
        val v = ContentValues().apply {
            put("addr", n.addr); put("name", n.name); put("hops", n.hops); put("last_heard", n.lastHeard)
            put("mv", n.mv); put("usb", n.usb?.let { if (it) 1 else 0 }); put("lat", n.lat); put("lon", n.lon)
        }
        writableDatabase.insertWithOnConflict("node", null, v, SQLiteDatabase.CONFLICT_REPLACE)
    }

    fun nodes(): List<NodeEntry> = readableDatabase.rawQuery("SELECT * FROM node", null).use { c ->
        fun int(n: String) = c.getColumnIndexOrThrow(n).let { if (c.isNull(it)) null else c.getInt(it) }
        fun dbl(n: String) = c.getColumnIndexOrThrow(n).let { if (c.isNull(it)) null else c.getDouble(it) }
        buildList {
            while (c.moveToNext()) add(NodeEntry(
                c.getString(0), c.getColumnIndexOrThrow("name").let { if (c.isNull(it)) null else c.getString(it) },
                int("hops"), c.getLong(c.getColumnIndexOrThrow("last_heard")), int("mv"), int("usb")?.let { it != 0 }, dbl("lat"), dbl("lon"),
            ))
        }
    }

    fun saveChannel(addr: String, name: String) {
        writableDatabase.insertWithOnConflict("channel", null, ContentValues().apply { put("addr", addr); put("name", name) }, SQLiteDatabase.CONFLICT_REPLACE)
    }

    fun channels(): Map<String, String> = readableDatabase.rawQuery("SELECT addr, name FROM channel", null).use { c ->
        buildMap { while (c.moveToNext()) put(c.getString(0), c.getString(1)) }
    }
}

/** The app's own settings (not the node's or the host's). */
class Prefs(ctx: Context) {
    private val p = ctx.getSharedPreferences("maarse", Context.MODE_PRIVATE)
    var mode: Mode
        get() = runCatching { Mode.valueOf(p.getString("mode", "BLE")!!) }.getOrDefault(Mode.BLE)
        set(v) = p.edit().putString("mode", v.name).apply()
    var bleAddress: String? get() = p.getString("ble", null); set(v) = p.edit().putString("ble", v).apply()
    var bleName: String? get() = p.getString("bleName", null); set(v) = p.edit().putString("bleName", v).apply()
    var remoteAddr: String get() = p.getString("remote", "") ?: ""; set(v) = p.edit().putString("remote", v).apply()
    var remoteKey: String get() = p.getString("remoteKey", "") ?: ""; set(v) = p.edit().putString("remoteKey", v).apply()
    /** Should the app connect at start (the user pressed Connect and not Disconnect)? */
    var running: Boolean get() = p.getBoolean("running", false); set(v) = p.edit().putBoolean("running", v).apply()
    var notifyMessages: Boolean get() = p.getBoolean("notifyMsg", true); set(v) = p.edit().putBoolean("notifyMsg", v).apply()
    /** The first-start guide has been through. */
    var onboarded: Boolean get() = p.getBoolean("onboarded", false); set(v) = p.edit().putBoolean("onboarded", v).apply()
    var notifyChannels: Boolean get() = p.getBoolean("notifyCh", false); set(v) = p.edit().putBoolean("notifyCh", v).apply()

    var nicknames: Map<String, String>
        get() = runCatching { JSONObject(p.getString("nicknames", "{}")!!).let { o -> o.keys().asSequence().associateWith { o.getString(it) } } }.getOrDefault(emptyMap())
        set(v) = p.edit().putString("nicknames", JSONObject(v).toString()).apply()

    var dismissedAlerts: Set<String>
        get() = p.getStringSet("dismissed", emptySet()) ?: emptySet()
        set(v) = p.edit().putStringSet("dismissed", v).apply()
}

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
    /** An attachment: path in the app's files, and its MIME type. */
    val file: String? = null,
    val mime: String? = null,
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
    /** Role byte of the announce (0 client, 1 repeater, 2 relay, 3 anchor), null until an announce with the field was heard. */
    val role: Int? = null,
    /** The announce claims this node is a device of a person (app-data field 0x08). */
    val person: Boolean = false,
)

class Db(ctx: Context) : SQLiteOpenHelper(ctx, "maarse.db", null, 2) {
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

    override fun onUpgrade(db: SQLiteDatabase, old: Int, new: Int) {
        if (old < 2) {
            db.execSQL("ALTER TABLE msg ADD COLUMN file TEXT")
            db.execSQL("ALTER TABLE msg ADD COLUMN mime TEXT")
        }
    }

    override fun onOpen(db: SQLiteDatabase) {
        super.onOpen(db)
        // A fresh install creates version 2 through onCreate: add the columns there too.
        runCatching { db.execSQL("ALTER TABLE msg ADD COLUMN file TEXT") }
        runCatching { db.execSQL("ALTER TABLE msg ADD COLUMN mime TEXT") }
        // Role and person claim of a node, from its announce.
        runCatching { db.execSQL("ALTER TABLE node ADD COLUMN role INTEGER") }
        runCatching { db.execSQL("ALTER TABLE node ADD COLUMN person INTEGER") }
        // Pages belong on the Pages screen only; remove any that once
        // ended up in a chat as a file.
        runCatching { db.delete("msg", "mime = ?", arrayOf(Pages.PAGE_MIME)) }
    }

    fun setFile(id: Long, path: String?, mime: String?, text: String? = null) {
        val v = ContentValues().apply { put("file", path); put("mime", mime); if (text != null) put("text", text) }
        writableDatabase.update("msg", v, "id = ?", arrayOf("$id"))
    }

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
        // Already imported (the host sends its history again on every connect).
        if (readableDatabase.rawQuery("SELECT 1 FROM msg WHERE hist_id = ?", arrayOf("$histId")).use { it.moveToFirst() }) return true
        val id = readableDatabase.rawQuery(
            "SELECT id FROM msg WHERE hist_id IS NULL AND conv = ? AND mine = ? AND sender = ? AND text = ? AND ABS(ts - ?) < 600000 LIMIT 1",
            arrayOf(conv, if (mine) "1" else "0", from, text, "$ts"),
        ).use { if (it.moveToFirst()) it.getLong(0) else null } ?: return false
        writableDatabase.updateWithOnConflict("msg", ContentValues().apply { put("hist_id", histId) }, "id = ?", arrayOf("$id"), SQLiteDatabase.CONFLICT_IGNORE)
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

    fun delete(id: Long) { writableDatabase.delete("msg", "id = ?", arrayOf("$id")) }

    /** A message deleted on another screen: same conversation, direction and text, within two minutes. */
    fun deleteLike(conv: String, mine: Boolean, text: String, ts: Long) {
        writableDatabase.delete("msg", "conv = ? AND mine = ? AND text = ? AND ABS(ts - ?) <= 120000",
            arrayOf(conv, if (mine) "1" else "0", text, "$ts"))
    }

    fun deleteConv(conv: String) { writableDatabase.delete("msg", "conv = ?", arrayOf(conv)) }

    private fun row(c: android.database.Cursor) = Msg(
        c.getLong(c.getColumnIndexOrThrow("id")), c.getString(c.getColumnIndexOrThrow("conv")),
        c.getInt(c.getColumnIndexOrThrow("mine")) != 0, c.getString(c.getColumnIndexOrThrow("sender")),
        c.getString(c.getColumnIndexOrThrow("text")), c.getLong(c.getColumnIndexOrThrow("ts")),
        c.getString(c.getColumnIndexOrThrow("status")), c.getLong(c.getColumnIndexOrThrow("msg_id")),
        c.getInt(c.getColumnIndexOrThrow("hops")),
        c.getColumnIndex("file").takeIf { it >= 0 && !c.isNull(it) }?.let { c.getString(it) },
        c.getColumnIndex("mime").takeIf { it >= 0 && !c.isNull(it) }?.let { c.getString(it) },
    )

    fun saveNode(n: NodeEntry) {
        val v = ContentValues().apply {
            put("addr", n.addr); put("name", n.name); put("hops", n.hops); put("last_heard", n.lastHeard)
            put("mv", n.mv); put("usb", n.usb?.let { if (it) 1 else 0 }); put("lat", n.lat); put("lon", n.lon)
            put("role", n.role); put("person", if (n.person) 1 else 0)
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
                int("role"), (int("person") ?: 0) != 0,
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
    /** The open backbone is the default, so a linked device finds the account's MeshCore without typing the address. */
    var meshcoreBackbone: String get() = p.getString("meshcoreBackbone", "")?.ifEmpty { null } ?: "https://nj01.dk.eu.maarse.dk"; set(v) = p.edit().putString("meshcoreBackbone", v).apply()
    var remoteKey: String get() = p.getString("remoteKey", "") ?: ""; set(v) = p.edit().putString("remoteKey", v).apply()
    /** Should the app connect at start (the user pressed Connect and not Disconnect)? */
    var running: Boolean get() = p.getBoolean("running", false); set(v) = p.edit().putBoolean("running", v).apply()
    var notifyMessages: Boolean get() = p.getBoolean("notifyMsg", true); set(v) = p.edit().putBoolean("notifyMsg", v).apply()
    /** Favourite users and favourite pages (addresses). */
    var favUsers: List<String>
        get() = (p.getString("favUsers", "") ?: "").split(',').filter { it.length == 32 }
        set(v) = p.edit().putString("favUsers", v.joinToString(",")).apply()
    var favPages: List<String>
        get() = (p.getString("favPages", "") ?: "").split(',').filter { it.length == 32 }
        set(v) = p.edit().putString("favPages", v.joinToString(",")).apply()
    /** Nodes known to share maarse:// pages (heard in announces or visited). */
    var pageNodes: List<String>
        get() = (p.getString("pageNodes", "") ?: "").split(',').filter { it.length == 32 }
        set(v) = p.edit().putString("pageNodes", v.takeLast(50).joinToString(",")).apply()
    /** The first-start guide has been through. */
    var onboarded: Boolean get() = p.getBoolean("onboarded", false); set(v) = p.edit().putBoolean("onboarded", v).apply()
    /** The sound chosen for a kind of notification ("dm", "channel", "mention"); see [Sounds]. */
    fun sound(kind: String): String = p.getString("sound-$kind", "") ?: ""
    fun setSound(kind: String, spec: String) = p.edit().putString("sound-$kind", spec).apply()
    /** MeshCore names seen per public key, to name nodes that a scan finds before they advertise. */
    var mcNames: Map<String, String>
        get() = runCatching { JSONObject(p.getString("mcNames", "{}")!!).let { o -> o.keys().asSequence().associateWith { o.getString(it) } } }.getOrDefault(emptyMap())
        set(v) = p.edit().putString("mcNames", JSONObject(v).toString()).apply()
    var notifyChannels: Boolean get() = p.getBoolean("notifyCh", false); set(v) = p.edit().putBoolean("notifyCh", v).apply()

    var nicknames: Map<String, String>
        get() = runCatching { JSONObject(p.getString("nicknames", "{}")!!).let { o -> o.keys().asSequence().associateWith { o.getString(it) } } }.getOrDefault(emptyMap())
        set(v) = p.edit().putString("nicknames", JSONObject(v).toString()).apply()

    /** Public keys heard per address (hex), for safety numbers. */
    var keys: Map<String, String>
        get() = runCatching { JSONObject(p.getString("keys", "{}")!!).let { o -> o.keys().asSequence().associateWith { o.getString(it) } } }.getOrDefault(emptyMap())
        set(v) = p.edit().putString("keys", JSONObject(v).toString()).apply()

    /** Contacts marked as verified: address -> the key that was verified. */
    var verified: Map<String, String>
        get() = runCatching { JSONObject(p.getString("verified", "{}")!!).let { o -> o.keys().asSequence().associateWith { o.getString(it) } } }.getOrDefault(emptyMap())
        set(v) = p.edit().putString("verified", JSONObject(v).toString()).apply()

    var dismissedAlerts: Set<String>
        get() = p.getStringSet("dismissed", emptySet()) ?: emptySet()
        set(v) = p.edit().putStringSet("dismissed", v).apply()
}

package dk.maarse.app

import android.os.Handler
import android.os.Looper
import kotlinx.coroutines.flow.MutableStateFlow
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.json.JSONArray
import org.json.JSONObject

/**
 * Sensor nodes (Nodes > Sensors), the app's side of `docs/sensors.md`. The
 * host (the phone's own node, or your home computer) keeps the readings and
 * does the work for the designer: it turns the setup JSON into the recipe the
 * sensor runs, checks it and runs it on a simulated day. The web client does
 * the same through the same HTTP API (maarse-host/src/sensors.rs).
 */
data class SensorChan(val chan: Int, val kind: Int, val name: String, val unit: String, val value: Double?)
data class SensorInfo(
    val node: String, val name: String, val favourite: Boolean, val ageS: Long,
    val alarm: String?, val alarmT: Long, val channels: List<SensorChan>,
) { val title get() = name.ifBlank { "Sensor " + node.take(4) } }

class SensorDesign(var node: String?, var setup: JSONObject) {
    var compiled: JSONObject? = null
    var sim: JSONObject? = null
    var scenario = "cold_night"
    var note = ""
    var sent = ""
}

class SensorTemplate(val key: String, val title: String, val text: String, val json: String)

object Sensors {
    private val main = Handler(Looper.getMainLooper())
    private val http by lazy { OkHttpClient() }
    private val json = "application/json".toMediaType()

    val list = MutableStateFlow<List<SensorInfo>?>(null)
    val error = MutableStateFlow("")
    /** Bumped whenever the design changes, so screens redraw. */
    val rev = MutableStateFlow(0)
    var design: SensorDesign? = null
    val status = MutableStateFlow<String?>(null)
    private var debounce: Runnable? = null
    private var busy = 0

    /** The boards and sensors the host offers (GET /api/sensors/catalog): the same list the web client draws its pickers from. */
    val catalog = MutableStateFlow<JSONObject?>(null)

    fun loadCatalog(force: Boolean = false) {
        if (catalog.value != null && !force) return
        call("GET", "/api/sensors/catalog") { r ->
            r.onSuccess { t ->
                runCatching { JSONObject(t) }.getOrNull()?.let {
                    catalog.value = it; rev.value = rev.value + 1
                    // The host looks on GitHub by itself where it can (a PC); a phone has no curl, so the app helps once a day.
                    val u = it.optJSONObject("update")
                    if (u != null && u.optBoolean("auto", true) && u.optBoolean("can_update") && System.currentTimeMillis() / 1000 - u.optLong("checked_at") > 24 * 3600) updateCatalog { }
                }
            }
        }
    }

    /** Fetches the signed sensor list from GitHub and gives it to the host, which checks the signature and the content. */
    fun updateCatalog(done: (String) -> Unit) {
        Thread {
            val base = "https://github.com/trixxor/maarse/releases/download/sensors/"
            val files = runCatching {
                fun get(n: String): String = http.newCall(Request.Builder().url(base + n).header("User-Agent", "maarse-app").build()).execute().use { r ->
                    if (!r.isSuccessful) throw Exception("$n: HTTP ${r.code}")
                    r.body?.string() ?: ""
                }
                Triple(get("catalog.json"), get("SHA256SUMS-sensors"), get("SHA256SUMS-sensors.sig"))
            }
            main.post {
                files.onFailure { done("Not done: " + (it.message ?: "no connection")) }
                files.onSuccess { (c, sums, sig) ->
                    val body = JSONObject().put("catalog", c).put("sums", sums).put("sig", sig).toString()
                    call("POST", "/api/sensors/catalog/update", body) { r ->
                        val j = r.getOrNull()?.let { runCatching { JSONObject(it) }.getOrNull() }
                        done(j?.optString("error")?.takeIf { it.isNotBlank() } ?: j?.optString("message") ?: (r.exceptionOrNull()?.message ?: "Done"))
                        loadCatalog(force = true)
                    }
                }
            }
        }.start()
    }

    fun setCatalogAuto(on: Boolean) {
        call("POST", "/api/sensors/catalog/update", JSONObject().put("auto", on).toString()) { loadCatalog(force = true) }
    }

    // Used until the host has answered (or when it is an older host without a catalog).
    private val FALLBACK = listOf(
        "xiao_nrf52840" to "Seeed XIAO nRF52840 + Wio-SX1262",
        "heltec_v4" to "Heltec WiFi LoRa 32 V4",
    )

    private fun boardJson(id: String): JSONObject? {
        val a = catalog.value?.optJSONArray("boards") ?: return null
        return (0 until a.length()).map { a.getJSONObject(it) }.firstOrNull { it.getString("id") == id }
    }

    /** Board id to name; a board whose firmware cannot read sensors yet says so. */
    fun boards(): List<Pair<String, String>> {
        val a = catalog.value?.optJSONArray("boards") ?: return FALLBACK
        return (0 until a.length()).map { a.getJSONObject(it) }.map { it.getString("id") to it.getString("name") + if (it.optBoolean("available", true)) "" else " (sensor firmware coming)" }
    }
    fun boardName(id: String) = boards().firstOrNull { it.first == id }?.second ?: id
    fun boardAvailable(id: String) = boardJson(id)?.optBoolean("available", true) ?: true
    fun boardNote(id: String) = boardJson(id)?.optString("note") ?: ""
    fun boardGrove(id: String) = boardJson(id)?.optString("grove") ?: ""
    fun i2cText(id: String) = boardJson(id)?.optJSONObject("i2c")?.optString("text") ?: if (id == "heltec_v4") "GPIO47 = SDA, GPIO48 = SCL" else "D6 = SDA, D7 = SCL"

    /** Pins that are free, by board: pin id to its name on the board. */
    fun pins(board: String): List<Pair<Int, String>> {
        val b = boardJson(board) ?: return if (board == "heltec_v4") listOf(3, 4, 6, 38, 39, 40, 41).map { it to "GPIO$it" } else listOf(0 to "D0", 6 to "D6", 7 to "D7")
        val a = b.getJSONArray("pins")
        return (0 until a.length()).map { a.getJSONObject(it) }.map { it.getInt("id") to it.getString("label") }
    }
    fun i2cPins(board: String): List<Int>? {
        val b = boardJson(board) ?: return if (board == "heltec_v4") listOf(47, 48) else listOf(6, 7)
        return b.optJSONObject("i2c")?.let { listOf(it.getInt("sda"), it.getInt("scl")) }
    }
    /** Pins with an analogue input. */
    fun adcPins(board: String): List<Pair<Int, String>> {
        val b = boardJson(board) ?: return if (board == "heltec_v4") listOf(4 to "GPIO4") else emptyList()
        val a = b.getJSONArray("analog")
        return (0 until a.length()).map { a.getJSONObject(it) }.map { it.getInt("id") to it.getString("label") }
    }
    fun catalogSensors(): List<JSONObject> {
        val a = catalog.value?.optJSONArray("sensors") ?: return emptyList()
        return (0 until a.length()).map { a.getJSONObject(it) }
    }
    fun sensorById(id: String) = catalogSensors().firstOrNull { it.optString("id") == id }
    fun sensorByDevice(device: String) = catalogSensors().firstOrNull { it.optString("device") == device }

    /** The starting points come with the catalogue: a new one needs no new app. */
    fun templates(): List<SensorTemplate> {
        val a = catalog.value?.optJSONArray("templates") ?: return emptyList()
        return (0 until a.length()).map { a.getJSONObject(it) }.map { SensorTemplate(it.getString("id"), it.getString("title"), it.getString("text"), it.getJSONObject("setup").toString()) }
    }

    private val BASIC_UNITS = mapOf("temperature" to "°C", "humidity" to "%", "pressure" to "hPa", "voltage" to "V", "percent" to "%", "distance" to "mm", "light" to "lx", "co2" to "ppm", "accel" to "mg")

    private fun kindObjects(): List<JSONObject> {
        val a = catalog.value?.optJSONArray("kinds") ?: return emptyList()
        return (0 until a.length()).mapNotNull { a.optJSONObject(it) }
    }

    /** The unit of a kind of measurement ("°C"), from the catalogue. */
    fun unitOf(kind: String): String = kindObjects().firstOrNull { it.optString("key") == kind }?.optString("unit") ?: BASIC_UNITS[kind] ?: ""

    /** The kinds a person can pick for a sensor of their own. */
    fun kindKeys(): List<String> = kindObjects().map { it.optString("key") }.filter { it != "digital" && it != "counter" }.ifEmpty { listOf("custom", "temperature", "humidity", "pressure", "voltage", "percent", "distance", "light", "co2", "accel") }

    fun newSetup(tpl: String): JSONObject {
        val t = templates().firstOrNull { it.key == tpl }
            ?: SensorTemplate("blank", "", "", """{"name":"","sources":[{"chan":1,"kind":"voltage","name":"Battery","input":{"type":"battery"}}],"rules":[]}""")
        val s = JSONObject("""{"v":1,"board":"xiao_nrf52840","interval_s":900,"sleep":true,
            "report_to":{"owners":[0],"channel":false},"alarm_to":{"owners":[0],"channel":false},"channel":""}""")
        val body = JSONObject(t.json)
        for (k in body.keys()) s.put(k, body.get(k))
        return s
    }

    // ----- the host's HTTP API -----

    private fun call(method: String, path: String, body: String? = null, done: (Result<String>) -> Unit) {
        val base = Node.hostBase()
        if (base == null) {
            main.post { done(Result.failure(Exception("Sensors need the phone as node, or your home computer (not a radio alone)."))) }
            return
        }
        Thread {
            val r = runCatching {
                val rb = Request.Builder().url(base.first + path)
                if (base.second.isNotEmpty()) rb.header("Authorization", "Bearer " + base.second)
                rb.method(method, body?.toRequestBody(json) ?: if (method == "GET") null else "".toRequestBody(json))
                http.newCall(rb.build()).execute().use { resp ->
                    val t = resp.body?.string() ?: ""
                    if (!resp.isSuccessful) throw Exception(runCatching { JSONObject(t).optString("error", resp.message) }.getOrDefault(resp.message))
                    t
                }
            }
            main.post { done(r) }
        }.start()
    }

    fun refresh() {
        call("GET", "/api/sensors") { r ->
            r.onSuccess { t ->
                val a = JSONArray(t)
                list.value = (0 until a.length()).map { i ->
                    val o = a.getJSONObject(i)
                    val ch = o.getJSONArray("channels")
                    val alarm = o.optJSONObject("alarm")
                    SensorInfo(o.getString("node"), o.optString("name"), o.optBoolean("favourite"), o.optLong("age_s"),
                        alarm?.optString("text"), alarm?.optLong("t") ?: 0,
                        (0 until ch.length()).map { j ->
                            val c = ch.getJSONObject(j)
                            SensorChan(c.getInt("chan"), c.getInt("kind"), c.optString("name"), c.optString("unit"), if (c.isNull("value")) null else c.getDouble("value"))
                        })
                }.sortedWith(compareBy<SensorInfo>({ !it.favourite }, { it.title.lowercase() }))
                error.value = ""
            }.onFailure { error.value = it.message ?: "Error" }
        }
    }

    fun series(node: String, chan: Int, seconds: Long, done: (List<Pair<Double, Double?>>) -> Unit) {
        val now = System.currentTimeMillis() / 1000
        call("GET", "/api/sensors/series?node=$node&chan=$chan&from=${now - seconds}&max=200") { r ->
            r.onSuccess { t ->
                val p = JSONObject(t).getJSONArray("points")
                done((0 until p.length()).map { val x = p.getJSONArray(it); x.getDouble(0) to (if (x.isNull(1)) null else x.getDouble(1)) })
            }
        }
    }

    fun alarms(node: String, done: (List<Triple<Long, Boolean, String>>) -> Unit) {
        call("GET", "/api/sensors/alarms?node=$node") { r ->
            r.onSuccess { t ->
                val a = JSONArray(t)
                done((0 until a.length()).map { val o = a.getJSONObject(it); Triple(o.getLong("t"), o.getBoolean("raised"), o.optString("text")) })
            }
        }
    }

    fun meta(node: String, name: String? = null, favourite: Boolean? = null) {
        val o = JSONObject().put("node", node)
        name?.let { o.put("name", it) }
        favourite?.let { o.put("favourite", it) }
        call("POST", "/api/sensors/meta", o.toString()) { refresh() }
    }

    /** The alarm is read (a mailbox alarm never clears by itself). */
    fun dismiss(node: String) = call("POST", "/api/sensors/dismiss", JSONObject().put("node", node).toString()) { refresh() }

    fun forget(node: String) = call("POST", "/api/sensors/forget", JSONObject().put("node", node).toString()) { refresh() }

    // ----- the designer -----

    fun openDesign(node: String?, tpl: String) {
        loadCatalog()
        val d = SensorDesign(node, newSetup(if (node == null) tpl else "blank"))
        design = d
        status.value = null
        if (node != null) {
            call("GET", "/api/sensors/setup?node=$node") { r ->
                r.onSuccess { t ->
                    val s = runCatching { JSONObject(t) }.getOrNull()
                    if (s != null && s.has("sources")) d.setup = s
                    else d.note = "This sensor has no saved setup on this host yet. Build the rules here."
                    changed()
                }
            }
        }
        changed()
    }

    /** The design changed: redraw, and ask the host to check it (after a pause). */
    fun changed() {
        rev.value = rev.value + 1
        val d = design ?: return
        debounce?.let { main.removeCallbacks(it) }
        val run = Runnable {
            val my = ++busy
            call("POST", "/api/sensors/compile", d.setup.toString()) { r ->
                if (my != busy || design !== d) return@call
                val c = r.getOrNull()?.let { JSONObject(it) } ?: JSONObject().put("ok", false).put("error", r.exceptionOrNull()?.message ?: "Error")
                d.compiled = c
                if (c.optBoolean("ok")) {
                    val req = JSONObject().put("setup", d.setup).put("scenario", d.scenario).toString()
                    call("POST", "/api/sensors/simulate", req) { s -> if (my == busy && design === d) { d.sim = s.getOrNull()?.let { JSONObject(it) }; rev.value = rev.value + 1 } }
                } else { d.sim = null; rev.value = rev.value + 1 }
            }
        }
        debounce = run
        main.postDelayed(run, 250)
    }

    /** Sends the compiled setup to a sensor we own, over the radio (it applies it after its next wake-up). */
    fun sendRadio(): String {
        val d = design ?: return "Nothing to send"
        val node = d.node ?: return "A new sensor needs its first setup over USB from a computer (the sensor takes you as its owner). After that you can change it from here."
        val hex = d.compiled?.takeIf { it.optBoolean("ok") }?.optString("hex") ?: return "Fix the setup first"
        val bytes = ByteArray(hex.length / 2) { hex.substring(it * 2, it * 2 + 2).toInt(16).toByte() }
        Thread {
            var off = 0
            while (off < bytes.size) {
                val piece = bytes.copyOfRange(off, minOf(bytes.size, off + 100))
                val head = byteArrayOf((off and 255).toByte(), (off shr 8).toByte(), (bytes.size and 255).toByte(), (bytes.size shr 8).toByte())
                Node.send(Cmd.sensor(Node.req(), node, 2, head + piece))
                off += 100
                Thread.sleep(400)
            }
            call("PUT", "/api/sensors/setup?node=$node", d.setup.toString()) {}
            main.post { status.value = "Sent. The pieces wait at a post office; the sensor applies the setup after its next wake-up." }
        }.start()
        return "Sending ..."
    }

    fun saveSetup() {
        val d = design ?: return
        val node = d.node ?: return
        call("PUT", "/api/sensors/setup?node=$node", d.setup.toString()) {}
        val name = d.setup.optString("name")
        if (name.isNotBlank()) meta(node, name = name)
    }

    fun statusWords(code: Int) = when (code) {
        0 -> ""; 1 -> "You are not an owner of this sensor."; 2 -> "The setup was damaged on the way."
        3 -> "This firmware does not know that setup version."; 4 -> "The setup is too large."
        5 -> "The sensor refused the setup (a pin or value it cannot use)."; 6 -> "The pieces came in the wrong order; try again."
        7 -> "The sensor could not store it."; 8 -> "No sensors in the setup."; else -> "Error $code"
    }

    /** Events from sensors: live readings (refresh the list) and answers (status of a sent setup). */
    fun onEvent(ev: Ev.Sensor) {
        when (ev.op) {
            3 -> if (list.value != null) refresh()
            2 -> if (ev.data.size >= 13) {
                val err = ev.data[0].toInt() and 255
                status.value = if (err == 0) "The sensor has the new setup." else "The sensor refused it: " + statusWords(err)
            }
        }
    }
}

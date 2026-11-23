package dk.maarse.app

import android.content.Context
import android.net.Uri
import android.os.Handler
import android.os.Looper
import kotlinx.coroutines.flow.MutableStateFlow
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import org.json.JSONObject

/**
 * Firmware updates of the radios attached to the host (the phone's own node, or your home
 * computer): the app's side of `maarse-host/src/fwupdate.rs`. The web client does the same
 * through the same HTTP API.
 *
 * The host sends the firmware to a radio on USB, Bluetooth or Wi-Fi; the radio checks the
 * signature and the version itself. A phone host has no `curl`, so the app downloads the
 * signed files from the latest GitHub release and hands them to the host
 * (`/api/firmware/upload?source=github`), which then does the same as on a PC.
 */
object Firmware {
    private val main = Handler(Looper.getMainLooper())
    private val http by lazy { OkHttpClient() }
    private const val LATEST = "https://github.com/trixxor/maarse/releases/latest/download/"
    private val BOARDS = listOf("xiao-nrf52840", "wismesh-tag", "heltec-v4", "xiao-esp32s3")
    private val octet = "application/octet-stream".toMediaType()
    private val json = "application/json".toMediaType()

    /** The host's answer to `GET /api/firmware` (null until it has answered). */
    val data = MutableStateFlow<JSONObject?>(null)
    /** What happened last, in plain words. */
    val message = MutableStateFlow("")
    val busy = MutableStateFlow(false)

    /** Is there a host to ask? A radio alone over Bluetooth has none. */
    fun available() = Node.hostBase() != null

    private fun call(method: String, path: String, body: ByteArray? = null, type: okhttp3.MediaType = json, done: (Result<JSONObject>) -> Unit) {
        val base = Node.hostBase()
        if (base == null) {
            main.post { done(Result.failure(Exception("Updating from the app needs the phone as node, or your home computer."))) }
            return
        }
        Thread {
            val r = runCatching {
                val rb = Request.Builder().url(base.first + path)
                if (base.second.isNotEmpty()) rb.header("Authorization", "Bearer " + base.second)
                rb.method(method, body?.toRequestBody(type) ?: if (method == "GET") null else ByteArray(0).toRequestBody(type))
                http.newCall(rb.build()).execute().use { resp ->
                    val t = resp.body?.string() ?: ""
                    val j = runCatching { JSONObject(t) }.getOrNull() ?: throw Exception(t.ifBlank { resp.message })
                    j
                }
            }
            main.post { done(r) }
        }.start()
    }

    private fun take(r: Result<JSONObject>) {
        r.onSuccess { j ->
            if (j.has("devices")) data.value = j
            j.optString("message").takeIf { it.isNotBlank() }?.let { message.value = it }
        }
        r.onFailure { message.value = it.message ?: "No answer from the host." }
    }

    fun refresh() = call("GET", "/api/firmware") { take(it) }

    private fun download(name: String): ByteArray =
        http.newCall(Request.Builder().url(LATEST + name).header("User-Agent", "maarse-app").build()).execute().use { r ->
            if (!r.isSuccessful) throw Exception("$name: HTTP ${r.code}")
            r.body?.bytes() ?: ByteArray(0)
        }

    private fun upload(name: String, bytes: ByteArray, source: String, done: (Result<JSONObject>) -> Unit) =
        call("POST", "/api/firmware/upload?source=$source&name=" + Uri.encode(name), bytes, octet, done)

    /** Looks for the newest release: downloads the small signed manifests and gives them to the host, which checks the signature. */
    fun check() {
        busy.value = true
        Thread {
            val files = BOARDS.mapNotNull { b -> runCatching { "maarse-$b-ota.manifest" to download("maarse-$b-ota.manifest") }.getOrNull() }
            main.post {
                if (files.isEmpty()) { busy.value = false; message.value = "Could not reach GitHub."; return@post }
                var left = files.size
                for ((n, b) in files) upload(n, b, "github") { r ->
                    take(r)
                    if (--left == 0) { busy.value = false; refresh() }
                }
            }
        }.start()
    }

    /** Updates one radio. From GitHub the app fetches the image for the radio's board first. */
    fun update(id: Int, board: String, source: String) {
        if (source == "local") { call("POST", "/api/firmware/update", JSONObject().put("id", id).put("source", "local").toString().toByteArray()) { take(it); refresh() }; return }
        busy.value = true
        message.value = "Downloading the firmware…"
        Thread {
            val got = runCatching { download("maarse-$board-ota.manifest") to download("maarse-$board-ota.bin") }
            main.post {
                got.onFailure { busy.value = false; message.value = tr("Could not download the firmware: {0}", it.message ?: tr("no connection")) }
                got.onSuccess { (m, img) ->
                    upload("maarse-$board-ota.manifest", m, "github") { r1 ->
                        take(r1)
                        upload("maarse-$board-ota.bin", img, "github") { r2 ->
                            take(r2)
                            call("POST", "/api/firmware/update", JSONObject().put("id", id).put("source", "github").toString().toByteArray()) { r3 -> busy.value = false; take(r3); refresh() }
                        }
                    }
                }
            }
        }.start()
    }

    /** The person's own build: a `.manifest` or a `.bin` file picked on the phone. */
    fun uploadOwn(ctx: Context, uri: Uri, name: String) {
        Thread {
            val bytes = runCatching { ctx.contentResolver.openInputStream(uri)?.use { it.readBytes() } }.getOrNull()
            main.post {
                if (bytes == null) { message.value = "Could not read that file."; return@post }
                upload(name, bytes, "local") { take(it); refresh() }
            }
        }.start()
    }

    fun stop() = call("POST", "/api/firmware/stop", "{}".toByteArray()) { take(it) }
}

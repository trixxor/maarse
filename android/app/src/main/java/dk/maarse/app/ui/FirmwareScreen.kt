package dk.maarse.app.ui

import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import dk.maarse.app.Firmware
import kotlinx.coroutines.delay
import org.json.JSONObject

private class Ask(val id: Int, val name: String, val board: String, val version: String, val source: String)

/** The text of a field that may be missing or null. */
private fun JSONObject.str(key: String): String? = if (isNull(key)) null else optString(key).takeIf { it.isNotBlank() }

/**
 * Firmware for the radios attached to the phone's node or your home computer. Same as the
 * web client's Firmware view: list the radios, offer an update, show the progress. The host does
 * the work; the radio checks the signature and keeps the old firmware if the new one does not start.
 */
@Composable
fun FirmwareSection() {
    val data by Firmware.data.collectAsState()
    val message by Firmware.message.collectAsState()
    val busy by Firmware.busy.collectAsState()
    val ctx = LocalContext.current
    var ask by remember { mutableStateOf<Ask?>(null) }

    val running = data?.optJSONObject("job")?.optString("state") == "running"
    LaunchedEffect(running) {
        while (true) {
            Firmware.refresh()
            delay(if (running) 1_500 else 8_000)
        }
    }
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { list ->
        for (u in list) {
            val name = ctx.contentResolver.query(u, null, null, null, null)?.use { c ->
                if (c.moveToFirst()) c.getString(c.getColumnIndexOrThrow(OpenableColumns.DISPLAY_NAME)) else null
            } ?: "firmware.bin"
            Firmware.uploadOwn(ctx, u, name)
        }
    }

    if (!Firmware.available()) {
        Hint("Updating a radio from the app needs the phone as node, or your home computer. With a radio alone over Bluetooth, update it over USB with the web flasher.")
        return
    }
    val d = data
    GroupTitle("Your radios")
    if (d == null) {
        Hint(message.ifBlank { "Loading…" })
        return
    }
    val devices = d.optJSONArray("devices")
    if (devices == null || devices.length() == 0) Hint("No radio is connected. A radio on USB, Bluetooth or Wi-Fi shows up here.")
    for (i in 0 until (devices?.length() ?: 0)) {
        val dev = devices!!.getJSONObject(i)
        val kind = when (dev.optString("kind")) { "usb-modem" -> "USB"; "wifi-modem" -> "Wi-Fi"; "ble-modem" -> "Bluetooth"; else -> dev.optString("kind") }
        Card2(Modifier.padding(vertical = 4.dp)) {
            Text(dev.optString("board_name").ifBlank { "Radio" } + "  ·  " + kind, style = MaterialTheme.typography.bodyLarge, fontWeight = androidx.compose.ui.text.font.FontWeight.SemiBold)
            Text(dev.optString("name"), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            val gh = dev.str("github_newer")
            val own = dev.str("local_newer")
            val text = when {
                dev.optInt("state") == 255 -> "Waiting for an answer. A radio with older firmware never answers: update it once over USB with the web flasher, and from then on it updates here."
                dev.optInt("state") == 5 -> "This radio cannot be updated over the air."
                dev.optInt("board") == 0 -> "Older firmware that cannot update itself. Update it once over USB with the web flasher; after that it updates here."
                else -> "Running " + dev.optString("current_text") + if (gh == null && own == null) " · up to date" else ""
            }
            Text(text, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            if (dev.optBoolean("can_update")) Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                val board = dev.optString("board_name")
                if (gh != null) Button(modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp), enabled = !running && !busy, onClick = { ask = Ask(dev.optInt("id"), dev.optString("name"), board, gh, "github") }) { Text("Update to $gh") }
                if (own != null && own != gh) OutlinedButton(modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp), enabled = !running && !busy, onClick = { ask = Ask(dev.optInt("id"), dev.optString("name"), board, own, "local") }) { Text("Update to $own (my file)") }
            }
        }
    }
    val job: JSONObject? = d.optJSONObject("job")
    if (job != null && (job.optString("state") == "running" || System.currentTimeMillis() / 1000 - job.optLong("started") < 1800)) {
        val total = job.optInt("total")
        val sent = job.optInt("sent")
        val failed = job.optString("state") == "error"
        Card2(Modifier.padding(vertical = 4.dp)) {
            Text(job.optString("name") + " → " + job.optString("version"), style = MaterialTheme.typography.bodyLarge)
            val p = if (job.optString("state") == "done") 1f else if (total > 0) sent.toFloat() / total else 0f
            if (!failed) LinearProgressIndicator(progress = { p }, modifier = Modifier.fillMaxWidth())
            Text(if (failed) "Failed: " + job.optString("text") else job.optString("text") + " (" + (p * 100).toInt() + " %)", style = MaterialTheme.typography.bodySmall)
            if (running) TextButton(modifier = Modifier.heightIn(min = 48.dp), onClick = { Firmware.stop() }) { Text("Stop") }
        }
    }
    if (message.isNotBlank()) Hint(message)

    GroupTitle("Where the firmware comes from")
    val gh = d.optJSONObject("github")
    val boards = gh?.optJSONArray("boards")
    val have = (0 until (boards?.length() ?: 0)).joinToString(", ") { boards!!.getJSONObject(it).let { b -> b.optString("name") + " " + b.optString("version_text") } }
    val checked = gh?.optLong("checked_at") ?: 0L
    Hint(
        "Latest release on GitHub: " + have.ifBlank { "nothing found yet" } + " · checked " +
            if (checked == 0L) "not yet" else ((System.currentTimeMillis() / 1000 - checked) / 60).toString() + " min ago",
    )
    Column(Modifier.padding(horizontal = 16.dp, vertical = 4.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedButton(enabled = !busy, onClick = { Firmware.check() }, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) { Text("Check for a new release") }
        OutlinedButton(onClick = { pick.launch(arrayOf("*/*")) }, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) { Text("Use my own build") }
    }
    val local = d.optJSONArray("local")
    if (local != null && local.length() > 0) {
        Hint("Your own files: " + (0 until local.length()).joinToString(", ") { local.getJSONObject(it).let { b -> b.optString("name") + " " + b.optString("version_text") + if (b.optBoolean("image")) "" else " (image missing)" } })
    }
    Hint("Pick both the .manifest and the .bin file of your own build. The radio only installs firmware signed with the Må:rse release key, for its own board, and newer than what it runs; if the new firmware does not start, it goes back to the old one.")

    ask?.let { a ->
        AlertDialog(
            onDismissRequest = { ask = null },
            title = { Text("Update " + a.name + "?") },
            text = { Text("It will be updated to " + a.version + (if (a.source == "local") " (your own build)" else " (the latest release)") + ". The radio restarts and is away for about a minute.") },
            confirmButton = { TextButton(onClick = { Firmware.update(a.id, a.board, a.source); ask = null }) { Text("Update") } },
            dismissButton = { TextButton(onClick = { ask = null }) { Text("Cancel") } },
        )
    }
}

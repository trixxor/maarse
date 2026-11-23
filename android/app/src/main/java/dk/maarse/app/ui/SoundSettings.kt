package dk.maarse.app.ui

import android.app.Activity
import android.content.Intent
import android.media.RingtoneManager
import android.net.Uri
import android.os.Build
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.RadioButton
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import dk.maarse.app.Node
import dk.maarse.app.Sounds
import dk.maarse.app.tr

/**
 * The notification sounds: one row per kind, Må:rse first and MeshCore in its own group, so you can
 * hear which network called. Each row opens a dialog to change the tone. The web client has the same.
 */
@Composable
internal fun SoundPickers() {
    GroupTitle("Må:rse sounds")
    Group {
        Sounds.MARSE_KINDS.forEachIndexed { i, (kind, label) -> SoundRow(kind, label, last = i == Sounds.MARSE_KINDS.lastIndex) }
    }
    McGroupTitle("MeshCore sounds", "own tone for the other network")
    Group { SoundRow("meshcore", "MeshCore messages", last = true) }
    Hint("Pick one of the tones that come with Må:rse, a sound from the phone, or your own audio file.")
}

@Composable
internal fun SoundRow(kind: String, label: String, last: Boolean = false) {
    val ctx = LocalContext.current
    var open by remember { mutableStateOf(false) }
    // The value is read again when the dialog closes, so the row shows the new tone.
    var spec by remember { mutableStateOf(Node.prefs.sound(kind)) }
    ValueRow(label, Sounds.label(ctx, spec), last) { open = true }
    if (open) SoundDialog(kind, label) { open = false; spec = Node.prefs.sound(kind) }
}

/** The dialog that changes the tone of one kind of notification. */
@Composable
internal fun SoundDialog(kind: String, label: String, onDismiss: () -> Unit) {
    val ctx = LocalContext.current
    var spec by remember { mutableStateOf(Node.prefs.sound(kind)) }
    fun choose(s: String) { spec = s; Node.prefs.setSound(kind, s) }
    val system = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) { r ->
        if (r.resultCode == Activity.RESULT_OK) {
            val u: Uri? = if (Build.VERSION.SDK_INT >= 33) r.data?.getParcelableExtra(RingtoneManager.EXTRA_RINGTONE_PICKED_URI, Uri::class.java)
            else @Suppress("DEPRECATION") r.data?.getParcelableExtra(RingtoneManager.EXTRA_RINGTONE_PICKED_URI)
            if (u != null) { choose("uri:$u"); Sounds.preview(ctx, "uri:$u") } else choose("none")
        }
    }
    val file = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { u ->
        if (u != null) Sounds.saveFile(ctx, u)?.let { choose(it); Sounds.preview(ctx, it) }
    }

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(label) },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState())) {
                @Composable fun option(name: String, value: String) {
                    Row(Modifier.fillMaxWidth().heightIn(min = 48.dp).clickable { choose(value); Sounds.preview(ctx, value) }, verticalAlignment = Alignment.CenterVertically) {
                        RadioButton(spec == value, { choose(value); Sounds.preview(ctx, value) })
                        Text(name)
                    }
                }
                option(tr("Default"), "")
                option(tr("Silent"), "none")
                HorizontalDivider()
                for ((id, name) in Sounds.TONES) option(tr(name), "tone:$id")
                HorizontalDivider()
                TextButton(modifier = Modifier.heightIn(min = 48.dp), onClick = {
                    system.launch(Intent(RingtoneManager.ACTION_RINGTONE_PICKER).apply {
                        putExtra(RingtoneManager.EXTRA_RINGTONE_TYPE, RingtoneManager.TYPE_NOTIFICATION)
                        putExtra(RingtoneManager.EXTRA_RINGTONE_SHOW_SILENT, true)
                        putExtra(RingtoneManager.EXTRA_RINGTONE_SHOW_DEFAULT, true)
                        if (spec.startsWith("uri:")) putExtra(RingtoneManager.EXTRA_RINGTONE_EXISTING_URI, Uri.parse(spec.removePrefix("uri:")))
                    })
                }) { Text("Sounds on this phone…") }
                TextButton(modifier = Modifier.heightIn(min = 48.dp), onClick = { file.launch(arrayOf("audio/*")) }) { Text("Choose an audio file…") }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss, modifier = Modifier.heightIn(min = 48.dp)) { Text("Done") } },
    )
}

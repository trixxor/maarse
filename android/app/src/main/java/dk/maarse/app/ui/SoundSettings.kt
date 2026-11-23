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
import androidx.compose.foundation.layout.padding
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

/** One row per kind of notification: its sound, and a dialog to change it. The web client has the same. */
@Composable
internal fun SoundPickers() {
    SectionTitle("Notification sounds")
    for ((kind, label) in Sounds.KINDS) SoundRow(kind, label)
    Hint("Pick one of the tones that come with Må:rse, a sound from the phone, or your own audio file. Alerts keep their own loud sound; change it in Android's notification settings.")
}

@Composable
private fun SoundRow(kind: String, label: String) {
    val ctx = LocalContext.current
    var spec by remember { mutableStateOf(Node.prefs.sound(kind)) }
    var open by remember { mutableStateOf(false) }
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

    Row(Modifier.fillMaxWidth().clickable { open = true }.padding(horizontal = 16.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(label, Modifier.weight(1f))
        Text(Sounds.label(ctx, spec))
    }
    if (open) AlertDialog(
        onDismissRequest = { open = false },
        title = { Text(label) },
        text = {
            Column {
                @Composable fun option(name: String, value: String) {
                    Row(Modifier.fillMaxWidth().clickable { choose(value); Sounds.preview(ctx, value) }, verticalAlignment = Alignment.CenterVertically) {
                        RadioButton(spec == value, { choose(value); Sounds.preview(ctx, value) })
                        Text(name)
                    }
                }
                option(tr("Default"), "")
                option(tr("Silent"), "none")
                HorizontalDivider()
                for ((id, name) in Sounds.TONES) option(tr(name), "tone:$id")
                HorizontalDivider()
                TextButton(onClick = {
                    system.launch(Intent(RingtoneManager.ACTION_RINGTONE_PICKER).apply {
                        putExtra(RingtoneManager.EXTRA_RINGTONE_TYPE, RingtoneManager.TYPE_NOTIFICATION)
                        putExtra(RingtoneManager.EXTRA_RINGTONE_SHOW_SILENT, true)
                        putExtra(RingtoneManager.EXTRA_RINGTONE_SHOW_DEFAULT, true)
                        if (spec.startsWith("uri:")) putExtra(RingtoneManager.EXTRA_RINGTONE_EXISTING_URI, Uri.parse(spec.removePrefix("uri:")))
                    })
                }) { Text("Sounds on this phone…") }
                TextButton(onClick = { file.launch(arrayOf("audio/*")) }) { Text("Choose an audio file…") }
            }
        },
        confirmButton = { TextButton(onClick = { open = false }) { Text("Done") } },
    )
}

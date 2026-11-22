package dk.maarse.app.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Call
import androidx.compose.material.icons.filled.CallEnd
import androidx.compose.material.icons.filled.Mic
import androidx.compose.material.icons.filled.MicOff
import androidx.compose.material.icons.automirrored.filled.VolumeUp
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dk.maarse.app.Calls
import dk.maarse.app.Node

/** Over everything while a call rings or runs. */
@Composable
fun CallOverlay() {
    val s by Calls.state.collectAsState()
    if (s.phase == Calls.Phase.IDLE) return
    val ctx = LocalContext.current
    val perm = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok -> if (ok) Calls.answer() else Calls.hangup() }
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(s.phase) { while (true) { now = System.currentTimeMillis(); kotlinx.coroutines.delay(1000) } }
    val name = Node.label(s.peer)
    Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().padding(32.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            Spacer(Modifier.height(48.dp))
            Avatar(name, s.peer, 120.dp)
            Spacer(Modifier.height(20.dp))
            Text(name, style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.Bold)
            Text(
                when (s.phase) {
                    Calls.Phase.CALLING -> "Calling…"
                    Calls.Phase.RINGING -> "is calling you"
                    else -> "%d:%02d · over the internet".format((now - s.started) / 60000, (now - s.started) / 1000 % 60)
                },
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            if (s.phase == Calls.Phase.ACTIVE) {
                val q by Calls.quality.collectAsState()
                val pct = if (q.got + q.lost > 0) 100.0 * q.lost / (q.got + q.lost) else 0.0
                Text("Received ${q.got} · lost ${q.lost} (%.1f%%) · ran dry ${q.dry}×".format(pct),
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(top = 8.dp))
            }
            Spacer(Modifier.weight(1f))
            when (s.phase) {
                Calls.Phase.RINGING -> Row(horizontalArrangement = Arrangement.spacedBy(64.dp)) {
                    RoundButton(Icons.Default.CallEnd, "Reject", Color(0xFFC0392B)) { Calls.hangup() }
                    RoundButton(Icons.Default.Call, "Answer", Green) {
                        if (ctx.checkSelfPermission(android.Manifest.permission.RECORD_AUDIO) == android.content.pm.PackageManager.PERMISSION_GRANTED) Calls.answer()
                        else perm.launch(android.Manifest.permission.RECORD_AUDIO)
                    }
                }
                else -> Row(horizontalArrangement = Arrangement.spacedBy(32.dp), verticalAlignment = Alignment.CenterVertically) {
                    if (s.phase == Calls.Phase.ACTIVE) RoundButton(if (s.muted) Icons.Default.MicOff else Icons.Default.Mic, if (s.muted) "Unmute" else "Mute",
                        if (s.muted) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.surfaceContainerHigh, small = true) { Calls.toggleMute() }
                    RoundButton(Icons.Default.CallEnd, "Hang up", Color(0xFFC0392B)) { Calls.hangup() }
                    if (s.phase == Calls.Phase.ACTIVE) RoundButton(Icons.AutoMirrored.Filled.VolumeUp, "Speaker",
                        if (s.speaker) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.surfaceContainerHigh, small = true) { Calls.toggleSpeaker() }
                }
            }
            Spacer(Modifier.height(32.dp))
        }
    }
}

@Composable
private fun RoundButton(icon: ImageVector, label: String, bg: Color, small: Boolean = false, onClick: () -> Unit) {
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        Box(Modifier.size(if (small) 56.dp else 72.dp).clip(CircleShape).background(bg).clickable(onClick = onClick), contentAlignment = Alignment.Center) {
            Icon(icon, label, tint = if (bg == MaterialTheme.colorScheme.surfaceContainerHigh) MaterialTheme.colorScheme.onSurface else Color.White)
        }
        Text(label, style = MaterialTheme.typography.labelMedium, modifier = Modifier.padding(top = 6.dp))
    }
}

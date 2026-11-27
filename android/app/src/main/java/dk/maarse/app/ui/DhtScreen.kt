package dk.maarse.app.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dk.maarse.app.DhtCheck
import dk.maarse.app.tr

/** Experimental > DHT check: asks the host whether the backbone DHT works as intended. */
@Composable
fun DhtPage() {
    val run by DhtCheck.run.collectAsState()
    val busy = run?.done == false
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
        Card2(Modifier.padding(top = 4.dp)) {
            Text(
                tr("Checks the backbone DHT from this host: do the contacts answer, can the other hosts find the records this host publishes, " +
                    "do they agree where an address is, and does a lookup succeed. Needs a host (the PC program or a server); costs a few internet requests, no radio airtime."),
                style = MaterialTheme.typography.bodyMedium,
            )
            Button(onClick = { DhtCheck.start() }, enabled = !busy, modifier = Modifier.padding(top = 8.dp)) { Text(tr(if (busy) "Checking…" else "Check the DHT")) }
        }
        val r = run
        if (r != null) {
            GroupTitle("Result")
            Card2 {
                if (r.done) {
                    val worst = DhtCheck.worst(r)
                    Text(
                        if (worst >= 3) "The DHT has a problem." else if (worst == 2) "The DHT works, with things to look at." else "The DHT works.",
                        fontWeight = FontWeight.SemiBold,
                    )
                }
                for (l in r.lines) Text(
                    "${DhtCheck.LEVELS.getOrElse(l.level) { "" }}: ${l.text}",
                    style = MaterialTheme.typography.bodyMedium,
                    color = when (l.level) { 3 -> Color(0xFFD1453B); 2 -> Color(0xFFB8860B); else -> MaterialTheme.colorScheme.onSurfaceVariant },
                )
            }
        }
    }
}

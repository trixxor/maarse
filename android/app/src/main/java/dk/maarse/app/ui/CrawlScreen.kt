package dk.maarse.app.ui

import android.content.Intent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import dk.maarse.app.Crawl
import dk.maarse.app.Node
import dk.maarse.app.tr

private fun snr(v: Int?) = if (v == null) "–" else (if (v > 0) "+" else "") + v + " dB"

/** Experimental > Network crawl: asks the repeaters you administer who they hear, one by one, and lists every link. */
@Composable
fun CrawlPage() {
    val ctx = LocalContext.current
    val run by Crawl.run.collectAsState()
    val nodes by Node.nodes.collectAsState()
    var start by remember { mutableStateOf("") }
    var max by remember { mutableStateOf("8") }
    var depth by remember { mutableStateOf("3") }
    var interval by remember { mutableStateOf("15") }
    var menu by remember { mutableStateOf(false) }
    val busy = run?.finished == false

    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
        Card2(Modifier.padding(top = 4.dp)) {
            Text(
                tr("Maps who hears whom: asks a repeater you administer for its neighbours, then the neighbours you also administer, and so on, " +
                    "one small question at a time. Shows every link with the signal as each side hears it, and flags links that are uneven or one-way."),
                style = MaterialTheme.typography.bodyMedium,
            )
        }
        GroupTitle("Start at")
        Card2 {
            val known = nodes.values.sortedBy { Node.label(it.addr).lowercase() }
            OutlinedButton(onClick = { menu = true }, enabled = !busy) { Text(if (start.isEmpty()) tr("Choose a node") else Node.label(start)) }
            DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                for (n in known) DropdownMenuItem(text = { Text(Node.label(n.addr)) }, onClick = { start = n.addr; menu = false })
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 8.dp)) {
                OutlinedTextField(
                    value = max, onValueChange = { max = it.filter(Char::isDigit).take(2) }, enabled = !busy, singleLine = true, label = { Text(tr("Most nodes")) },
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number), modifier = Modifier.weight(1f),
                )
                OutlinedTextField(
                    value = depth, onValueChange = { depth = it.filter(Char::isDigit).take(1) }, enabled = !busy, singleLine = true, label = { Text(tr("Hops")) },
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number), modifier = Modifier.weight(1f),
                )
                OutlinedTextField(
                    value = interval, onValueChange = { interval = it.filter(Char::isDigit).take(3) }, enabled = !busy, singleLine = true, label = { Text(tr("Seconds between")) },
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number), modifier = Modifier.weight(1f),
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 8.dp)) {
                Button(onClick = {
                    if (busy) Crawl.stop()
                    else if (start.isNotEmpty()) Crawl.start(start, max.toIntOrNull() ?: 8, depth.toIntOrNull() ?: 3, interval.toIntOrNull() ?: 15)
                    else Node.lastError.value = "Choose a node to start from"
                }) { Text(if (busy) tr("Stop") else tr("Start")) }
                OutlinedButton(enabled = run?.edges?.isNotEmpty() == true, onClick = {
                    val r = run ?: return@OutlinedButton
                    val i = Intent(Intent.ACTION_SEND).apply {
                        type = "text/csv"
                        putExtra(Intent.EXTRA_SUBJECT, "network-crawl.csv")
                        putExtra(Intent.EXTRA_TEXT, Crawl.csv(r))
                    }
                    runCatching { ctx.startActivity(Intent.createChooser(i, "Export CSV")) }
                }) { Text(tr("Export CSV")) }
            }
        }
        val r = run
        if (r != null) {
            val s = Crawl.summary(r)
            val muted = MaterialTheme.colorScheme.onSurfaceVariant
            GroupTitle("Result")
            Card2 {
                Text("${s.asked} nodes asked, ${s.answered} answered, ${s.links} links found" + if (busy) " (still going)" else "", fontWeight = FontWeight.SemiBold)
                if (s.silent > 0) Text("${s.silent} did not answer: you must be an administrator of a node, and it must be in reach.", style = MaterialTheme.typography.bodySmall, color = muted)
                if (s.odd > 0) Text("${s.odd} links look uneven or one-way: check the antennas or the settings at that end.", style = MaterialTheme.typography.bodySmall, color = muted)
                if (s.unknown > 0) Text("${s.unknown} neighbours are not known here (no announce heard), so they were not asked.", style = MaterialTheme.typography.bodySmall, color = muted)
            }
            GroupTitle("Nodes asked")
            Card2 {
                for (n in r.nodes) {
                    val what = when (n.state) {
                        Crawl.State.OK -> "answered, ${n.items.size} neighbours"
                        Crawl.State.WAITING -> "waiting…"
                        Crawl.State.REFUSED -> "refused: ${n.text}"
                        Crawl.State.STOPPED -> "stopped"
                        Crawl.State.SILENT -> "no answer"
                    }
                    Text("${Node.label(n.addr)}: $what", style = MaterialTheme.typography.bodyMedium)
                }
            }
            val links = Crawl.links(r)
            if (links.isNotEmpty()) {
                GroupTitle("Links")
                Card2 {
                    for (l in links) {
                        Text("${Node.label(l.a)} ↔ ${l.bName}", style = MaterialTheme.typography.bodyMedium)
                        Text(
                            "first hears second ${snr(l.ab)} · second hears first ${snr(l.ba)}" + if (l.note.isNotEmpty()) " · ${l.note}" else "",
                            style = MaterialTheme.typography.bodySmall, color = muted,
                        )
                    }
                }
            }
        }
    }
}

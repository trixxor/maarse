package dk.maarse.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import dk.maarse.app.Battery
import dk.maarse.app.StatusSource
import dk.maarse.app.batteryOf
import dk.maarse.app.plainRows

/** The radio's battery, big; or why there is none to show. */
@Composable
internal fun BatteryCard(s: Map<Int, Int>, source: StatusSource) {
    val battery = batteryOf(s, source) ?: return
    Card2 {
        Text(if (source == StatusSource.HOST) "Battery of the connected radio" else "Radio battery", fontWeight = FontWeight.SemiBold)
        when (battery) {
            is Battery.Read -> {
                val low = (battery.percent ?: 100) < 20 && !battery.external
                Text(
                    listOfNotNull(battery.percent?.let { "$it %" }, "%.2f V".format(battery.mv / 1000.0), if (battery.external) "charging or on USB power" else null).joinToString("  ·  "),
                    style = MaterialTheme.typography.headlineMedium, fontWeight = FontWeight.Bold,
                )
                LinearProgressIndicator(
                    progress = { (battery.percent ?: 0).coerceIn(3, 100) / 100f },
                    modifier = Modifier.fillMaxWidth().height(14.dp).clip(RoundedCornerShape(7.dp)),
                    color = if (low) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.primary,
                )
                if (low) Text("The battery is low. Charge the radio soon.", color = MaterialTheme.colorScheme.error)
            }
            Battery.NoReading -> Text("No battery reading from this radio. The battery must be connected to the radio board's own battery pads.",
                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Battery.NotReceived -> Text("The battery has not been received yet. Press Fetch again.",
                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

/** A few facts in everyday words; the numbers behind them are under Details. */
@Composable
internal fun InShortCard(s: Map<Int, Int>, airtime: Boolean = true) {
    val rows = plainRows(s, airtime)
    if (rows.isEmpty()) return
    Card2 {
        Text("In short", fontWeight = FontWeight.SemiBold)
        rows.forEach { (label, value) ->
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Text(label, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium)
                Text(value, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Medium)
            }
        }
    }
}

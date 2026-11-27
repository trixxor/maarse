package dk.maarse.app

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/**
 * DHT check: asks the host (the one built into this app, or the one the app is connected to) to check the
 * backbone DHT: do the contacts answer, can the other hosts find the records this host publishes, do they agree,
 * and does a lookup from here succeed. The host answers with one line per finding. Same behaviour as the web
 * client's DHT check. Costs a few internet requests and no radio airtime.
 */
object DhtCheck {
    data class Line(val level: Int, val text: String)
    data class Run(val req: Int, val lines: List<Line> = emptyList(), val done: Boolean = false)

    val LEVELS = listOf("Info", "OK", "Check", "Problem")
    const val TIMEOUT_MS = 60_000L

    val run = MutableStateFlow<Run?>(null)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private var timer: Job? = null

    fun start() {
        timer?.cancel()
        val req = Node.req()
        run.value = Run(req)
        Node.send(Cmd.dhtCheck(req))
        timer = scope.launch {
            delay(TIMEOUT_MS)
            run.update { r ->
                if (r == null || r.done || r.req != req) r
                else r.copy(
                    done = true,
                    lines = r.lines.ifEmpty { listOf(Line(3, "No answer. Only a host (the PC program or a server) has a DHT; a radio connected directly does not.")) },
                )
            }
        }
    }

    /** One line of the report; true if it belonged to the running check. */
    fun onLine(ev: Ev.DhtLine): Boolean {
        var mine = false
        run.update { r ->
            if (r == null || r.done || r.req != ev.req) r
            else { mine = true; r.copy(lines = r.lines + Line(ev.level, ev.text), done = ev.last) }
        }
        if (mine && ev.last) timer?.cancel()
        return mine
    }

    fun worst(r: Run) = r.lines.maxOfOrNull { it.level } ?: 0
}

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
 * Managing another Må:rse node (a repeater in the field) as one of its administrators, over LoRa or the internet.
 * No login: every command is signed by us. Over the air every byte costs airtime, so the default is a short status
 * (a few numbers in one reply); the full status and the neighbour list are asked for only when you press their buttons.
 *
 * A radio change is a trial: the node uses the new settings but saves them only when we confirm. If the confirmation
 * does not reach it in time it goes back to its old settings. Our own radio must be able to hear it on the new
 * settings for the confirmation to arrive (change ours too, or reach the node over the internet).
 */
object RemoteAdmin {
    /** Uptime, role, traffic, errors, queue, signal, noise, battery, temperature, crashes and the radio trial left. */
    val SHORT = listOf(1, 7, 10, 11, 12, 13, 14, 32, 33, 40, 41, 44, 34, 35, 80, 49, 85, 82, 86, 90, 118)
    const val TRIAL_MINUTES = 5

    /** The trial: the settings tried, when it ends, and how far it is: applied, confirming, saved or expired. */
    data class Trial(val target: String, val cfg: RadioCfg, val until: Long, val phase: String)

    val target = MutableStateFlow("")
    val full = MutableStateFlow(false)
    val stats = MutableStateFlow<Map<Int, Int>>(emptyMap())
    val neighbors = MutableStateFlow<List<Neighbor>?>(null)
    val radio = MutableStateFlow<RadioCfg?>(null)
    val trial = MutableStateFlow<Trial?>(null)
    val message = MutableStateFlow("")
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private var expiry: Job? = null
    private var retry: Job? = null

    private fun send(inner: ByteArray) = Node.send(Cmd.remote(Node.req(), target.value, inner))

    /** Starts managing `addr`: forget what we knew of another node and ask for its short status and radio settings. */
    fun open(addr: String) {
        if (target.value != addr) {
            target.value = addr; stats.value = emptyMap(); neighbors.value = null; radio.value = null; message.value = ""
            if (trial.value?.target != addr) trial.value = null
        }
        fetch()
        send(Cmd.getRadio())
    }

    fun fetch() {
        stats.value = emptyMap()
        send(if (full.value) Cmd.getStats() else Cmd.getStatsKeys(SHORT))
    }

    fun setFull(on: Boolean) { full.value = on; fetch() }

    fun askNeighbors() { neighbors.value = null; send(Cmd.getNeighbors()) }

    /** Tries new radio settings on the node for [TRIAL_MINUTES] minutes. */
    fun tryRadio(cfg: RadioCfg) {
        val to = target.value
        stopTimers()
        val until = System.currentTimeMillis() + TRIAL_MINUTES * 60_000L
        trial.value = Trial(to, cfg, until, "applied")
        send(Cmd.setRadioTrial(cfg, TRIAL_MINUTES))
        expiry = scope.launch {
            delay(until - System.currentTimeMillis())
            trial.value?.takeIf { it.phase == "applied" || it.phase == "confirming" }?.let {
                trial.value = it.copy(phase = "expired")
                retry?.cancel()
                message.value = "The radio change was not confirmed in time, so the node goes back to its old settings."
            }
        }
    }

    /** Confirms the trial and keeps asking every 15 seconds until the node answers with the settings it now runs. */
    fun confirm() {
        val t = trial.value ?: return
        if (t.phase != "applied" && t.phase != "confirming") return
        trial.value = t.copy(phase = "confirming")
        retry?.cancel()
        retry = scope.launch {
            while (trial.value?.phase == "confirming") {
                send(Cmd.confirmRadio())
                delay(15_000)
            }
        }
    }

    private fun stopTimers() { expiry?.cancel(); retry?.cancel() }

    fun onReply(from: String, inner: Ev?) {
        if (from != target.value) return
        when (inner) {
            is Ev.Stats -> stats.update { old ->
                // A short status is one frame; a full one comes in chunks: a frame with key 1 starts a new set.
                val m = if (inner.items.any { it.first == 1 }) mutableMapOf() else old.toMutableMap()
                inner.items.forEach { (k, v) -> m[k] = v }
                m
            }
            is Ev.Neighbors -> neighbors.update { if (inner.part == 0) inner.items else (it ?: emptyList()) + inner.items }
            is Ev.Radio -> {
                radio.value = inner.cfg
                val t = trial.value
                // After the confirmation, settings equal to the trial's mean the node saved them.
                if (t != null && t.target == from && t.phase == "confirming" && t.cfg == inner.cfg) {
                    trial.value = t.copy(phase = "saved"); stopTimers()
                    message.value = "Saved. The node confirmed the new radio settings."
                }
            }
            is Ev.Error -> message.value = inner.text
            else -> {}
        }
    }
}


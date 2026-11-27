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
 * Network crawl: who hears whom. Starts at one repeater we administer, asks it for its neighbour list over remote
 * control (one small reply), then asks each neighbour we also administer, and so on. One question at a time with a
 * pause between them, so the crawl costs little airtime. Same behaviour as the web client's Network crawl.
 */
object Crawl {
    enum class State { WAITING, OK, SILENT, REFUSED, STOPPED }

    data class Asked(val addr: String, val depth: Int, val state: State = State.WAITING, val items: List<Neighbor> = emptyList(), val text: String = "")
    data class Edge(val from: String, val to: String?, val prefix: String, val snr: Int)
    data class Run(
        val start: String, val max: Int, val depth: Int, val intervalS: Int,
        val nodes: List<Asked> = emptyList(), val edges: List<Edge> = emptyList(), val unknown: Set<String> = emptySet(), val finished: Boolean = false,
    )

    /** One pair of nodes. [ab]: how well a hears b; [ba]: how well b hears a (null if b was not asked or does not list a). */
    data class Link(val a: String, val b: String, val bName: String, val ab: Int, val ba: Int?, val note: String)
    data class Summary(val asked: Int, val answered: Int, val silent: Int, val links: Int, val odd: Int, val unknown: Int)

    const val TIMEOUT_MS = 40_000L
    const val ASYMMETRY_DB = 6
    const val UNKNOWN = "node not known here"

    val run = MutableStateFlow<Run?>(null)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
    private var job: Job? = null
    private val lock = Any()
    @Volatile private var waitingFor: String? = null
    private var partial = emptyList<Neighbor>()

    private fun resolve(prefix: String): String? = Node.nodes.value.keys.firstOrNull { it.startsWith(prefix) }

    fun start(start: String, max: Int, depth: Int, intervalS: Int) {
        stop()
        val r = Run(start, max.coerceIn(1, 40), depth.coerceIn(1, 8), intervalS.coerceIn(5, 300))
        run.value = r
        Node.note("Network crawl from ${Node.label(start)}: at most ${r.max} nodes, ${r.depth} hops deep, one question every ${r.intervalS} s")
        job = scope.launch {
            val queue = ArrayDeque(listOf(start to 0))
            val seen = mutableSetOf(start)
            while (queue.isNotEmpty() && (run.value?.nodes?.size ?: 0) < r.max) {
                val (addr, hops) = queue.removeFirst()
                synchronized(lock) {
                    partial = emptyList()
                    waitingFor = addr
                    run.update { it?.copy(nodes = it.nodes + Asked(addr, hops)) }
                }
                Node.send(Cmd.remote(Node.req(), addr, Cmd.getNeighbors()))
                var waited = 0L
                while (waited < TIMEOUT_MS && waitingFor == addr) { delay(250); waited += 250 }
                synchronized(lock) {
                    if (waitingFor == addr) { waitingFor = null; setState(addr, State.SILENT) }
                    run.value?.edges?.filter { it.from == addr && it.to != null }?.forEach { e ->
                        if (seen.add(e.to!!) && hops < r.depth) queue.addLast(e.to to hops + 1)
                    }
                }
                if (queue.isNotEmpty()) delay(r.intervalS * 1000L)
            }
            finish()
        }
    }

    fun stop() {
        job?.cancel()
        synchronized(lock) {
            waitingFor?.let { setState(it, State.STOPPED) }
            waitingFor = null
        }
        finish()
    }

    private fun finish() { run.update { it?.copy(finished = true) } }

    private fun setState(addr: String, st: State, text: String = "") {
        run.update { r -> r?.copy(nodes = r.nodes.map { if (it.addr == addr && it.state == State.WAITING) it.copy(state = st, text = text) else it }) }
    }

    /** A reply from a node over remote control; true if the crawl was waiting for it. */
    fun onReply(from: String, inner: Ev?): Boolean = synchronized(lock) {
        if (waitingFor != from) return false
        when (inner) {
            is Ev.Error -> {
                setState(from, State.REFUSED, inner.text)
                waitingFor = null
            }
            is Ev.Neighbors -> {
                partial = if (inner.part == 0) inner.items else partial + inner.items
                if (!inner.last) return true
                val items = partial
                val edges = items.map { Edge(from, resolve(it.prefix), it.prefix, it.snr) }
                run.update { r ->
                    r?.copy(
                        nodes = r.nodes.map { if (it.addr == from) it.copy(state = State.OK, items = items) else it },
                        edges = r.edges + edges,
                        unknown = r.unknown + edges.filter { it.to == null }.map { it.prefix },
                    )
                }
                waitingFor = null
            }
            else -> return false
        }
        true
    }

    fun links(r: Run): List<Link> {
        val asked = r.nodes.filter { it.state == State.OK }.map { it.addr }.toSet()
        val heard = r.edges.filter { it.to != null }.associateBy { it.from + ">" + it.to }
        val done = mutableSetOf<String>()
        val rows = mutableListOf<Link>()
        for (e in r.edges) {
            val b = e.to ?: e.prefix
            if (!done.add(listOf(e.from, b).sorted().joinToString("|"))) continue
            val ba = if (e.to != null) heard[b + ">" + e.from] else null
            val note = when {
                e.to == null -> UNKNOWN
                ba != null -> if (Math.abs(e.snr - ba.snr) >= ASYMMETRY_DB) "uneven: one side hears it much better" else ""
                b in asked -> "heard one way only"
                else -> ""
            }
            rows += Link(e.from, b, if (e.to != null) Node.label(b) else b, e.snr, ba?.snr, note)
        }
        return rows.sortedWith(compareBy({ it.note.isEmpty() }, { Node.label(it.a).lowercase() }))
    }

    fun summary(r: Run): Summary {
        val l = links(r)
        return Summary(
            r.nodes.size, r.nodes.count { it.state == State.OK }, r.nodes.count { it.state == State.SILENT || it.state == State.REFUSED },
            l.size, l.count { it.note.isNotEmpty() && it.note != UNKNOWN }, r.unknown.size,
        )
    }

    fun csv(r: Run): String =
        (listOf("node_a,node_b,a_hears_b_snr_db,b_hears_a_snr_db,note") + links(r).map { "${it.a},${it.b},${it.ab},${it.ba ?: ""},${it.note}" })
            .joinToString("\n") + "\n"
}

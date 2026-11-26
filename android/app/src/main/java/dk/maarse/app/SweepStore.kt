package dk.maarse.app

import android.content.Context
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/** The spectrum recordings (`.msr` files, see [SweepRec]) kept in the app's own folder. */
object SweepStore {
    class Item(val file: File) {
        val name: String get() = file.nameWithoutExtension
        val at: Long get() = file.lastModified()
        val id: String get() = file.name
    }

    private lateinit var dir: File
    val items = MutableStateFlow<List<Item>>(emptyList())
    /** The recording the Recordings page opens (set when a recording has just been saved). */
    val open = MutableStateFlow<String?>(null)

    fun init(ctx: Context) {
        dir = File(ctx.applicationContext.filesDir, "recordings").also { it.mkdirs() }
        refresh()
    }

    fun refresh() {
        items.value = (dir.listFiles { f -> f.extension == "msr" } ?: emptyArray()).map { Item(it) }.sortedByDescending { it.at }
    }

    /** Saves a recording under a name (characters a file name cannot hold are replaced). Returns the item. */
    fun save(name: String, bytes: ByteArray): Item {
        val safe = name.replace(Regex("[^\\w .-]+"), "-").trim().ifEmpty { "Spectrum" }
        var f = File(dir, "$safe.msr")
        var n = 2
        while (f.exists()) f = File(dir, "$safe ($n).msr").also { n++ }
        f.writeBytes(bytes)
        refresh()
        return Item(f)
    }

    fun read(item: Item): ByteArray = item.file.readBytes()

    fun delete(item: Item) { item.file.delete(); refresh() }
}

/**
 * A recording being made from the live sweeps (Experimental > Spectrum > Record). The radio ends a run of sweeps after
 * 2 minutes by itself, so this stops and starts it again every 105 s; the gap is a fraction of a second.
 * Leaving the Spectrum page cancels it. The radio hears no messages while it sweeps.
 */
object SweepRecording {
    private const val START_KHZ = 863_000L
    private const val STEP_KHZ = 110
    private const val RESTART_MS = 105_000L

    class Active(val rec: SweepRec.Recorder, val t0: Long, val durationS: Int) { @Volatile var masked = 0 }

    val active = MutableStateFlow<Active?>(null)
    val elapsedMs = MutableStateFlow(0L)
    val message = MutableStateFlow("")
    /** Set when a recording has just been saved, so the page can offer to open it. */
    val saved = MutableStateFlow<SweepStore.Item?>(null)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private var job: Job? = null

    private fun startSweeps() = Node.send(Cmd.sweep(START_KHZ, STEP_KHZ, SweepRec.BINS, 255))
    private fun stopSweeps() = Node.send(Cmd.sweep(0, 0, 0, 0))

    fun start(durationS: Int) {
        cancel()
        message.value = ""; saved.value = null
        Node.send(Cmd.sweep(0, 0, 0, 0))
        Node.clearSweeps()
        val a = Active(SweepRec.Recorder(SweepRec.Config(START_KHZ, STEP_KHZ, durationS, 100, 6)), System.currentTimeMillis(), durationS)
        active.value = a
        elapsedMs.value = 0
        startSweeps()
        job = scope.launch {
            var nextRestart = RESTART_MS
            while (active.value === a) {
                delay(500)
                val el = System.currentTimeMillis() - a.t0
                elapsedMs.value = el
                if (el >= durationS * 1000L) { finish(); break }
                if (el >= nextRestart) {
                    nextRestart += RESTART_MS
                    stopSweeps(); delay(300)
                    if (active.value === a) startSweeps()
                }
            }
        }
    }

    /** Called for every sweep event. */
    fun onSweep(ev: Ev.Sweep) {
        val a = active.value ?: return
        if (ev.startKhz != START_KHZ) return
        val over = SweepRec.isOverload(ev.rssi)
        if (over) a.masked++
        a.rec.push(System.currentTimeMillis() - a.t0, ev.rssi, over)
    }

    fun finish() {
        val a = active.value ?: return
        job?.cancel(); active.value = null
        stopSweeps()
        if (a.rec.sweeps == 0) { message.value = "No sweeps arrived. Is a radio connected?"; return }
        val bytes = a.rec.finish()
        val name = "Spectrum " + SimpleDateFormat("yyyy-MM-dd HH-mm", Locale.US).format(Date(a.t0))
        val item = SweepStore.save(name, bytes)
        SweepStore.open.value = item.id
        saved.value = item
        message.value = "Saved ${item.name} (${bytes.size} bytes)."
    }

    fun cancel() {
        if (active.value == null) return
        job?.cancel(); active.value = null; stopSweeps()
    }
}

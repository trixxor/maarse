package dk.maarse.app

import android.annotation.SuppressLint
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioRecord
import android.media.AudioTrack
import android.media.MediaCodec
import android.media.MediaFormat
import android.media.MediaRecorder
import kotlinx.coroutines.flow.MutableStateFlow
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.security.SecureRandom
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/**
 * Calls: Opus over maarse-host (only over the internet all the way; the host
 * refuses with NOT_OVER_TCP otherwise). The same content as the web client:
 *
 * ```text
 * type (u8) | call ID (u32 LE) | data
 *   1 = ring, 2 = answer, 3 = hang up / reject / busy,
 *   4 = audio: seq (u16 LE) | (length (u8) | Opus frame) ×3   (20 ms frames, 16 kbit/s, 48 kHz mono)
 * ```
 */
object Calls {
    const val RING = 1; const val ANSWER = 2; const val END = 3; const val AUDIO = 4
    private const val RATE = 48000
    private const val FRAME = 960 // 20 ms
    /** Bytes of (length | frame) that fit one call packet after sealing (see maarse-host calltest.rs). */
    private const val AUDIO_BUDGET = 119

    enum class Phase { IDLE, CALLING, RINGING, ACTIVE }
    data class State(val phase: Phase = Phase.IDLE, val peer: String = "", val started: Long = 0, val muted: Boolean = false, val speaker: Boolean = false, val note: String = "")

    val state = MutableStateFlow(State())

    /** Network quality during a call: packets received and lost (from sequence gaps), times playback ran dry. */
    data class Quality(val got: Int = 0, val lost: Int = 0, val dry: Int = 0)
    val quality = MutableStateFlow(Quality())
    private var lastSeq = -1
    private var id = 0
    private var seq = 0
    private var media: Media? = null
    private lateinit var app: Context

    fun init(ctx: Context) {
        app = ctx.applicationContext
        app.getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel("calls", "Calls", NotificationManager.IMPORTANCE_HIGH).apply { description = "Incoming calls" })
    }

    /** Calls need a host (the phone node or a host elsewhere), not a radio alone. */
    fun possible() = Node.prefs.mode == Mode.PHONE || Node.prefs.mode == Mode.REMOTE

    private fun frame(kind: Int, extra: ByteArray = ByteArray(0)): ByteArray =
        ByteBuffer.allocate(5 + extra.size).order(ByteOrder.LITTLE_ENDIAN).put(kind.toByte()).putInt(id).put(extra).array()

    private fun send(kind: Int, extra: ByteArray = ByteArray(0)) {
        val peer = state.value.peer
        Node.post { Node.send(Cmd.sendCall(peer, frame(kind, extra))) }
    }

    /** Call test on a backbone: 1 music, 2 echo (a normal call screen), 3 speed (no sound; see speed). */
    fun startTest(peer: String, mode: Int) {
        if (mode == 3) return speedTest(peer)
        start(peer, mode)
    }

    /** Speed test result, shown in the backbone menu. */
    data class Speed(val running: Boolean = false, val bytes: Long = 0, val got: Int = 0, val lost: Int = 0, val ms: Long = 0, val note: String = "")
    val speed = MutableStateFlow(Speed())
    private var speedId = 0; private var speedPeer = ""; private var speedT0 = 0L; private var speedSeq = -1

    private fun speedTest(peer: String) {
        if (speed.value.running) return
        speedId = SecureRandom().nextInt(); speedPeer = peer; speedT0 = 0; speedSeq = -1
        speed.value = Speed(running = true, note = "Calling the backbone…")
        val ring = ByteBuffer.allocate(6).order(ByteOrder.LITTLE_ENDIAN).put(RING.toByte()).putInt(speedId).put(3).array()
        Node.post { Node.send(Cmd.sendCall(peer, ring)) }
        android.os.Handler(android.os.Looper.getMainLooper()).postDelayed({
            if (speed.value.running) speed.value = speed.value.copy(running = false, note = "No answer or not finished in time")
        }, 20_000)
    }

    private fun onSpeed(kind: Int, d: ByteArray) {
        val now = System.currentTimeMillis()
        when (kind) {
            ANSWER -> { speedT0 = now; speed.value = speed.value.copy(note = "Measuring for 5 s…") }
            AUDIO -> if (d.size >= 7) {
                val sq = (d[5].toInt() and 0xff) or ((d[6].toInt() and 0xff) shl 8)
                val gap = if (speedSeq < 0) 0 else ((sq - speedSeq - 1) and 0xffff).let { if (it > 1000) 0 else it }
                speedSeq = sq
                speed.value = speed.value.let { it.copy(bytes = it.bytes + d.size - 7, got = it.got + 1, lost = it.lost + gap, ms = now - speedT0) }
            }
            END -> speed.value = speed.value.copy(running = false, note = "")
        }
    }

    fun start(peer: String, mode: Int = 0) {
        if (state.value.phase != Phase.IDLE) return
        id = SecureRandom().nextInt(); seq = 0
        state.value = State(Phase.CALLING, peer)
        send(RING, if (mode > 0) byteArrayOf(mode.toByte()) else ByteArray(0))
        Node.post { }
        android.os.Handler(android.os.Looper.getMainLooper()).postDelayed({
            if (state.value.phase == Phase.CALLING && state.value.peer == peer) { send(END); stop("No answer") }
        }, 45_000)
    }

    fun answer() {
        if (state.value.phase != Phase.RINGING) return
        send(ANSWER)
        begin()
    }

    fun hangup() {
        if (state.value.phase == Phase.IDLE) return
        send(END)
        stop("")
    }

    fun toggleMute() { state.value = state.value.copy(muted = !state.value.muted); media?.muted = state.value.muted }

    fun toggleSpeaker() {
        val on = !state.value.speaker
        state.value = state.value.copy(speaker = on)
        @Suppress("DEPRECATION")
        app.getSystemService(AudioManager::class.java).isSpeakerphoneOn = on
    }

    /** The host refused (not reachable over the internet all the way). */
    fun failed(why: String) { if (state.value.phase != Phase.IDLE) stop(why) }

    private fun stop(why: String) {
        media?.close(); media = null
        app.getSystemService(NotificationManager::class.java).cancel("call", 0)
        app.getSystemService(AudioManager::class.java).mode = AudioManager.MODE_NORMAL
        state.value = State(note = why)
        if (why.isNotEmpty()) Node.note("Call: $why")
    }

    private fun begin() {
        quality.value = Quality(); lastSeq = -1
        app.getSystemService(NotificationManager::class.java).cancel("call", 0)
        val m = runCatching { Media() }.getOrElse {
            send(END); stop("Calls need Android 10 or newer (Opus) and the microphone: ${it.message}"); return
        }
        media = m
        app.getSystemService(AudioManager::class.java).mode = AudioManager.MODE_IN_COMMUNICATION
        state.value = state.value.copy(phase = Phase.ACTIVE, started = System.currentTimeMillis())
    }

    /** Call content from another node (event 0xa5). */
    fun onData(from: String, d: ByteArray) {
        if (d.size < 5) return
        val b = ByteBuffer.wrap(d).order(ByteOrder.LITTLE_ENDIAN)
        val kind = b.get(0).toInt(); val cid = b.getInt(1)
        if (speedId != 0 && cid == speedId && from == speedPeer) return onSpeed(kind, d)
        val s = state.value
        if (kind == RING) {
            if (s.phase != Phase.IDLE) { // busy
                Node.send(Cmd.sendCall(from, ByteBuffer.allocate(5).order(ByteOrder.LITTLE_ENDIAN).put(END.toByte()).putInt(cid).array()))
                return
            }
            id = cid; seq = 0
            state.value = State(Phase.RINGING, from)
            ringNotification(from)
            return
        }
        if (from != s.peer || cid != id) return
        when {
            kind == ANSWER && s.phase == Phase.CALLING -> begin()
            kind == END -> stop(if (s.phase == Phase.CALLING) "Rejected or busy" else "The other side hung up")
            kind == AUDIO && s.phase == Phase.ACTIVE -> {
                if (d.size >= 7) {
                    val sq = (d[5].toInt() and 0xff) or ((d[6].toInt() and 0xff) shl 8)
                    val gap = if (lastSeq < 0) 0 else ((sq - lastSeq - 1) and 0xffff).let { if (it > 1000) 0 else it }
                    lastSeq = sq
                    quality.value = quality.value.let { it.copy(got = it.got + 1, lost = it.lost + gap) }
                }
                var o = 7
                while (o < d.size) {
                    val n = d[o].toInt() and 0xff
                    if (n == 0 || o + 1 + n > d.size) break
                    media?.play(d.copyOfRange(o + 1, o + 1 + n))
                    o += 1 + n
                }
            }
        }
    }

    private fun ringNotification(from: String) {
        val open = PendingIntent.getActivity(app, 7, Intent(app, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP), PendingIntent.FLAG_IMMUTABLE)
        val n = Notification.Builder(app, "calls").setSmallIcon(R.drawable.ic_stat)
            .setContentTitle("${Node.label(from)} is calling").setContentText("Tap to answer")
            .setCategory(Notification.CATEGORY_CALL).setContentIntent(open).setFullScreenIntent(open, true).setOngoing(true).build()
        runCatching { app.getSystemService(NotificationManager::class.java).notify("call", 0, n) }
    }

    /** Microphone → Opus → network, and network → Opus → speaker. */
    private class Media {
        @Volatile var muted = false
        @Volatile private var on = true
        private val rec: AudioRecord
        private val enc: MediaCodec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_AUDIO_OPUS)
        private val dec: MediaCodec = MediaCodec.createDecoderByType(MediaFormat.MIMETYPE_AUDIO_OPUS)
        private val track: AudioTrack
        private val inQ = LinkedBlockingQueue<ByteArray>(50)
        private var threads: List<Thread> = emptyList()

        init {
            val min = AudioRecord.getMinBufferSize(RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
            @SuppressLint("MissingPermission")
            val r = AudioRecord(MediaRecorder.AudioSource.VOICE_COMMUNICATION, RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT, maxOf(min, FRAME * 8))
            check(r.state == AudioRecord.STATE_INITIALIZED) { "no microphone" }
            rec = r
            enc.configure(MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_OPUS, RATE, 1).apply {
                setInteger(MediaFormat.KEY_BIT_RATE, 16000)
            }, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
            // Opus decoder set-up data: OpusHead, pre-skip and seek pre-roll (ns).
            val head = ByteBuffer.allocate(19).order(ByteOrder.LITTLE_ENDIAN)
                .put("OpusHead".toByteArray()).put(1).put(1).putShort(312).putInt(RATE).putShort(0).put(0).array()
            val ns = { v: Long -> ByteBuffer.allocate(8).order(ByteOrder.LITTLE_ENDIAN).putLong(v).array() }
            dec.configure(MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_OPUS, RATE, 1).apply {
                setByteBuffer("csd-0", ByteBuffer.wrap(head))
                setByteBuffer("csd-1", ByteBuffer.wrap(ns(312L * 1_000_000_000 / RATE)))
                setByteBuffer("csd-2", ByteBuffer.wrap(ns(80_000_000)))
            }, null, null, 0)
            track = AudioTrack.Builder()
                .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION).setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build())
                .setAudioFormat(AudioFormat.Builder().setSampleRate(RATE).setEncoding(AudioFormat.ENCODING_PCM_16BIT).setChannelMask(AudioFormat.CHANNEL_OUT_MONO).build())
                .setTransferMode(AudioTrack.MODE_STREAM)
                .setBufferSizeInBytes(maxOf(AudioTrack.getMinBufferSize(RATE, AudioFormat.CHANNEL_OUT_MONO, AudioFormat.ENCODING_PCM_16BIT), FRAME * 2 * 25)).build()
            enc.start(); dec.start(); rec.startRecording(); track.play()
            threads = listOf(Thread({ guard(::sendLoop) }, "call-send"), Thread({ guard(::playLoop) }, "call-play")).onEach { it.start() }
        }

        /** A codec error ends the loop, never the app. */
        private fun guard(loop: () -> Unit) { runCatching(loop) }

        fun play(opus: ByteArray) { inQ.offer(opus) }

        private fun sendLoop() {
            val pcm = ByteArray(FRAME * 2)
            val info = MediaCodec.BufferInfo()
            val pending = ArrayList<ByteArray>()
            var pts = 0L
            while (on) {
                var got = 0
                while (got < pcm.size && on) { val n = rec.read(pcm, got, pcm.size - got); if (n <= 0) break; got += n }
                if (muted) java.util.Arrays.fill(pcm, 0)
                val i = enc.dequeueInputBuffer(10_000)
                if (i >= 0) {
                    enc.getInputBuffer(i)!!.apply { clear(); put(pcm) }
                    enc.queueInputBuffer(i, 0, pcm.size, pts, 0); pts += 20_000
                }
                while (true) {
                    val o = enc.dequeueOutputBuffer(info, 0)
                    if (o < 0) break
                    if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG == 0 && info.size in 1..255) {
                        val b = ByteArray(info.size); enc.getOutputBuffer(o)!!.apply { position(info.offset); get(b) }
                        // Opus is variable rate: send early rather than make a
                        // packet too large to go through (it would be dropped).
                        if (pending.isNotEmpty() && pending.sumOf { it.size + 1 } + 1 + b.size > AUDIO_BUDGET) flush(pending)
                        pending += b
                    }
                    enc.releaseOutputBuffer(o, false)
                }
                if (pending.size >= 3) flush(pending)
            }
        }

        private fun flush(pending: ArrayList<ByteArray>) {
            val body = java.io.ByteArrayOutputStream()
            body.write(seq and 0xff); body.write((seq shr 8) and 0xff)
            pending.forEach { body.write(it.size); body.write(it) }
            pending.clear(); seq = (seq + 1) and 0xffff
            send(AUDIO, body.toByteArray())
        }

        private fun playLoop() {
            val info = MediaCodec.BufferInfo()
            var pts = 0L
            // Jitter buffer: packets carry 60 ms and come over the internet in
            // bursts. Wait for 160 ms of audio before playing, and again after
            // running dry, instead of playing each packet the moment it lands
            // (that gave choppy, crackling sound).
            var buffering = true
            while (on) {
                if (buffering) {
                    if (inQ.size < 8) { Thread.sleep(10); continue }
                    buffering = false
                }
                val f = inQ.poll(50, TimeUnit.MILLISECONDS)
                if (f == null) { buffering = true; quality.value = quality.value.let { it.copy(dry = it.dry + 1) }; continue }
                run {
                    val i = dec.dequeueInputBuffer(10_000)
                    if (i >= 0) { dec.getInputBuffer(i)!!.apply { clear(); put(f) }; dec.queueInputBuffer(i, 0, f.size, pts, 0); pts += 20_000 }
                }
                while (true) {
                    val o = dec.dequeueOutputBuffer(info, 0)
                    if (o < 0) break
                    if (info.size > 0) {
                        val b = ByteArray(info.size); dec.getOutputBuffer(o)!!.apply { position(info.offset); get(b) }
                        track.write(b, 0, b.size)
                    }
                    dec.releaseOutputBuffer(o, false)
                }
                // Too far behind: drop old audio rather than lag.
                while (inQ.size > 20) inQ.poll()
            }
        }

        fun close() {
            on = false
            // Let both loops finish before the codecs go away.
            threads.forEach { if (it !== Thread.currentThread()) it.join(500) }
            runCatching { rec.stop() }; rec.release()
            runCatching { enc.stop() }; enc.release()
            runCatching { dec.stop() }; dec.release()
            runCatching { track.stop() }; track.release()
        }
    }
}

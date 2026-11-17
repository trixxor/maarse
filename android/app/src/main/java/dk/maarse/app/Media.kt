package dk.maarse.app

import android.annotation.SuppressLint
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.AudioTrack
import android.media.MediaRecorder
import java.io.ByteArrayOutputStream
import java.io.File

/**
 * Codec2 voice messages through maarse-voice (Rust, JNI): the same format as
 * the web client's, `audio/x-codec2`, 8 kHz mono. At 1200 bit/s ten seconds
 * are about 1.5 KB, small enough for LoRa.
 */
object Voice {
    const val MIME = "audio/x-codec2"
    const val MODE = 5 // 1200 bit/s
    const val RATE = 8000
    const val MAX_S = 20

    init { System.loadLibrary("maarse_android") }

    @JvmStatic external fun encode(pcm: ShortArray, mode: Int): ByteArray?
    @JvmStatic external fun decode(data: ByteArray): ShortArray?

    /** Seconds in a voice message (header: "C2" | mode | frame count u16). */
    fun seconds(data: ByteArray): Int {
        if (data.size < 5) return 0
        val frames = (data[3].toInt() and 0xff) or ((data[4].toInt() and 0xff) shl 8)
        val ms = when (data[2].toInt()) { 0, 1 -> 20; else -> 40 }
        return (frames * ms + 999) / 1000
    }
}

/** Records a voice message (tap to start, tap to stop, at most 20 s). */
class VoiceRecorder {
    @Volatile private var on = false
    private var thread: Thread? = null
    private val pcm = ByteArrayOutputStream()
    var startedAt = 0L; private set

    @SuppressLint("MissingPermission")
    fun start(): Boolean {
        val min = AudioRecord.getMinBufferSize(Voice.RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        val rec = runCatching {
            AudioRecord(MediaRecorder.AudioSource.VOICE_RECOGNITION, Voice.RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT, maxOf(min, 3200))
        }.getOrNull() ?: return false
        if (rec.state != AudioRecord.STATE_INITIALIZED) { rec.release(); return false }
        pcm.reset()
        on = true
        startedAt = System.currentTimeMillis()
        rec.startRecording()
        thread = Thread({
            val buf = ByteArray(1600)
            while (on && System.currentTimeMillis() - startedAt < Voice.MAX_S * 1000L) {
                val n = rec.read(buf, 0, buf.size)
                if (n > 0) synchronized(pcm) { pcm.write(buf, 0, n) }
            }
            rec.stop(); rec.release()
        }, "voice-rec").also { it.start() }
        return true
    }

    /** Stops and returns the encoded message (null if too short or failed). */
    fun stop(): ByteArray? {
        on = false
        thread?.join(1000)
        val b = synchronized(pcm) { pcm.toByteArray() }
        if (b.size < Voice.RATE / 2) return null // under 1/4 s
        val s = ShortArray(b.size / 2) { ((b[2 * it].toInt() and 0xff) or (b[2 * it + 1].toInt() shl 8)).toShort() }
        return Voice.encode(s, Voice.MODE)
    }

    fun cancel() { on = false }
}

/** Plays a voice message; one at a time. */
object VoicePlayer {
    private var track: AudioTrack? = null
    var playing: String? = null; private set

    fun play(path: String, done: () -> Unit) {
        stop()
        val data = runCatching { File(path).readBytes() }.getOrNull() ?: return
        val pcm = Voice.decode(data) ?: return
        val t = AudioTrack.Builder()
            .setAudioAttributes(AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).setContentType(AudioAttributes.CONTENT_TYPE_SPEECH).build())
            .setAudioFormat(AudioFormat.Builder().setSampleRate(Voice.RATE).setEncoding(AudioFormat.ENCODING_PCM_16BIT).setChannelMask(AudioFormat.CHANNEL_OUT_MONO).build())
            .setTransferMode(AudioTrack.MODE_STATIC).setBufferSizeInBytes(pcm.size * 2).build()
        t.write(pcm, 0, pcm.size)
        t.notificationMarkerPosition = pcm.size
        t.setPlaybackPositionUpdateListener(object : AudioTrack.OnPlaybackPositionUpdateListener {
            override fun onMarkerReached(a: AudioTrack) { stop(); done() }
            override fun onPeriodicNotification(a: AudioTrack) {}
        })
        track = t
        playing = path
        t.play()
    }

    fun stop() {
        track?.let { runCatching { it.stop() }; it.release() }
        track = null
        playing = null
    }
}

object Images {
    /** A picture made small enough: smaller sides and lower quality until it fits `limit`. */
    fun shrink(data: ByteArray, limit: Int): ByteArray? {
        val src = BitmapFactory.decodeByteArray(data, 0, data.size) ?: return null
        for (side in listOf(1280, 960, 640, 480, 320, 240, 160)) {
            val scale = minOf(1f, side.toFloat() / maxOf(src.width, src.height))
            val bmp = if (scale < 1f) Bitmap.createScaledBitmap(src, (src.width * scale).toInt().coerceAtLeast(1), (src.height * scale).toInt().coerceAtLeast(1), true) else src
            for (q in listOf(80, 60, 45, 30)) {
                val out = ByteArrayOutputStream()
                bmp.compress(Bitmap.CompressFormat.WEBP_LOSSY, q, out)
                if (out.size() <= limit) return out.toByteArray()
            }
        }
        return null
    }

    /** A thumbnail for the conversation (decoded at a fraction of the size). */
    fun thumb(path: String, maxSide: Int = 720): Bitmap? {
        val o = BitmapFactory.Options().apply { inJustDecodeBounds = true }
        BitmapFactory.decodeFile(path, o)
        if (o.outWidth <= 0) return null
        var s = 1
        while (maxOf(o.outWidth, o.outHeight) / (s * 2) >= maxSide) s *= 2
        return BitmapFactory.decodeFile(path, BitmapFactory.Options().apply { inSampleSize = s })
    }
}

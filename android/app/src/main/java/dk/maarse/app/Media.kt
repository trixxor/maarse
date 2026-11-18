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

    /** Over LoRa a chunk carries about 183 bytes and takes about 1.6 s (measured, SF9). */
    fun loraSecs(bytes: Int): Int = ((bytes + 182) / 183 * 1.6 + 4).toInt()

    /** Grey values of the picture, longest side `max` pixels. */
    private fun greyPixels(data: ByteArray, max: Int): Triple<IntArray, Int, Int>? {
        val src = BitmapFactory.decodeByteArray(data, 0, data.size) ?: return null
        val scale = minOf(1f, max.toFloat() / maxOf(src.width, src.height))
        val w = (src.width * scale).toInt().coerceAtLeast(1)
        val h = (src.height * scale).toInt().coerceAtLeast(1)
        val bmp = Bitmap.createScaledBitmap(src, w, h, true)
        val px = IntArray(w * h)
        bmp.getPixels(px, 0, w, 0, 0, w, h)
        for (i in px.indices) {
            val c = px[i]
            px[i] = (0.299 * ((c shr 16) and 0xff) + 0.587 * ((c shr 8) and 0xff) + 0.114 * (c and 0xff)).toInt()
        }
        return Triple(px, w, h)
    }

    /** Grey, small: low-quality WebP of about 1.5 KB at most (text stays readable). */
    fun grey(data: ByteArray): ByteArray? {
        val (g, w, h) = greyPixels(data, 256) ?: return null
        val bmp = Bitmap.createBitmap(IntArray(g.size) { 0xff shl 24 or (g[it] shl 16) or (g[it] shl 8) or g[it] }, w, h, Bitmap.Config.ARGB_8888)
        var best: ByteArray? = null
        for (q in listOf(30, 20, 10, 5)) {
            val out = ByteArrayOutputStream()
            bmp.compress(Bitmap.CompressFormat.WEBP_LOSSY, q, out)
            val b = out.toByteArray()
            if (best == null || b.size < best.size) best = b
            if (b.size <= 1500) break
        }
        return best
    }

    private val BAYER4 = intArrayOf(0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5)

    /** Dots: 1-bit ordered (Bayer) dithering after a contrast stretch, as a 1-bit PNG. */
    fun dots(data: ByteArray): ByteArray? {
        val (g, w, h) = greyPixels(data, 256) ?: return null
        val hist = IntArray(256)
        for (v in g) hist[v]++
        val cut = g.size * 0.02
        var lo = 0; var s = 0
        while (lo < 255) { s += hist[lo]; if (s >= cut) break; lo++ }
        var hi = 255; s = 0
        while (hi > 0) { s += hist[hi]; if (s >= cut) break; hi-- }
        val span = maxOf(1, hi - lo)
        val stride = (w + 7) / 8 + 1
        val raw = ByteArray(stride * h)
        for (y in 0 until h) for (x in 0 until w) {
            val v = ((g[y * w + x] - lo) * 255 / span).coerceIn(0, 255)
            if (v > (BAYER4[(y % 4) * 4 + (x % 4)] + 0.5) * 16) {
                val i = y * stride + 1 + (x shr 3)
                raw[i] = (raw[i].toInt() or (0x80 ushr (x and 7))).toByte()
            }
        }
        return pngGrey1(w, h, raw)
    }

    private fun pngGrey1(w: Int, h: Int, raw: ByteArray): ByteArray {
        val out = ByteArrayOutputStream()
        fun chunk(type: String, body: ByteArray) {
            val t = type.toByteArray(Charsets.US_ASCII) + body
            out.write(java.nio.ByteBuffer.allocate(4).putInt(body.size).array())
            out.write(t)
            val crc = java.util.zip.CRC32().apply { update(t) }.value.toInt()
            out.write(java.nio.ByteBuffer.allocate(4).putInt(crc).array())
        }
        out.write(byteArrayOf(137.toByte(), 80, 78, 71, 13, 10, 26, 10))
        chunk("IHDR", java.nio.ByteBuffer.allocate(13).putInt(w).putInt(h).put(1).put(0).put(0).put(0).put(0).array())
        val z = ByteArrayOutputStream()
        java.util.zip.DeflaterOutputStream(z, java.util.zip.Deflater(9)).use { it.write(raw) }
        chunk("IDAT", z.toByteArray())
        chunk("IEND", ByteArray(0))
        return out.toByteArray()
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

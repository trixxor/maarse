package dk.maarse.app

import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.ContentValues
import android.content.Context
import android.media.AudioAttributes
import android.media.Ringtone
import android.media.RingtoneManager
import android.net.Uri
import android.os.Build
import android.provider.MediaStore
import android.provider.OpenableColumns

/**
 * Notification sounds the person can choose, one per kind of notification.
 *
 * A sound is stored as a short text ("spec"): "" = the phone's default, "none" = silent,
 * "tone:<id>" = one of the tones that come with the app (res/raw, made by tools/make_tones.py),
 * "uri:<content uri>" = a system sound or a file the person picked. The same kinds and tone names
 * exist in the web client (TONES in maarse-client/index.html).
 *
 * Android fixes a channel's sound when the channel is made, so every choice gets its own channel
 * (the id carries a hash of the spec) and the channels of the old choices are removed.
 */
object Sounds {
    /** id -> label of the kinds of notification. */
    val KINDS = listOf(
        "dm" to "Direct messages",
        "channel" to "Channels",
        "mention" to "Mentions",
    )
    val TONES = listOf("ding" to "Ding", "chime" to "Chime", "pop" to "Pop", "drops" to "Drops", "bell" to "Bell", "low" to "Low")

    private fun toneUri(ctx: Context, id: String): Uri? {
        val res = ctx.resources.getIdentifier("tone_$id", "raw", ctx.packageName)
        return if (res == 0) null else Uri.parse("android.resource://${ctx.packageName}/$res")
    }

    /** The Uri of a spec, or null for the phone's default or silence. */
    fun uri(ctx: Context, spec: String): Uri? = when {
        spec.startsWith("tone:") -> toneUri(ctx, spec.removePrefix("tone:"))
        spec.startsWith("uri:") -> runCatching { Uri.parse(spec.removePrefix("uri:")) }.getOrNull()
        else -> null
    }

    /** A person-readable name of a spec. */
    fun label(ctx: Context, spec: String): String = when {
        spec.isEmpty() -> tr("Default")
        spec == "none" -> tr("Silent")
        spec.startsWith("tone:") -> TONES.firstOrNull { it.first == spec.removePrefix("tone:") }?.second?.let { tr(it) } ?: spec
        else -> uri(ctx, spec)?.let { u ->
            runCatching { RingtoneManager.getRingtone(ctx, u)?.getTitle(ctx) }.getOrNull()
                ?: runCatching { ctx.contentResolver.query(u, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { if (it.moveToFirst()) it.getString(0) else null } }.getOrNull()
                ?: tr("Chosen sound")
        } ?: spec
    }

    private var playing: Ringtone? = null

    /** Plays a spec once, so the person hears what they are choosing. */
    fun preview(ctx: Context, spec: String) {
        playing?.stop()
        val u = if (spec == "none") return else uri(ctx, spec) ?: RingtoneManager.getDefaultUri(RingtoneManager.TYPE_NOTIFICATION)
        playing = runCatching { RingtoneManager.getRingtone(ctx, u)?.also { it.audioAttributes = attrs(); it.play() } }.getOrNull()
    }

    private fun attrs() = AudioAttributes.Builder()
        .setUsage(AudioAttributes.USAGE_NOTIFICATION)
        .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION).build()

    /**
     * The notification channel for a kind with the person's chosen sound. The default sound uses
     * the plain "messages" channel, which Android's own settings can still change.
     */
    fun channelFor(ctx: Context, kind: String, spec: String): String {
        if (spec.isEmpty()) return Notify.CH_MESSAGES
        val id = "snd-$kind-" + spec.hashCode().toUInt().toString(16)
        val nm = ctx.getSystemService(NotificationManager::class.java)
        if (nm.getNotificationChannel(id) == null) {
            val label = KINDS.firstOrNull { it.first == kind }?.second ?: kind
            val ch = NotificationChannel(id, tr(label), NotificationManager.IMPORTANCE_HIGH)
            ch.description = tr("Sound for {0}", tr(label).lowercase())
            if (spec == "none") ch.setSound(null, null) else ch.setSound(uri(ctx, spec), attrs())
            nm.createNotificationChannel(ch)
            // The sound of a channel cannot be changed: drop the channels of earlier choices.
            nm.notificationChannels.filter { it.id.startsWith("snd-$kind-") && it.id != id }.forEach { nm.deleteNotificationChannel(it.id) }
        }
        return id
    }

    /**
     * Copies a sound file the person picked into the phone's Notifications folder, so Android's
     * notification service can read it, and returns its spec. Null if it cannot be saved.
     */
    fun saveFile(ctx: Context, src: Uri): String? {
        if (Build.VERSION.SDK_INT < 29) return "uri:$src"
        return runCatching {
            val name = ctx.contentResolver.query(src, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { if (it.moveToFirst()) it.getString(0) else null } ?: "maarse-sound"
            val values = ContentValues().apply {
                put(MediaStore.Audio.Media.DISPLAY_NAME, "Maarse " + name)
                put(MediaStore.Audio.Media.MIME_TYPE, ctx.contentResolver.getType(src) ?: "audio/mpeg")
                put(MediaStore.Audio.Media.RELATIVE_PATH, "Notifications/Maarse")
                put(MediaStore.Audio.Media.IS_NOTIFICATION, 1)
            }
            val dst = ctx.contentResolver.insert(MediaStore.Audio.Media.EXTERNAL_CONTENT_URI, values) ?: return null
            ctx.contentResolver.openInputStream(src)!!.use { i -> ctx.contentResolver.openOutputStream(dst)!!.use { o -> i.copyTo(o) } }
            "uri:$dst"
        }.getOrNull()
    }
}

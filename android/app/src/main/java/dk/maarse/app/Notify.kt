package dk.maarse.app

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build

object Notify {
    const val CH_SERVICE = "service"
    const val CH_MESSAGES = "messages"
    const val CH_ALERTS = "alerts"
    const val CH_ALERTS_QUIET = "alerts-quiet"

    fun channels(ctx: Context) {
        val nm = ctx.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(NotificationChannel(CH_SERVICE, tr("Connection"), NotificationManager.IMPORTANCE_MIN).apply {
            description = tr("Shown while Må:rse keeps the connection to the node")
        })
        nm.createNotificationChannel(NotificationChannel(CH_MESSAGES, tr("Messages"), NotificationManager.IMPORTANCE_HIGH))
        nm.createNotificationChannel(NotificationChannel(CH_ALERTS, tr("Alerts (actual, severe)"), NotificationManager.IMPORTANCE_HIGH).apply {
            description = tr("Signed alerts from trusted issuers: severe or extreme, not a test")
            enableVibration(true)
            vibrationPattern = longArrayOf(0, 600, 300, 600, 300, 600)
        })
        nm.createNotificationChannel(NotificationChannel(CH_ALERTS_QUIET, tr("Alerts (other)"), NotificationManager.IMPORTANCE_DEFAULT).apply {
            description = tr("Minor alerts, exercises, tests and cancellations")
        })
    }

    private fun allowed(ctx: Context) = Build.VERSION.SDK_INT < 33 ||
        ctx.checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

    private fun open(ctx: Context, conv: String?): PendingIntent {
        val i = Intent(ctx, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP)
        if (conv != null) i.putExtra("conv", conv)
        return PendingIntent.getActivity(ctx, conv?.hashCode() ?: 0, i, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }

    /**
     * `kind` picks the sound the person chose for it (see [Sounds]): "dm", "channel" or "mention".
     * Everything that comes from MeshCore (conversation ids start with "mc:") uses the separate
     * "meshcore" tone, so the person can hear which network called.
     */
    fun message(ctx: Context, conv: String, title: String, text: String, kind: String = "dm") {
        if (!allowed(ctx)) return
        val tone = if (conv.startsWith("mc:")) "meshcore" else kind
        val n = Notification.Builder(ctx, Sounds.channelFor(ctx, tone, Node.prefs.sound(tone)))
            .setSmallIcon(R.drawable.ic_stat).setContentTitle(title).setContentText(text)
            .setStyle(Notification.BigTextStyle().bigText(text))
            .setContentIntent(open(ctx, conv)).setAutoCancel(true).setCategory(Notification.CATEGORY_MESSAGE).build()
        ctx.getSystemService(NotificationManager::class.java).notify(conv, 0, n)
    }

    fun cancel(ctx: Context, conv: String) {
        ctx.getSystemService(NotificationManager::class.java).cancel(conv, 0)
    }

    fun alert(ctx: Context, a: Ev.Alert, what: String, loud: Boolean) {
        if (!allowed(ctx)) return
        val from = if (a.name.isNotEmpty()) a.name else tr("trusted key") + " " + shortAddr(a.issuer)
        val n = Notification.Builder(ctx, if (loud) CH_ALERTS else CH_ALERTS_QUIET)
            .setSmallIcon(R.drawable.ic_stat).setContentTitle(tr(what))
            .setContentText(a.text).setStyle(Notification.BigTextStyle().bigText("${a.text}\n\n" + tr("From {0}", from)))
            .setContentIntent(open(ctx, "alerts")).setAutoCancel(true).setCategory(Notification.CATEGORY_ALARM).build()
        ctx.getSystemService(NotificationManager::class.java).notify("alert", a.key.hashCode(), n)
    }
}

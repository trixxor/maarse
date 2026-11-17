package dk.maarse.app

import android.app.Application
import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder

class MaarseApp : Application() {
    override fun onCreate() {
        super.onCreate()
        // The host process (":host") only runs maarse-host; no UI state there.
        if (getProcessName().endsWith(":host")) return
        Node.init(this)
        if (Node.prefs.running) Node.start(Node.prefs.mode)
    }
}

/**
 * Keeps the app process (and with it the Bluetooth or WebSocket connection)
 * alive while the screen is off, so messages and alerts still arrive.
 */
class NodeService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        val n = Notification.Builder(this, Notify.CH_SERVICE)
            .setContentTitle("Må:rse is connected")
            .setContentText(Node.prefs.mode.title)
            .setSmallIcon(R.drawable.ic_stat).setContentIntent(open).setOngoing(true).build()
        if (Build.VERSION.SDK_INT >= 34) startForeground(1, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        else startForeground(1, n)
        return START_STICKY
    }

    companion object {
        fun start(ctx: Context) {
            runCatching { ctx.startForegroundService(Intent(ctx, NodeService::class.java)) }
        }
        fun stop(ctx: Context) { ctx.stopService(Intent(ctx, NodeService::class.java)) }
    }
}

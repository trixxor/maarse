package dk.maarse.app

import android.app.Application
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
        I18n.init(this)
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
        Notify.channels(this)
        val mode = Node.prefs.mode
        val n = Notify.service(this, when (mode) { Mode.PHONE -> false; Mode.SERVER -> true; else -> null }, mode.title)
        if (Build.VERSION.SDK_INT >= 34) startForeground(Notify.SERVICE_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        else startForeground(Notify.SERVICE_ID, n)
        return START_STICKY
    }

    companion object {
        fun start(ctx: Context) {
            runCatching { ctx.startForegroundService(Intent(ctx, NodeService::class.java)) }
        }
        fun stop(ctx: Context) { ctx.stopService(Intent(ctx, NodeService::class.java)) }
    }
}

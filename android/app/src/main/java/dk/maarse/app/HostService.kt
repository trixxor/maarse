package dk.maarse.app

import android.app.Notification
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.BatteryManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import java.io.File

/**
 * Runs maarse-host (Rust, via JNI) in a process of its own (":host" in the
 * manifest). The host never returns and may end its process on a fatal
 * error or a settings restart; stopping the node is ending this process.
 */
class HostService : Service() {
    private val main = Handler(Looper.getMainLooper())
    private var server = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            stopForeground(STOP_FOREGROUND_REMOVE)
            android.os.Process.killProcess(android.os.Process.myPid())
            return START_NOT_STICKY
        }
        Notify.channels(this)
        server = intent?.getBooleanExtra(EXTRA_SERVER, false) ?: server
        val n = notification(if (server) "Server running" else "Phone node running", if (server) "Backbone node and post office" else "Own Må:rse node on the backbone")
        if (Build.VERSION.SDK_INT >= 34) startForeground(NOTE_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE)
        else startForeground(NOTE_ID, n)
        if (!started) {
            started = true
            System.loadLibrary("maarse_android")
            val args = HostFiles.args(this, server)
            Thread({ runHost(args.toTypedArray(), server) }, "maarse-host").start()
            if (server) main.postDelayed(::checkServerConditions, 30_000)
        }
        return START_STICKY
    }

    /** Server mode only on Wi-Fi with power: on mobile data it cannot be reached (CGNAT) and drains the battery. */
    private fun checkServerConditions() {
        if (!HostFiles.serverConditionsMet(this)) {
            HostFiles.lastServerStop(this, "Stopped: the phone left Wi-Fi or the charger")
            stopForeground(STOP_FOREGROUND_REMOVE)
            android.os.Process.killProcess(android.os.Process.myPid())
            return
        }
        main.postDelayed(::checkServerConditions, 30_000)
    }

    private fun notification(title: String, text: String): Notification {
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        return Notification.Builder(this, Notify.CH_SERVICE)
            .setContentTitle(title).setContentText(text).setSmallIcon(R.drawable.ic_stat)
            .setContentIntent(open).setOngoing(true).build()
    }

    companion object {
        const val ACTION_STOP = "dk.maarse.app.STOP_HOST"
        const val EXTRA_SERVER = "server"
        private const val NOTE_ID = 2
        @Volatile private var started = false

        @JvmStatic external fun runHost(args: Array<String>, server: Boolean)

        fun start(ctx: Context, server: Boolean) {
            val i = Intent(ctx, HostService::class.java).putExtra(EXTRA_SERVER, server)
            runCatching { ctx.startForegroundService(i) }
        }

        fun stop(ctx: Context) {
            ctx.startService(Intent(ctx, HostService::class.java).setAction(ACTION_STOP))
        }
    }
}

/** The host's files in the app's private folder: data, access key and settings.txt. */
object HostFiles {
    /** The phone host's web/WebSocket port, on 127.0.0.1 only. */
    const val PORT = 47080

    fun dataDir(ctx: Context, server: Boolean) = File(ctx.filesDir, if (server) "maarse-server-data" else "maarse-data")

    fun accessKey(ctx: Context): String? =
        File(dataDir(ctx, false), "access-key.txt").takeIf { it.exists() }?.readText()?.trim()?.takeIf { it.length >= 16 }

    fun adminPassword(ctx: Context): String? =
        File(dataDir(ctx, true), "admin-password.txt").takeIf { it.exists() }?.readText()?.trim()

    fun settingsFile(ctx: Context, server: Boolean) = File(dataDir(ctx, server), "settings.txt")

    fun readSettings(ctx: Context, server: Boolean): String =
        settingsFile(ctx, server).takeIf { it.exists() }?.readText() ?: defaults(server)

    fun writeSettings(ctx: Context, server: Boolean, text: String) {
        dataDir(ctx, server).mkdirs()
        settingsFile(ctx, server).writeText(text.lines().joinToString("\n") { it.trimEnd() }.trim() + "\n")
    }

    /** First start: join the open backbone, and do not listen (mobile networks cannot be reached anyway). */
    private fun defaults(server: Boolean) = if (server) "--public\n" else "--public\n--listen off\n"

    /** The fixed part of the command line; settings.txt holds the rest. */
    fun args(ctx: Context, server: Boolean): List<String> {
        val dir = dataDir(ctx, server)
        dir.mkdirs()
        if (!settingsFile(ctx, server).exists()) writeSettings(ctx, server, defaults(server))
        val a = mutableListOf("--data", dir.path, "--service", "--no-browser")
        // The phone node's UI connects over 127.0.0.1, which every app on the
        // phone can reach: always ask for the key. The server keeps its own
        // dashboard (admin login) on port 8080.
        if (!server) a += listOf("--http", "127.0.0.1:$PORT", "--require-key")
        // The radio over Bluetooth as this node's LoRa modem (ModemBridge).
        if (!server && File(dir, "modem-bridge.txt").exists()) a += listOf("--modem-tcp", "127.0.0.1:${ModemBridge.PORT}")
        return a
    }

    fun serverConditionsMet(ctx: Context): Boolean {
        val cm = ctx.getSystemService(ConnectivityManager::class.java)
        val caps = cm.getNetworkCapabilities(cm.activeNetwork)
        val wifi = caps?.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) == true || caps?.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) == true
        val bat = ctx.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
        val plugged = (bat?.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0) ?: 0) != 0
        return wifi && plugged
    }

    // A file, not SharedPreferences: written by the host process, read by the app's.
    fun lastServerStop(ctx: Context, why: String) = File(ctx.filesDir, "server-stop.txt").writeText(why)

    fun takeServerStop(ctx: Context): String? {
        val f = File(ctx.filesDir, "server-stop.txt")
        return f.takeIf { it.exists() }?.readText()?.also { f.delete() }
    }
}

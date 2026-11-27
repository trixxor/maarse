package dk.maarse.app

import android.annotation.SuppressLint
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCallback
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattDescriptor
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.content.Context
import android.os.Build
import android.os.Handler
import android.os.Looper
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okio.ByteString
import okio.ByteString.Companion.toByteString
import java.util.ArrayDeque
import java.util.concurrent.TimeUnit

/** A connection to a node that speaks the companion protocol. */
interface Link {
    val name: String
    /** Sends one whole frame (length prefix included). */
    fun send(frame: ByteArray)
    fun close()
}

interface LinkListener {
    fun onOpen(link: Link)
    fun onFrame(link: Link, frame: ByteArray)
    fun onClosed(link: Link, why: String)
}

/**
 * Why a Bluetooth connection ended. Status 5 or 137 from a paired radio: it no longer knows this phone (a new code
 * makes it forget the paired ones), and Android keeps trying the old pairing until the person forgets the radio.
 */
fun bleLostText(name: String, status: Int, paired: Boolean): String =
    if (paired && (status == 5 || status == 137)) "The radio no longer accepts this phone's pairing (was its Bluetooth code changed?). " +
        "Forget $name in the phone's Bluetooth settings, then connect again"
    else "Bluetooth connection lost (status $status)"

/** Is a connection attempt that began `sinceMs` ago still worth waiting for, rather than starting another? */
fun attemptStillFresh(connecting: Boolean, sinceMs: Long): Boolean = connecting && sinceMs < 10_000

/** Is this a random static Bluetooth address (the two highest bits set), as every Må:rse radio uses? */
fun isRandomStatic(address: String): Boolean = (address.substringBefore(':').toIntOrNull(16) ?: 0) >= 0xC0

/** A radio node over Bluetooth LE (Nordic UART Service). */
@SuppressLint("MissingPermission")
class BleLink(ctx: Context, private val address: String, private val l: LinkListener, private val raw: Boolean = false) : Link {
    private val main = Handler(Looper.getMainLooper())
    private val decoder = FrameDecoder()
    private val out = ArrayDeque<ByteArray>()
    private var writing = false
    private var chunk = 20
    private var gatt: BluetoothGatt? = null
    private var rx: BluetoothGattCharacteristic? = null
    private var closed = false
    private var pairTries = 0
    override var name = "Bluetooth"; private set
    // Android sometimes reports nothing at all about an attempt; without this it would be waited for forever.
    private val noAnswer = Runnable { gatt?.let { it.disconnect(); it.close() }; finish("The radio did not answer") }

    init {
        val adapter = ctx.getSystemService(BluetoothManager::class.java).adapter
        // Say which kind of address it is: once the radio is forgotten, Android no longer knows and guesses "public".
        // (Before Android 13 it cannot be said; there, choosing the radio again from the list teaches Android.)
        val dev: BluetoothDevice = if (Build.VERSION.SDK_INT >= 33 && isRandomStatic(address))
            adapter.getRemoteLeDevice(address, BluetoothDevice.ADDRESS_TYPE_RANDOM) else adapter.getRemoteDevice(address)
        name = "Bluetooth: " + (dev.name ?: address)
        gatt = dev.connectGatt(ctx, false, object : BluetoothGattCallback() {
            override fun onConnectionStateChange(g: BluetoothGatt, status: Int, state: Int) {
                main.removeCallbacks(noAnswer)
                if (state == BluetoothProfile.STATE_CONNECTED) g.requestMtu(247)
                else if (state == BluetoothProfile.STATE_DISCONNECTED) {
                    val why = bleLostText(g.device.name ?: address, status, g.device.bondState == BluetoothDevice.BOND_BONDED)
                    g.close()
                    main.post { finish(why) }
                }
            }

            override fun onMtuChanged(g: BluetoothGatt, mtu: Int, status: Int) {
                if (status == BluetoothGatt.GATT_SUCCESS) chunk = maxOf(20, mtu - 3)
                g.discoverServices()
            }

            override fun onServicesDiscovered(g: BluetoothGatt, status: Int) {
                val s = g.getService(Nus.SERVICE)
                if (s == null) { g.disconnect(); main.post { finish("The node has no Nordic UART service") }; return }
                rx = s.getCharacteristic(Nus.RX)
                val tx = s.getCharacteristic(Nus.TX)
                g.setCharacteristicNotification(tx, true)
                subscribe(g, tx.getDescriptor(Nus.CCCD))
            }

            override fun onDescriptorWrite(g: BluetoothGatt, d: BluetoothGattDescriptor, status: Int) {
                // 5, 15 and 137: the radio has a Bluetooth code and wants this phone paired first.
                if (status != 5 && status != 15 && status != 137) main.post { l.onOpen(this@BleLink) }
                else main.post { pairThenRetry(g, d) }
            }

            override fun onCharacteristicChanged(g: BluetoothGatt, c: BluetoothGattCharacteristic, value: ByteArray) {
                main.post { deliver(value) }
            }

            @Deprecated("Android 12 and older")
            override fun onCharacteristicChanged(g: BluetoothGatt, c: BluetoothGattCharacteristic) {
                @Suppress("DEPRECATION") val v = c.value ?: return
                if (Build.VERSION.SDK_INT < 33) main.post { deliver(v) }
            }

            override fun onCharacteristicWrite(g: BluetoothGatt, c: BluetoothGattCharacteristic, status: Int) {
                main.post { writing = false; pump() }
            }
        }, BluetoothDevice.TRANSPORT_LE)
        main.postDelayed(noAnswer, 20_000)
    }

    private fun subscribe(g: BluetoothGatt, cccd: BluetoothGattDescriptor) {
        if (Build.VERSION.SDK_INT >= 33) g.writeDescriptor(cccd, BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE)
        else @Suppress("DEPRECATION") run { cccd.value = BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE; g.writeDescriptor(cccd) }
    }

    /** Android shows its own window for the code; try again while the person types it. */
    private fun pairThenRetry(g: BluetoothGatt, cccd: BluetoothGattDescriptor) {
        if (closed) return
        if (pairTries == 0 && g.device.bondState == BluetoothDevice.BOND_NONE) g.device.createBond()
        if (pairTries++ < 30) main.postDelayed({ if (!closed) subscribe(g, cccd) }, 2000)
        else { g.disconnect(); g.close(); finish("Pair with the radio using its Bluetooth code, then connect again") }
    }

    /** Raw (a modem bridge): the bytes as they come; otherwise whole frames. */
    private fun deliver(v: ByteArray) {
        if (raw) l.onFrame(this, v) else for (f in decoder.push(v)) l.onFrame(this, f)
    }

    override fun send(frame: ByteArray) {
        main.post {
            var i = 0
            while (i < frame.size) { out.add(frame.copyOfRange(i, minOf(frame.size, i + chunk))); i += chunk }
            pump()
        }
    }

    /** One write at a time (Android requires it). */
    private fun pump() {
        val g = gatt ?: return
        val c = rx ?: return
        if (writing || out.isEmpty()) return
        val next = out.poll() ?: return
        writing = true
        val ok = if (Build.VERSION.SDK_INT >= 33) {
            g.writeCharacteristic(c, next, BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT) == BluetoothGatt.GATT_SUCCESS
        } else @Suppress("DEPRECATION") run {
            c.writeType = BluetoothGattCharacteristic.WRITE_TYPE_DEFAULT; c.value = next; g.writeCharacteristic(c)
        }
        if (!ok) { writing = false; out.addFirst(next); main.postDelayed({ pump() }, 20) }
    }

    private fun finish(why: String) {
        if (closed) return
        closed = true
        main.removeCallbacks(noAnswer)
        gatt = null; rx = null; out.clear()
        l.onClosed(this, why)
    }

    override fun close() {
        main.post {
            gatt?.let { it.disconnect(); it.close() }
            finish("Disconnected")
        }
    }
}

/** maarse-host over WebSocket: the phone's own host, or a host elsewhere (VPN). */
class WsLink(url: String, override val name: String, private val l: LinkListener, token: String = "") : Link {
    private val main = Handler(Looper.getMainLooper())
    private var closed = false
    private val decoder = FrameDecoder()
    private val ws: WebSocket = client.newWebSocket(Request.Builder().url(url).apply {
        if (token.isNotEmpty()) header("Authorization", "Bearer $token")
    }.build(), object : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) { main.post { l.onOpen(this@WsLink) } }
        override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
            val b = bytes.toByteArray()
            main.post { if (!closed) for (f in decoder.push(b)) l.onFrame(this@WsLink, f) }
        }
        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) { main.post { finish("Closed ($code)") } }
        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            main.post { finish(if (response?.code == 403) "The host refused the access key" else (t.message ?: "Connection failed")) }
        }
    })

    override fun send(frame: ByteArray) { ws.send(frame.toByteString()) }
    private fun finish(why: String) { if (!closed) { closed = true; l.onClosed(this, why) } }
    override fun close() { ws.close(1000, null); main.post { finish("Disconnected") } }

    companion object {
        private val client = OkHttpClient.Builder().pingInterval(20, TimeUnit.SECONDS).readTimeout(0, TimeUnit.SECONDS).build()
    }
}

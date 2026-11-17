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
    override var name = "Bluetooth"; private set

    init {
        val dev: BluetoothDevice = ctx.getSystemService(BluetoothManager::class.java).adapter.getRemoteDevice(address)
        name = "Bluetooth: " + (dev.name ?: address)
        gatt = dev.connectGatt(ctx, false, object : BluetoothGattCallback() {
            override fun onConnectionStateChange(g: BluetoothGatt, status: Int, state: Int) {
                if (state == BluetoothProfile.STATE_CONNECTED) g.requestMtu(247)
                else if (state == BluetoothProfile.STATE_DISCONNECTED) {
                    g.close()
                    main.post { finish("Bluetooth connection lost (status $status)") }
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
                val cccd = tx.getDescriptor(Nus.CCCD)
                if (Build.VERSION.SDK_INT >= 33) g.writeDescriptor(cccd, BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE)
                else @Suppress("DEPRECATION") run { cccd.value = BluetoothGattDescriptor.ENABLE_NOTIFICATION_VALUE; g.writeDescriptor(cccd) }
            }

            override fun onDescriptorWrite(g: BluetoothGatt, d: BluetoothGattDescriptor, status: Int) {
                main.post { l.onOpen(this@BleLink) }
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
class WsLink(private val url: String, override val name: String, private val l: LinkListener) : Link {
    private val main = Handler(Looper.getMainLooper())
    private var closed = false
    private val decoder = FrameDecoder()
    private val ws: WebSocket = client.newWebSocket(Request.Builder().url(url).build(), object : WebSocketListener() {
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

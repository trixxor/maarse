package dk.maarse.app

import android.content.Context
import java.io.File
import java.io.InputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.security.SecureRandom

/**
 * The phone node's LoRa radio over Bluetooth: maarse-host (in the `:host`
 * process) connects to 127.0.0.1:[PORT] with `--modem-tcp` and speaks the
 * serial modem protocol; this bridge passes the bytes to and from the radio
 * over Bluetooth (Nordic UART). The radio then only sends and receives; the
 * phone's host does the routing, the account, the post offices.
 *
 * Other apps can reach 127.0.0.1 too, so the host must first send the key
 * from `modem-tcp-key.txt` in its data folder.
 */
object ModemBridge {
    const val PORT = 47081
    private var server: ServerSocket? = null
    @Volatile private var running = false

    /** Turns the bridge on for the host's next start (writes key and flag). */
    fun configure(ctx: Context, on: Boolean) {
        val dir = HostFiles.dataDir(ctx, false).apply { mkdirs() }
        val flag = File(dir, "modem-bridge.txt")
        if (on) {
            val key = ByteArray(16).also { SecureRandom().nextBytes(it) }.hex()
            File(dir, "modem-tcp-key.txt").writeText(key)
            flag.writeText("on\n")
        } else {
            flag.delete()
            File(dir, "modem-tcp-key.txt").delete()
        }
    }

    fun enabled(ctx: Context) = File(HostFiles.dataDir(ctx, false), "modem-bridge.txt").exists()

    fun start(ctx: Context, radio: String) {
        if (running) return
        val key = File(HostFiles.dataDir(ctx, false), "modem-tcp-key.txt").readTextOrNull()?.trim()
        if (key.isNullOrEmpty()) {
            // The host started without a key: make one and restart the host so both use it.
            Node.note("Radio bridge: no key yet, making one and restarting the node")
            configure(ctx, true)
            Node.restartHost()
            return
        }
        running = true
        Thread({
            try {
                // IPv4 on purpose: the host dials 127.0.0.1, and getLoopbackAddress() may be ::1.
                ServerSocket(PORT, 1, InetAddress.getByName("127.0.0.1")).use { srv ->
                    server = srv
                    while (running) {
                        val s = runCatching { srv.accept() }.getOrNull() ?: break
                        // One broken host connection must not end the bridge.
                        runCatching { serve(ctx.applicationContext, s, key, radio) }
                            .onFailure { Node.post { Node.note("Radio bridge: host connection lost (${it.message})") } }
                    }
                }
            } catch (e: Exception) {
                Node.post { Node.note("Radio bridge stopped: ${e.message}") }
            }
            running = false
        }, "modem-bridge").start()
    }

    val isRunning get() = running

    fun stop() {
        running = false
        runCatching { server?.close() }
        server = null
    }

    private fun readLine(i: InputStream): String {
        val b = StringBuilder()
        while (b.length < 100) {
            val c = i.read()
            if (c < 0 || c == '\n'.code) break
            b.append(c.toChar())
        }
        return b.toString()
    }

    /** One host connection: check the key, then pass bytes both ways until either side goes. */
    private fun serve(ctx: Context, s: Socket, key: String, radio: String) {
        s.use { sock ->
            sock.tcpNoDelay = true
            val input = sock.getInputStream()
            if (readLine(input).trim() != "KEY $key") return
            val out = sock.getOutputStream()
            val done = java.util.concurrent.CountDownLatch(1)
            // Radio data arrives on the main thread, where Android forbids socket writes.
            val writer = java.util.concurrent.Executors.newSingleThreadExecutor()
            var link: BleLink? = null
            // Only watched for the radio's answer about its Bluetooth code.
            val frames = FrameDecoder()
            var asked = false
            val listener = object : LinkListener {
                override fun onOpen(link: Link) {
                    Node.note("Radio connected as the phone node's LoRa modem")
                    link.send(Cmd.getBlePin())
                }
                override fun onFrame(link: Link, frame: ByteArray) {
                    for (ev in frames.push(frame).map(::decodeEvent).filterIsInstance<Ev.BlePin>()) {
                        if (!ev.own) {
                            // With the factory code the radio is no modem.
                            asked = true
                            Node.note("The radio still has the factory Bluetooth code: choose your own")
                            Node.codeNeeded.value = { pin -> link.send(Cmd.setBlePin(pin)) }
                        } else if (asked) {
                            asked = false
                            Node.codeNeeded.value = null
                            // The host connects again and sets the radio up as its modem.
                            done.countDown()
                        }
                    }
                    writer.execute { runCatching { out.write(frame); out.flush() }.onFailure { done.countDown() } }
                }
                override fun onClosed(link: Link, why: String) {
                    Node.note("Radio (modem): $why")
                    done.countDown()
                }
            }
            Node.post { link = runCatching { BleLink(ctx, radio, listener, raw = true) }.getOrNull().also { if (it == null) done.countDown() } }
            val reader = Thread({
                val buf = ByteArray(512)
                try {
                    while (true) {
                        val n = input.read(buf)
                        if (n < 0) break
                        val l = link ?: run { Thread.sleep(20); link } ?: continue
                        l.send(buf.copyOf(n))
                    }
                } catch (_: Exception) {
                }
                done.countDown()
            }, "modem-bridge-in")
            reader.start()
            done.await()
            writer.shutdownNow()
            Node.post { link?.close() }
        }
    }
}

private fun File.readTextOrNull() = runCatching { readText() }.getOrNull()

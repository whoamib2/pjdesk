package us.spader.pjdesk.core

import android.content.Context
import android.net.wifi.WifiManager
import android.os.Handler
import android.os.Looper
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetSocketAddress
import java.net.SocketTimeoutException
import java.util.concurrent.atomic.AtomicBoolean

private const val DISCOVERY_PORT = 21202
private const val HEARD_MS = 12_000L

/**
 * Listens for PJDesk computers on the local network. The beacon carries a name
 * and an ID, never a password. Peers drop off after 12 seconds of silence.
 */
class LanListen(
    context: Context,
    private val onSights: (List<LanSight>) -> Unit,
) {
    private val app = context.applicationContext
    private val running = AtomicBoolean(false)
    private val main = Handler(Looper.getMainLooper())
    private val heard = LinkedHashMap<String, LanSight>()
    private var thread: Thread? = null
    private var socket: DatagramSocket? = null
    private var lock: WifiManager.MulticastLock? = null

    fun start() {
        if (!running.compareAndSet(false, true)) return
        try {
            val wifi = app.getSystemService(Context.WIFI_SERVICE) as? WifiManager
            lock = wifi?.createMulticastLock("pjdesk-lan")?.apply {
                setReferenceCounted(false)
                acquire()
            }
        } catch (_: RuntimeException) {
            lock = null
        }
        thread = Thread({ loop() }, "pjdesk-lan").also { it.isDaemon = true; it.start() }
    }

    fun stop() {
        running.set(false)
        try {
            socket?.close()
        } catch (_: RuntimeException) {
        }
        thread?.join(500)
        thread = null
        try {
            if (lock?.isHeld == true) lock?.release()
        } catch (_: RuntimeException) {
        }
        lock = null
    }

    private fun loop() {
        val udp = try {
            DatagramSocket(null).apply {
                reuseAddress = true
                broadcast = true
                bind(InetSocketAddress(DISCOVERY_PORT))
                soTimeout = 800
            }
        } catch (_: Exception) {
            running.set(false)
            return
        }
        socket = udp
        val buf = ByteArray(512)
        try {
            while (running.get()) {
                val packet = DatagramPacket(buf, buf.size)
                try {
                    udp.receive(packet)
                    val bytes = packet.data.copyOfRange(packet.offset, packet.offset + packet.length)
                    val body = decodeLanBeacon(bytes) ?: continue
                    val host = packet.address?.hostAddress ?: continue
                    if (!acceptLanHost(host)) continue
                    val id = body.id.filter { it.isDigit() }
                    if (id.length != 9) continue
                    synchronized(heard) {
                        heard[id] = LanSight(id, body.name.trim().take(64), host, body.port, System.currentTimeMillis())
                    }
                } catch (_: SocketTimeoutException) {
                } catch (_: Exception) {
                    if (!running.get()) break
                }
                publish()
            }
        } finally {
            try {
                udp.close()
            } catch (_: RuntimeException) {
            }
        }
    }

    private fun publish() {
        val now = System.currentTimeMillis()
        val list = synchronized(heard) {
            val stale = heard.filterValues { now - it.seenAt > HEARD_MS }.keys
            stale.forEach { heard.remove(it) }
            heard.values.toList()
        }
        main.post {
            if (running.get()) onSights(list)
        }
    }
}

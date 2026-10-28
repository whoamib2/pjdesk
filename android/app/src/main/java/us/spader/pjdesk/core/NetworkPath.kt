package us.spader.pjdesk.core

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.wifi.WifiInfo
import android.net.wifi.WifiManager
import android.os.Build
import java.net.Inet4Address

/**
 * Names the network this phone is on, so a LAN address that worked here can be
 * tried first next time. The gateway stays put when DHCP gives the phone a new
 * address. The Wi-Fi name is used when the system will share it.
 */
object NetworkPath {
    /** IPv4 addresses on the network this phone is using. Loopback is left out. */
    fun ipv4s(context: Context): List<String> {
        val cm = context.applicationContext.getSystemService(ConnectivityManager::class.java) ?: return emptyList()
        val props = cm.getLinkProperties(cm.activeNetwork ?: return emptyList()) ?: return emptyList()
        val out = ArrayList<String>()
        for (link in props.linkAddresses) {
            val addr = link.address
            if (addr !is Inet4Address || addr.isLoopbackAddress || addr.isAnyLocalAddress) continue
            val text = addr.hostAddress ?: continue
            if (text.isNotBlank() && !out.contains(text)) out.add(text)
        }
        return out
    }

    fun key(context: Context): String {
        val cm = context.applicationContext.getSystemService(ConnectivityManager::class.java) ?: return ""
        val network = cm.activeNetwork ?: return ""
        val props = cm.getLinkProperties(network)
        val gateway = props?.routes
            ?.firstOrNull { it.isDefaultRoute && it.gateway != null && !it.gateway!!.isAnyLocalAddress }
            ?.gateway
            ?.hostAddress
            .orEmpty()
        val ssid = ssid(context, cm)
        return when {
            gateway.isNotBlank() && ssid.isNotBlank() -> "gw:$gateway|$ssid"
            gateway.isNotBlank() -> "gw:$gateway"
            ssid.isNotBlank() -> "ssid:$ssid"
            else -> ""
        }
    }

    private fun ssid(context: Context, cm: ConnectivityManager): String {
        val network = cm.activeNetwork ?: return ""
        val caps = cm.getNetworkCapabilities(network) ?: return ""
        if (!caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)) return ""
        val raw = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            (caps.transportInfo as? WifiInfo)?.ssid
        } else {
            @Suppress("DEPRECATION")
            context.applicationContext.getSystemService(WifiManager::class.java)?.connectionInfo?.ssid
        } ?: return ""
        val name = raw.trim().removePrefix("\"").removeSuffix("\"")
        if (name.isBlank() || name == "<unknown ssid>" || name == "0x") return ""
        return name
    }
}

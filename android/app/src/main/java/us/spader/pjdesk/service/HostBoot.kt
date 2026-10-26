package us.spader.pjdesk.service

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import us.spader.pjdesk.AppLog

/** Starts the phone host after boot and after this app is updated. */
class HostBoot : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent?) {
        val action = intent?.action.orEmpty()
        if (action != Intent.ACTION_BOOT_COMPLETED && action != Intent.ACTION_MY_PACKAGE_REPLACED) return
        AppLog.i("starting the host after $action")
        runCatching { HostService.arm(context) }
            .onFailure { AppLog.i("could not start the host after $action: ${it.message}") }
    }
}

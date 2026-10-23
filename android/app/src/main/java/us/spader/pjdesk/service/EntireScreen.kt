package us.spader.pjdesk.service

import android.content.Context
import android.content.Intent
import android.media.projection.MediaProjectionConfig
import android.media.projection.MediaProjectionManager
import android.os.Build
import us.spader.pjdesk.AppLog

/**
 * Every capture asks for the whole display. On Android 14 and later
 * [MediaProjectionConfig.createConfigForDefaultDisplay] limits the system
 * dialog to that display, so a single app is not the default.
 */
object EntireScreen {
    fun captureIntent(context: Context): Intent {
        val manager = context.getSystemService(MediaProjectionManager::class.java)
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            AppLog.i("screen capture intent for the default display")
            manager.createScreenCaptureIntent(MediaProjectionConfig.createConfigForDefaultDisplay())
        } else {
            manager.createScreenCaptureIntent()
        }
    }
}

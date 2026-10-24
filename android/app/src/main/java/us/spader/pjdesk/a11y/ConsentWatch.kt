package us.spader.pjdesk.a11y

import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import us.spader.pjdesk.AppLog
import us.spader.pjdesk.service.RemoteControl

/**
 * Whether this poll may tap. A locked phone and a missing accessibility
 * service keep waiting. The watch outlives the activity that opened the dialog.
 */
object ConsentWatch {
    const val POLL_MS = 400L

    /** About 50 seconds, inside the minute the PC waits for screen sharing. */
    const val POLL_LIMIT = 125

    data class ClickResult(val action: Boolean, val gesture: Boolean, val bounds: String)

    data class Decision(
        val click: Boolean,
        val again: Boolean,
        val giveUp: Boolean,
        val note: String?,
    )

    fun decide(
        bound: Boolean,
        locked: Boolean,
        polls: Int,
        limit: Int = POLL_LIMIT,
        elapsedMs: Long = polls.toLong() * POLL_MS,
    ): Decision {
        // The deadline is wall time. A system-window event can poll early, and
        // that must not burn the 50 seconds.
        if (elapsedMs >= limit.toLong() * POLL_MS) {
            val note = when {
                locked -> "keyguard is locked; the screen share dialog cannot be tapped"
                !bound -> "accessibility service is not bound; the screen share dialog was not tapped"
                else -> "auto-accept could not click the screen share dialog"
            }
            return Decision(click = false, again = false, giveUp = true, note = note)
        }
        if (!bound) {
            return Decision(
                click = false,
                again = true,
                giveUp = false,
                note = "consent watch waiting; accessibility service is not bound",
            )
        }
        if (locked) {
            return Decision(
                click = false,
                again = true,
                giveUp = false,
                note = "keyguard is locked; the screen share dialog cannot be tapped until the phone is unlocked",
            )
        }
        return Decision(click = true, again = true, giveUp = false, note = null)
    }
}

/**
 * Polls the current accessibility service. A new activity, or a service that
 * was destroyed and bound again, does not cancel the watch.
 */
object ConsentWatcher {
    private val main = Handler(Looper.getMainLooper())
    private var polls = 0
    private var onGiveUp: (() -> Unit)? = null
    private var lastDialog = ""
    private var lastNote = ""
    private var awaiting = false
    private var awaitingWhy = ""
    private var awaitingPolls = 0
    private var startedAt = 0L
    private var ticking = false
    private var pendingNudge = false
    private var lastNudge = 0L
    private val poll = Runnable { tick() }

    fun start(giveUp: () -> Unit) {
        polls = 0
        lastDialog = ""
        lastNote = ""
        awaiting = false
        awaitingWhy = ""
        awaitingPolls = 0
        startedAt = SystemClock.elapsedRealtime()
        pendingNudge = false
        lastNudge = 0L
        onGiveUp = giveUp
        main.removeCallbacks(poll)
        main.post(poll)
    }

    fun stop() {
        main.removeCallbacks(poll)
        ticking = false
        pendingNudge = false
        awaiting = false
        onGiveUp = null
    }

    /** A system window changed. Poll now, without resetting the 50 second deadline. */
    fun nudge() {
        if (!ProjectionConsent.armed) return
        val now = SystemClock.elapsedRealtime()
        if (now - lastNudge < 200L) return
        lastNudge = now
        if (ticking) {
            pendingNudge = true
            return
        }
        main.removeCallbacks(poll)
        main.post(poll)
    }

    fun gestureFinished(completed: Boolean, why: String) {
        awaiting = false
        awaitingWhy = ""
        awaitingPolls = 0
        AppLog.i("screen share tap completed=$completed why=$why")
        if (completed && why == "share" && ProjectionConsent.armed) {
            ProjectionConsent.markClicked()
            ProjectionConsent.stopWatching()
        }
    }

    private fun tick() {
        if (!ProjectionConsent.armed) return
        ticking = true
        polls += 1
        var again = true
        try {
            val service = RemoteControl.service
            val locked = service?.let { ScreenAlert.locked(it) } ?: false
            val elapsed = (SystemClock.elapsedRealtime() - startedAt).coerceAtLeast(0L)
            if (polls == 1 || polls % 12 == 0) {
                val id = service?.let { System.identityHashCode(it) } ?: 0
                AppLog.i("consent watch poll=$polls elapsed=${elapsed}ms bound=${service != null} keyguard=$locked service=$id")
            }
            if (awaiting) {
                awaitingPolls += 1
                if (awaitingPolls > 4) {
                    awaiting = false
                    awaitingWhy = ""
                    awaitingPolls = 0
                    AppLog.i("screen share tap was not confirmed")
                }
            }
            val decision = ConsentWatch.decide(service != null, locked, polls, elapsedMs = elapsed)
            again = decision.again && !decision.giveUp
            val note = decision.note
            if (note != null && note != lastNote) {
                lastNote = note
                AppLog.i(note)
            }
            if (service != null) {
                val nodes = service.collectConsent()
                if (polls == 1 || polls % 12 == 0) {
                    val system = nodes.count { it.packageName == ProjectionConsent.SYSTEM_UI }
                    AppLog.i("consent watch read nodes=${nodes.size} systemui=$system")
                }
                val step = ProjectionConsent.decide(nodes)
                logDialog(nodes, step)
                if (decision.click && !awaiting && step is ConsentStep.Click) {
                    val result = service.activate(step.index, nodes.getOrNull(step.index), step.why)
                    AppLog.i("screen share auto-click ${step.why} action=${result.action} gesture=${result.gesture} bounds=${result.bounds}")
                    if (result.action && step.why == "share") {
                        ProjectionConsent.markClicked()
                        ProjectionConsent.stopWatching()
                        again = false
                    } else if (result.gesture) {
                        awaiting = true
                        awaitingWhy = step.why
                        awaitingPolls = 0
                    }
                }
            }
            if (decision.giveUp || !decision.again) {
                again = false
                val giveUp = onGiveUp
                onGiveUp = null
                giveUp?.invoke()
            }
        } catch (error: Throwable) {
            AppLog.e("consent watch failed", error)
        } finally {
            ticking = false
        }
        if (again && ProjectionConsent.armed) {
            if (pendingNudge) {
                pendingNudge = false
                main.post(poll)
            } else {
                main.postDelayed(poll, ConsentWatch.POLL_MS)
            }
        }
    }

    private fun logDialog(nodes: List<ConsentNode>, step: ConsentStep) {
        if (!ProjectionConsent.looksLikeConsent(nodes)) return
        val mode = nodes.firstOrNull { ProjectionConsent.isModeControl(it) }
        val button = nodes.firstOrNull { it.viewId == "android:id/button1" }
        val line = "screen share dialog mode='${mode?.text.orEmpty()}' child='${mode?.subtree.orEmpty()}' button='${button?.text.orEmpty()}' step=$step"
        if (line == lastDialog) return
        lastDialog = line
        AppLog.i(line)
    }
}

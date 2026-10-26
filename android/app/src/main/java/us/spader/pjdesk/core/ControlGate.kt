package us.spader.pjdesk.core

/**
 * Publishes accessibility on and off to the session.
 * The first report is immediate. A later drop is held so a rebind that lasts
 * less than [holdOffMs] does not tell the PC that control turned off and on.
 */
class ControlGate(private val holdOffMs: Long = 2_000L) {
    private var published: Boolean? = null
    private var falseSince: Long? = null

    /** The value to send now, or null when the held drop is still waiting. */
    fun offer(on: Boolean, now: Long): Boolean? {
        if (on) {
            falseSince = null
            if (published == true) return null
            published = true
            return true
        }
        if (published != true) {
            falseSince = null
            if (published == false) return null
            published = false
            return false
        }
        if (falseSince == null) falseSince = now
        if (now - falseSince!! < holdOffMs) return null
        falseSince = null
        published = false
        return false
    }

    fun holdingDrop(): Boolean = falseSince != null

    fun remaining(now: Long): Long {
        val since = falseSince ?: return holdOffMs
        return (holdOffMs - (now - since)).coerceAtLeast(0L)
    }
}

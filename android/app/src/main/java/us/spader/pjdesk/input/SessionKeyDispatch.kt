package us.spader.pjdesk.input

/** The open session registers a handler so MainActivity can forward hardware keys. */
object SessionKeyDispatch {
    @Volatile
    var handler: ((scan: Int, down: Boolean) -> Boolean)? = null
}

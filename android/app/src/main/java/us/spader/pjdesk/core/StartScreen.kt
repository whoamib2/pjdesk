package us.spader.pjdesk.core

/** The screen PJDesk opens to. Saved computers unless the user chose Home. */
object StartScreen {
    const val SAVED = "saved"
    const val HOME = "home"
    val choices = listOf(SAVED to "Saved computers", HOME to "Home (connect and share)")

    fun normalize(value: String?): String = if (value == HOME) HOME else SAVED

    /** Bottom bar index: 0 Home, 1 Saved. */
    fun tab(value: String?): Int = if (normalize(value) == HOME) 0 else 1
}

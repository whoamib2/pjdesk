package us.spader.pjdesk.input

import android.view.InputDevice
import android.view.KeyEvent

/** Maps a hardware keyboard KeyEvent to a PC/AT set-1 scan code used by the host. */
object HardwareKeys {
    /** Side buttons and system navigation stay on this phone. */
    private val localOnly = setOf(
        KeyEvent.KEYCODE_BACK,
        KeyEvent.KEYCODE_HOME,
        KeyEvent.KEYCODE_APP_SWITCH,
        KeyEvent.KEYCODE_VOLUME_UP,
        KeyEvent.KEYCODE_VOLUME_DOWN,
        KeyEvent.KEYCODE_VOLUME_MUTE,
        KeyEvent.KEYCODE_POWER,
        KeyEvent.KEYCODE_CAMERA,
        KeyEvent.KEYCODE_STEM_PRIMARY,
    )

    fun shouldCapture(event: KeyEvent): Boolean {
        if (!captures(event.keyCode, event.action)) return false
        val sources = event.source
        val fromKeyboard = sources and InputDevice.SOURCE_KEYBOARD != 0
        val fromDpad = sources and InputDevice.SOURCE_DPAD != 0
        return fromKeyboard || fromDpad || scan(event.keyCode) != null
    }

    fun captures(keyCode: Int, action: Int): Boolean {
        if (keyCode in localOnly) return false
        if (action != KeyEvent.ACTION_DOWN && action != KeyEvent.ACTION_UP) return false
        return scan(keyCode) != null
    }

    fun scan(event: KeyEvent): Int? = scan(event.keyCode)

    fun scan(keyCode: Int): Int? = SCAN[keyCode]

    private const val EXT = 0xE000

    private val SCAN: Map<Int, Int> = hashMapOf(
        KeyEvent.KEYCODE_ESCAPE to 0x01,
        KeyEvent.KEYCODE_1 to 0x02,
        KeyEvent.KEYCODE_2 to 0x03,
        KeyEvent.KEYCODE_3 to 0x04,
        KeyEvent.KEYCODE_4 to 0x05,
        KeyEvent.KEYCODE_5 to 0x06,
        KeyEvent.KEYCODE_6 to 0x07,
        KeyEvent.KEYCODE_7 to 0x08,
        KeyEvent.KEYCODE_8 to 0x09,
        KeyEvent.KEYCODE_9 to 0x0A,
        KeyEvent.KEYCODE_0 to 0x0B,
        KeyEvent.KEYCODE_MINUS to 0x0C,
        KeyEvent.KEYCODE_EQUALS to 0x0D,
        KeyEvent.KEYCODE_DEL to 0x0E, // Backspace
        KeyEvent.KEYCODE_TAB to 0x0F,
        KeyEvent.KEYCODE_Q to 0x10,
        KeyEvent.KEYCODE_W to 0x11,
        KeyEvent.KEYCODE_E to 0x12,
        KeyEvent.KEYCODE_R to 0x13,
        KeyEvent.KEYCODE_T to 0x14,
        KeyEvent.KEYCODE_Y to 0x15,
        KeyEvent.KEYCODE_U to 0x16,
        KeyEvent.KEYCODE_I to 0x17,
        KeyEvent.KEYCODE_O to 0x18,
        KeyEvent.KEYCODE_P to 0x19,
        KeyEvent.KEYCODE_LEFT_BRACKET to 0x1A,
        KeyEvent.KEYCODE_RIGHT_BRACKET to 0x1B,
        KeyEvent.KEYCODE_ENTER to 0x1C,
        KeyEvent.KEYCODE_CTRL_LEFT to 0x1D,
        KeyEvent.KEYCODE_A to 0x1E,
        KeyEvent.KEYCODE_S to 0x1F,
        KeyEvent.KEYCODE_D to 0x20,
        KeyEvent.KEYCODE_F to 0x21,
        KeyEvent.KEYCODE_G to 0x22,
        KeyEvent.KEYCODE_H to 0x23,
        KeyEvent.KEYCODE_J to 0x24,
        KeyEvent.KEYCODE_K to 0x25,
        KeyEvent.KEYCODE_L to 0x26,
        KeyEvent.KEYCODE_SEMICOLON to 0x27,
        KeyEvent.KEYCODE_APOSTROPHE to 0x28,
        KeyEvent.KEYCODE_GRAVE to 0x29,
        KeyEvent.KEYCODE_SHIFT_LEFT to 0x2A,
        KeyEvent.KEYCODE_BACKSLASH to 0x2B,
        KeyEvent.KEYCODE_Z to 0x2C,
        KeyEvent.KEYCODE_X to 0x2D,
        KeyEvent.KEYCODE_C to 0x2E,
        KeyEvent.KEYCODE_V to 0x2F,
        KeyEvent.KEYCODE_B to 0x30,
        KeyEvent.KEYCODE_N to 0x31,
        KeyEvent.KEYCODE_M to 0x32,
        KeyEvent.KEYCODE_COMMA to 0x33,
        KeyEvent.KEYCODE_PERIOD to 0x34,
        KeyEvent.KEYCODE_SLASH to 0x35,
        KeyEvent.KEYCODE_SHIFT_RIGHT to 0x36,
        KeyEvent.KEYCODE_ALT_LEFT to 0x38,
        KeyEvent.KEYCODE_SPACE to 0x39,
        KeyEvent.KEYCODE_CAPS_LOCK to 0x3A,
        KeyEvent.KEYCODE_F1 to 0x3B,
        KeyEvent.KEYCODE_F2 to 0x3C,
        KeyEvent.KEYCODE_F3 to 0x3D,
        KeyEvent.KEYCODE_F4 to 0x3E,
        KeyEvent.KEYCODE_F5 to 0x3F,
        KeyEvent.KEYCODE_F6 to 0x40,
        KeyEvent.KEYCODE_F7 to 0x41,
        KeyEvent.KEYCODE_F8 to 0x42,
        KeyEvent.KEYCODE_F9 to 0x43,
        KeyEvent.KEYCODE_F10 to 0x44,
        KeyEvent.KEYCODE_NUM_LOCK to 0x45,
        KeyEvent.KEYCODE_SCROLL_LOCK to 0x46,
        KeyEvent.KEYCODE_F11 to 0x57,
        KeyEvent.KEYCODE_F12 to 0x58,
        KeyEvent.KEYCODE_CTRL_RIGHT to (EXT or 0x1D),
        KeyEvent.KEYCODE_ALT_RIGHT to (EXT or 0x38),
        KeyEvent.KEYCODE_SYSRQ to (EXT or 0x37), // PrintScreen
        KeyEvent.KEYCODE_INSERT to (EXT or 0x52),
        KeyEvent.KEYCODE_FORWARD_DEL to (EXT or 0x53),
        KeyEvent.KEYCODE_MOVE_HOME to (EXT or 0x47),
        KeyEvent.KEYCODE_MOVE_END to (EXT or 0x4F),
        KeyEvent.KEYCODE_PAGE_UP to (EXT or 0x49),
        KeyEvent.KEYCODE_PAGE_DOWN to (EXT or 0x51),
        KeyEvent.KEYCODE_DPAD_LEFT to (EXT or 0x4B),
        KeyEvent.KEYCODE_DPAD_RIGHT to (EXT or 0x4D),
        KeyEvent.KEYCODE_DPAD_UP to (EXT or 0x48),
        KeyEvent.KEYCODE_DPAD_DOWN to (EXT or 0x50),
        KeyEvent.KEYCODE_META_LEFT to (EXT or 0x5B),
        KeyEvent.KEYCODE_META_RIGHT to (EXT or 0x5C),
        KeyEvent.KEYCODE_MENU to (EXT or 0x5D),
        KeyEvent.KEYCODE_NUMPAD_ENTER to (EXT or 0x1C),
        KeyEvent.KEYCODE_NUMPAD_0 to 0x52,
        KeyEvent.KEYCODE_NUMPAD_1 to 0x4F,
        KeyEvent.KEYCODE_NUMPAD_2 to 0x50,
        KeyEvent.KEYCODE_NUMPAD_3 to 0x51,
        KeyEvent.KEYCODE_NUMPAD_4 to 0x4B,
        KeyEvent.KEYCODE_NUMPAD_5 to 0x4C,
        KeyEvent.KEYCODE_NUMPAD_6 to 0x4D,
        KeyEvent.KEYCODE_NUMPAD_7 to 0x47,
        KeyEvent.KEYCODE_NUMPAD_8 to 0x48,
        KeyEvent.KEYCODE_NUMPAD_9 to 0x49,
        KeyEvent.KEYCODE_BREAK to (EXT or 0x46), // Pause
    )
}

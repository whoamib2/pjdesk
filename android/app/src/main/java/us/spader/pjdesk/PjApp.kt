package us.spader.pjdesk

import android.app.Application
import us.spader.pjdesk.core.Engine
import us.spader.pjdesk.data.Store

class PjApp : Application() {
    lateinit var engine: Engine
        private set
    lateinit var store: Store
        private set

    override fun onCreate() {
        super.onCreate()
        store = Store(this)
        engine = Engine(this)
        engine.start()
    }
}

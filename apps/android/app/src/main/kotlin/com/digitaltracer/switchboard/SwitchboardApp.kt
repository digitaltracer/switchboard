package com.digitaltracer.switchboard

import android.app.Application

class SwitchboardApp : Application() {
    lateinit var model: AppModel
        private set
    lateinit var voice: Voice
        private set

    override fun onCreate() {
        super.onCreate()
        model = AppModel(this, KeystoreKeys(this))
        if (Push.start(this)) Push.token(model::registerPush)
        voice = Voice(this) { model.vocabulary }
    }
}

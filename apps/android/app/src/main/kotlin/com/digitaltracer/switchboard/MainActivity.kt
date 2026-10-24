package com.digitaltracer.switchboard

import android.content.Intent
import android.os.Bundle
import android.graphics.Color
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.LaunchedEffect

class MainActivity : ComponentActivity() {
    private val model get() = (application as SwitchboardApp).model

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        if (savedInstanceState == null) opened(intent)
        setContent {
            val isDark = when (model.appearance) {
                AppModel.Appearance.System -> isSystemInDarkTheme()
                AppModel.Appearance.Light -> false
                AppModel.Appearance.Dark -> true
            }
            // Status and navigation bar icons follow the app's theme, not only the phone's.
            LaunchedEffect(isDark) {
                val bars = if (isDark) SystemBarStyle.dark(Color.TRANSPARENT) else SystemBarStyle.light(Color.TRANSPARENT, Color.TRANSPARENT)
                enableEdgeToEdge(bars, bars)
            }
            SwitchboardTheme(isDark) {
                when (model.phase) {
                    AppModel.Phase.Paired -> PairedRoot(model, (application as SwitchboardApp).voice)
                    else -> PairingScreen(model)
                }
            }
        }
    }

    override fun onStart() {
        super.onStart()
        model.becameActive()
    }

    override fun onStop() {
        super.onStop()
        model.wentToBackground()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        opened(intent)
    }

    /** A `switchboard://pair` link fills in the pairing screen; nothing pairs without a tap. */
    private fun opened(intent: Intent) {
        val data = intent.data ?: return
        if (data.scheme == "switchboard" && data.host == "pair" && model.phase != AppModel.Phase.Paired) {
            model.openedLink = data.toString()
        }
    }
}

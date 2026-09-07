package com.example.wezterm_android

import android.app.Application
import com.google.android.material.color.DynamicColors

class WezTermApplication : Application() {
    override fun onCreate() {
        super.onCreate()
        DynamicColors.applyToActivitiesIfAvailable(this)
    }
}

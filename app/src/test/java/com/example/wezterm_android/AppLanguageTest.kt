package com.example.wezterm_android

import org.junit.Assert.assertEquals
import org.junit.Test

class AppLanguageTest {
    @Test
    fun emptyLanguageListFollowsTheSystem() {
        assertEquals(AppLanguage.SYSTEM, AppLanguage.fromLanguageTags(""))
    }

    @Test
    fun recognizesEverySupportedExplicitLanguage() {
        assertEquals(AppLanguage.ENGLISH, AppLanguage.fromLanguageTags("en"))
        assertEquals(
            AppLanguage.SIMPLIFIED_CHINESE,
            AppLanguage.fromLanguageTags("zh-CN"),
        )
    }

    @Test
    fun readsThePrimaryLocaleAndFallsBackSafely() {
        assertEquals(AppLanguage.ENGLISH, AppLanguage.fromLanguageTags("en,zh-CN"))
        assertEquals(AppLanguage.SYSTEM, AppLanguage.fromLanguageTags("fr"))
    }
}

package com.example.wezterm_android

enum class AppLanguage(val languageTag: String) {
    SYSTEM(""),
    ENGLISH("en"),
    SIMPLIFIED_CHINESE("zh-CN"),
    ;

    companion object {
        fun fromLanguageTags(languageTags: String): AppLanguage {
            val primaryTag = languageTags.substringBefore(',').trim()
            if (primaryTag.isEmpty()) return SYSTEM
            return entries.firstOrNull {
                it.languageTag.equals(primaryTag, ignoreCase = true)
            } ?: SYSTEM
        }
    }
}

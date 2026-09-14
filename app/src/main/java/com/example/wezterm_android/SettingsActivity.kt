package com.example.wezterm_android

import android.content.Intent
import android.graphics.Color
import android.net.Uri
import android.os.Bundle
import android.view.Gravity
import android.view.ViewGroup
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import androidx.activity.enableEdgeToEdge
import androidx.appcompat.app.AppCompatActivity
import androidx.appcompat.app.AppCompatDelegate
import androidx.core.os.LocaleListCompat
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import com.google.android.material.button.MaterialButton
import com.google.android.material.card.MaterialCardView
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import com.google.android.material.slider.Slider
import com.google.android.material.divider.MaterialDivider
import com.google.android.material.textview.MaterialTextView

class SettingsActivity : AppCompatActivity() {
    private lateinit var languageButton: MaterialButton

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()

        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(resolveThemeColor(com.google.android.material.R.attr.colorSurface))
        }

        val header = LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(dp(8), dp(6), dp(16), dp(6))
        }
        header.addView(
            MaterialButton(
                this,
                null,
                com.google.android.material.R.attr.materialIconButtonStyle,
            ).apply {
                text = getString(R.string.settings_back)
                contentDescription = getString(R.string.settings_back_description)
                setOnClickListener { finish() }
            },
            LinearLayout.LayoutParams(dp(48), dp(48)),
        )
        header.addView(
            MaterialTextView(this).apply {
                text = getString(R.string.settings_title)
                textSize = 22f
                setTextColor(resolveThemeColor(com.google.android.material.R.attr.colorOnSurface))
            },
            LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f),
        )
        root.addView(
            header,
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ),
        )
        root.addView(
            MaterialDivider(this),
            LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(1)),
        )

        val scroll = ScrollView(this).apply {
            isFillViewport = true
        }
        val content = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(20), dp(20), dp(20), dp(32))
        }

        content.addView(sectionTitle(R.string.settings_language_title))
        content.addView(
            MaterialTextView(this).apply {
                setText(R.string.settings_language_summary)
                textSize = 14f
                alpha = 0.72f
                setPadding(0, dp(4), 0, dp(10))
            },
        )
        languageButton = MaterialButton(
            this,
            null,
            com.google.android.material.R.attr.materialButtonOutlinedStyle,
        ).apply {
            isAllCaps = false
            gravity = Gravity.START or Gravity.CENTER_VERTICAL
            setOnClickListener { showLanguageDialog() }
        }
        content.addView(
            languageButton,
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ),
        )

        content.addView(
            sectionTitle(R.string.settings_terminal_zoom_title).apply {
                setPadding(0, dp(28), 0, dp(10))
            },
        )
        content.addView(
            MaterialTextView(this).apply {
                setText(R.string.settings_terminal_zoom_summary)
                textSize = 14f
                alpha = 0.72f
                setPadding(0, dp(4), 0, dp(10))
            },
        )

        val zoomPreview = TerminalGridPreviewView(this).apply {
            zoomPercent = TerminalZoom.load(this@SettingsActivity)
        }
        val zoomGridLabel = MaterialTextView(this).apply {
            textSize = 13f
            alpha = 0.85f
            gravity = Gravity.CENTER
            setPadding(0, dp(8), 0, dp(4))
        }
        fun updateZoomGridLabel() {
            val (columns, rows) = zoomPreview.gridDimensions()
            zoomGridLabel.text = getString(
                R.string.settings_terminal_zoom_grid,
                columns,
                rows,
                zoomPreview.zoomPercent,
            )
        }
        zoomPreview.onGridChanged = { _, _ -> updateZoomGridLabel() }
        content.addView(
            MaterialCardView(this).apply {
                radius = dp(12).toFloat()
                cardElevation = 0f
                strokeWidth = 0
                addView(
                    zoomPreview,
                    LinearLayout.LayoutParams(
                        ViewGroup.LayoutParams.MATCH_PARENT,
                        dp(180),
                    ),
                )
            },
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ),
        )
        content.addView(zoomGridLabel)
        content.addView(
            Slider(this).apply {
                valueFrom = TerminalZoom.MIN_PERCENT.toFloat()
                valueTo = TerminalZoom.MAX_PERCENT.toFloat()
                stepSize = TerminalZoom.STEP_PERCENT.toFloat()
                value = zoomPreview.zoomPercent.toFloat()
                addOnChangeListener { _, value, fromUser ->
                    if (!fromUser) return@addOnChangeListener
                    val percent = value.toInt()
                    zoomPreview.zoomPercent = percent
                    TerminalZoom.save(this@SettingsActivity, percent)
                    updateZoomGridLabel()
                }
            },
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ),
        )

        content.addView(
            sectionTitle(R.string.settings_developer_title).apply {
                setPadding(0, dp(28), 0, dp(10))
            },
        )
        val developerCard = MaterialCardView(this).apply {
            radius = dp(16).toFloat()
            cardElevation = 0f
        }
        val developerContent = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(dp(18), dp(18), dp(18), dp(14))
            addView(
                MaterialTextView(this@SettingsActivity).apply {
                    text = getString(R.string.settings_developer_name)
                    textSize = 18f
                },
            )
            addView(
                MaterialTextView(this@SettingsActivity).apply {
                    setText(R.string.settings_developer_description)
                    textSize = 14f
                    alpha = 0.78f
                    setPadding(0, dp(8), 0, dp(8))
                },
            )
            addView(
                MaterialTextView(this@SettingsActivity).apply {
                    text = getString(R.string.settings_version, appVersionName())
                    textSize = 13f
                    alpha = 0.7f
                },
            )
            addView(
                MaterialButton(this@SettingsActivity).apply {
                    setText(R.string.settings_open_source)
                    isAllCaps = false
                    gravity = Gravity.START or Gravity.CENTER_VERTICAL
                    setOnClickListener { openProjectSource() }
                },
                LinearLayout.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.WRAP_CONTENT,
                ),
            )
        }
        developerCard.addView(developerContent)
        content.addView(
            developerCard,
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ),
        )

        scroll.addView(
            content,
            ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ),
        )
        root.addView(
            scroll,
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                0,
                1f,
            ),
        )

        ViewCompat.setOnApplyWindowInsetsListener(root) { view, insets ->
            val safe = insets.getInsets(
                WindowInsetsCompat.Type.systemBars() or
                    WindowInsetsCompat.Type.displayCutout(),
            )
            view.setPadding(safe.left, safe.top, safe.right, safe.bottom)
            insets
        }
        setContentView(root)
        ViewCompat.requestApplyInsets(root)
        updateLanguageButton()
    }

    override fun onResume() {
        super.onResume()
        if (::languageButton.isInitialized) updateLanguageButton()
    }

    private fun showLanguageDialog() {
        val languages = AppLanguage.entries
        val labels = languages.map(::languageLabel).toTypedArray()
        val selected = currentLanguage()
        MaterialAlertDialogBuilder(this)
            .setTitle(R.string.settings_language_title)
            .setSingleChoiceItems(labels, languages.indexOf(selected)) { dialog, which ->
                val language = languages[which]
                dialog.dismiss()
                window.decorView.post {
                    AppCompatDelegate.setApplicationLocales(
                        if (language == AppLanguage.SYSTEM) {
                            LocaleListCompat.getEmptyLocaleList()
                        } else {
                            LocaleListCompat.forLanguageTags(language.languageTag)
                        },
                    )
                }
            }
            .setNegativeButton(android.R.string.cancel, null)
            .show()
    }

    private fun updateLanguageButton() {
        languageButton.text = languageLabel(currentLanguage())
    }

    private fun currentLanguage(): AppLanguage =
        AppLanguage.fromLanguageTags(
            AppCompatDelegate.getApplicationLocales().toLanguageTags(),
        )

    private fun languageLabel(language: AppLanguage): String =
        getString(
            when (language) {
                AppLanguage.SYSTEM -> R.string.settings_language_system
                AppLanguage.ENGLISH -> R.string.settings_language_english
                AppLanguage.SIMPLIFIED_CHINESE -> R.string.settings_language_simplified_chinese
            },
        )

    private fun sectionTitle(textResource: Int): TextView =
        MaterialTextView(this).apply {
            setText(textResource)
            textSize = 16f
            setTextColor(resolveThemeColor(androidx.appcompat.R.attr.colorPrimary))
        }

    private fun openProjectSource() {
        startActivity(
            Intent(Intent.ACTION_VIEW, Uri.parse(getString(R.string.settings_project_url))),
        )
    }

    @Suppress("DEPRECATION")
    private fun appVersionName(): String =
        packageManager.getPackageInfo(packageName, 0).versionName ?: "—"

    private fun resolveThemeColor(attribute: Int): Int {
        val value = android.util.TypedValue()
        return if (theme.resolveAttribute(attribute, value, true)) value.data else Color.BLACK
    }

    private fun dp(value: Int): Int =
        (value * resources.displayMetrics.density).toInt()
}

package expo.modules.takusuwidget

import android.content.Context
import android.content.SharedPreferences
import android.widget.RemoteViews
import androidx.core.content.ContextCompat

// These mirror the constants in expo.modules.takusuappicon.TakusuTheme.
private const val PREFS_NAME = "takusu_theme_prefs"
private const val KEY_THEME = "takusu_theme"
private const val DEFAULT_THEME = "light"

internal object WidgetTheme {
    private data class Style(
        val backgroundRes: Int,
        val logoColorRes: Int,
    )

    // Colors referenced here are defined in colors.xml and mirror the app icon palette in takusu-app-icon.
    private val styles =
        mapOf(
            "light" to Style(R.drawable.brand_icon_bg, R.color.takusu_widget_brand),
            "dark" to Style(R.drawable.brand_icon_bg_dark, R.color.takusu_widget_logo_dark),
            "catppuccin" to Style(R.drawable.brand_icon_bg_catppuccin, R.color.takusu_widget_logo_dark),
            "aura-soft-dark" to Style(R.drawable.brand_icon_bg_aura_soft_dark, R.color.takusu_widget_brand_light),
        )

    private fun getPreferences(context: Context): SharedPreferences =
        context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)

    fun currentTheme(context: Context): String =
        getPreferences(context).getString(KEY_THEME, DEFAULT_THEME) ?: DEFAULT_THEME

    fun apply(
        context: Context,
        views: RemoteViews,
    ) {
        val theme = currentTheme(context)
        val style = styles[theme] ?: styles.getValue(DEFAULT_THEME)
        val logoColor = ContextCompat.getColor(context, style.logoColorRes)
        views.setInt(R.id.widget_brand_icon, "setBackgroundResource", style.backgroundRes)
        views.setInt(R.id.widget_brand_icon, "setColorFilter", logoColor)
    }
}

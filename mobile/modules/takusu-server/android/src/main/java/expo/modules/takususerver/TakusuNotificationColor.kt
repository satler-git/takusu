package expo.modules.takususerver

import android.content.Context
import android.graphics.Color

// Theme-aware tint color for notification icons.
//
// The small icon is an all-white silhouette, and Android tints it with the
// notification color. This helper reads the active app icon theme saved by
// TakusuAppIconModule and returns the matching brand/foreground color.
internal object TakusuNotificationColor {
    private const val PREFS_NAME = "takusu_theme_prefs"
    private const val KEY_THEME = "takusu_theme"

    fun color(context: Context): Int {
        val prefs = context.getSharedPreferences(PREFS_NAME, Context.MODE_PRIVATE)
        val theme = prefs.getString(KEY_THEME, "light") ?: "light"
        return when (theme) {
            "dark", "catppuccin" -> Color.parseColor("#9B8BC4")
            "aura-soft-dark" -> Color.parseColor("#A48BD6")
            else -> Color.parseColor("#7261A3")
        }
    }
}

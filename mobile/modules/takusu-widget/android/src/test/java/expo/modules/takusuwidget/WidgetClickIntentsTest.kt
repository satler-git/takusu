package expo.modules.takusuwidget

import android.app.PendingIntent
import android.content.ComponentName
import android.content.Intent
import android.os.Build
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment
import org.robolectric.Shadows
import org.robolectric.annotation.Config

@RunWith(RobolectricTestRunner::class)
@Config(sdk = [Build.VERSION_CODES.UPSIDE_DOWN_CAKE])
class WidgetClickIntentsTest {
    @Test(expected = IllegalArgumentException::class)
    fun createListPendingIntent_throwsWhenComponentIsNull() {
        val context = RuntimeEnvironment.getApplication()
        val launchIntent =
            Intent(Intent.ACTION_MAIN).apply {
                `package` = context.packageName
            }

        WidgetClickIntents.createListPendingIntent(context, launchIntent)
    }

    @Test
    fun createListPendingIntent_usesExplicitComponentAndMutableFlag() {
        val context = RuntimeEnvironment.getApplication()
        val component = ComponentName(context.packageName, "MainActivity")
        val launchIntent =
            Intent(Intent.ACTION_MAIN).apply {
                addCategory(Intent.CATEGORY_LAUNCHER)
                `package` = context.packageName
                setComponent(component)
            }

        val pendingIntent =
            WidgetClickIntents.createListPendingIntent(context, launchIntent)

        val shadow = Shadows.shadowOf(pendingIntent)
        val savedIntent = shadow.savedIntent
        assertNotNull(savedIntent)
        assertEquals(Intent.ACTION_VIEW, savedIntent.action)
        assertEquals(component, savedIntent.component)
        assertEquals(context.packageName, savedIntent.`package`)
        assertTrue((shadow.flags and PendingIntent.FLAG_MUTABLE) != 0)
        assertTrue((shadow.flags and PendingIntent.FLAG_UPDATE_CURRENT) != 0)
        assertTrue((savedIntent.flags and Intent.FLAG_ACTIVITY_NEW_TASK) != 0)
        assertTrue((savedIntent.flags and Intent.FLAG_ACTIVITY_CLEAR_TOP) != 0)
    }

    @Test
    fun createRootPendingIntent_usesImmutableFlag() {
        val context = RuntimeEnvironment.getApplication()
        val component = ComponentName(context.packageName, "MainActivity")
        val launchIntent =
            Intent(Intent.ACTION_MAIN).apply {
                addCategory(Intent.CATEGORY_LAUNCHER)
                `package` = context.packageName
                setComponent(component)
            }

        val pendingIntent =
            WidgetClickIntents.createRootPendingIntent(context, launchIntent)

        val shadow = Shadows.shadowOf(pendingIntent)
        val savedIntent = shadow.savedIntent
        assertNotNull(savedIntent)
        assertEquals(component, savedIntent.component)
        assertTrue((shadow.flags and PendingIntent.FLAG_IMMUTABLE) != 0)
        assertTrue((shadow.flags and PendingIntent.FLAG_UPDATE_CURRENT) != 0)
    }
}

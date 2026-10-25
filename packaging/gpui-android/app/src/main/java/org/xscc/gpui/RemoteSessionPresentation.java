package org.xscc.gpui;

import android.app.Activity;
import android.content.pm.ActivityInfo;
import android.os.Build;
import android.os.Looper;
import android.util.Log;
import android.view.View;
import android.view.Window;

import androidx.core.view.ViewCompat;
import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.core.view.WindowInsetsControllerCompat;

/** A reversible presentation request, separate from the Activity's text input. */
public final class RemoteSessionPresentation {
    private final Activity activity;
    private Snapshot original;

    public RemoteSessionPresentation(Activity activity) {
        this.activity = activity;
    }

    public void setEnabled(boolean enabled) {
        if (Looper.myLooper() != Looper.getMainLooper()) {
            throw new IllegalStateException("Session presentation must use the UI thread");
        }
        if (enabled == (original != null)) {
            return;
        }
        if (!enabled) {
            restore();
            return;
        }
        try {
            original = new Snapshot(activity);
            activity.setRequestedOrientation(ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE);
            WindowInsetsControllerCompat controller = original.controller;
            controller.setSystemBarsBehavior(
                    WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
            // Never hide Type.ime(): the keyboard remains a separate inset.
            controller.hide(WindowInsetsCompat.Type.systemBars());
        } catch (RuntimeException error) {
            Log.w("GPUI", "Cannot apply remote session presentation", error);
            restore();
        }
    }

    @SuppressWarnings("deprecation") // API29 keeps its original decor visibility flags.
    private void restore() {
        Snapshot saved = original;
        if (saved == null) {
            return;
        }
        boolean restored = true;
        try {
            activity.setRequestedOrientation(saved.orientation);
        } catch (RuntimeException error) {
            restored = false;
            Log.w("GPUI", "Cannot restore Activity orientation", error);
        }
        try {
            saved.controller.setSystemBarsBehavior(saved.behavior);
            int hidden = WindowInsetsCompat.Type.systemBars() & ~saved.visibleBars;
            if (hidden != 0) {
                saved.controller.hide(hidden);
            }
            if (saved.visibleBars != 0) {
                saved.controller.show(saved.visibleBars);
            }
            if (Build.VERSION.SDK_INT < 30) {
                saved.decor.setSystemUiVisibility(saved.decorFlags);
            }
        } catch (RuntimeException error) {
            restored = false;
            Log.w("GPUI", "Cannot restore system bars", error);
        }
        if (restored) {
            original = null;
        }
    }

    private static final class Snapshot {
        final int orientation;
        final View decor;
        final int decorFlags;
        final int behavior;
        final int visibleBars;
        final WindowInsetsControllerCompat controller;

        @SuppressWarnings("deprecation")
        Snapshot(Activity activity) {
            orientation = activity.getRequestedOrientation();
            Window window = activity.getWindow();
            decor = window.getDecorView();
            decorFlags = decor.getSystemUiVisibility();
            controller = WindowCompat.getInsetsController(window, decor);
            behavior = controller.getSystemBarsBehavior();
            WindowInsetsCompat insets = ViewCompat.getRootWindowInsets(decor);
            int visible = 0;
            for (int type : new int[] {WindowInsetsCompat.Type.statusBars(),
                    WindowInsetsCompat.Type.navigationBars(), WindowInsetsCompat.Type.captionBar()}) {
                boolean shown = insets != null ? insets.isVisible(type)
                        : (type == WindowInsetsCompat.Type.statusBars()
                            ? (decorFlags & View.SYSTEM_UI_FLAG_FULLSCREEN) == 0
                            : type == WindowInsetsCompat.Type.navigationBars()
                                && (decorFlags & View.SYSTEM_UI_FLAG_HIDE_NAVIGATION) == 0);
                if (shown) {
                    visible |= type;
                }
            }
            visibleBars = visible;
        }
    }
}

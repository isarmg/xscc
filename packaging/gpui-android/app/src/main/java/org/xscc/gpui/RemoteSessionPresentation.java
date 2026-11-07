package org.xscc.gpui;

import android.content.pm.ActivityInfo;
import android.os.Build;
import android.os.Looper;
import android.util.Log;
import android.view.Display;
import android.view.Surface;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.View;
import android.view.Window;
import android.view.WindowManager;

import androidx.core.view.ViewCompat;
import androidx.core.view.WindowCompat;
import androidx.core.view.WindowInsetsCompat;
import androidx.core.view.WindowInsetsControllerCompat;

/** A reversible presentation request, separate from the Activity's text input. */
public final class RemoteSessionPresentation implements SurfaceHolder.Callback {
    private final AndroidImeBridge activity;
    private Snapshot original;

    public RemoteSessionPresentation(AndroidImeBridge activity) {
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
            activity.getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
            if (original.surfaceHolder != null) {
                original.surfaceHolder.addCallback(this);
            }
            requestHighRefreshRate();
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

    private void requestHighRefreshRate() {
        if (original == null) {
            return;
        }
        Window window = activity.getWindow();
        try {
            Display display = window.getDecorView().getDisplay();
            if (display == null) {
                Log.w("GPUI", "Remote session display unavailable; cannot request refresh rate");
                return;
            }
            Display.Mode current = display.getMode();
            Display.Mode preferred = current;
            for (Display.Mode mode : display.getSupportedModes()) {
                if (mode.getPhysicalWidth() == current.getPhysicalWidth()
                        && mode.getPhysicalHeight() == current.getPhysicalHeight()
                        && mode.getRefreshRate() > preferred.getRefreshRate()) {
                    preferred = mode;
                }
            }
            WindowManager.LayoutParams attributes = window.getAttributes();
            float refreshRate = preferred.getRefreshRate();
            attributes.preferredDisplayModeId = preferred.getModeId();
            attributes.preferredRefreshRate = refreshRate;
            window.setAttributes(attributes);
            setSurfaceFrameRate(original.surfaceHolder, refreshRate);
        } catch (RuntimeException error) {
            Log.w("GPUI", "Cannot request remote session refresh rate", error);
        }
    }

    private static void setSurfaceFrameRate(SurfaceHolder holder, float refreshRate) {
        if (Build.VERSION.SDK_INT >= 30 && holder != null) {
            Surface surface = holder.getSurface();
            if (surface.isValid()) {
                surface.setFrameRate(refreshRate, Surface.FRAME_RATE_COMPATIBILITY_DEFAULT);
            }
        }
    }

    @Override
    public void surfaceCreated(SurfaceHolder holder) {
        requestHighRefreshRate();
    }

    @Override
    public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) {
        requestHighRefreshRate();
    }

    @Override
    public void surfaceDestroyed(SurfaceHolder holder) {
        // A destroyed surface has no frame-rate vote to clear.
    }

    @SuppressWarnings("deprecation") // API29 keeps its original decor visibility flags.
    private void restore() {
        Snapshot saved = original;
        if (saved == null) {
            return;
        }
        boolean restored = true;
        try {
            if (saved.surfaceHolder != null) {
                saved.surfaceHolder.removeCallback(this);
            }
            setSurfaceFrameRate(saved.surfaceHolder, 0);
        } catch (RuntimeException error) {
            restored = false;
            Log.w("GPUI", "Cannot clear session surface refresh preference", error);
        }
        try {
            activity.setRequestedOrientation(saved.orientation);
        } catch (RuntimeException error) {
            restored = false;
            Log.w("GPUI", "Cannot restore Activity orientation", error);
        }
        try {
            if (saved.keepScreenOn) {
                activity.getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
            } else {
                activity.getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
            }
        } catch (RuntimeException error) {
            restored = false;
            Log.w("GPUI", "Cannot restore screen wake policy", error);
        }
        try {
            Window window = activity.getWindow();
            WindowManager.LayoutParams attributes = window.getAttributes();
            attributes.preferredDisplayModeId = saved.preferredDisplayModeId;
            attributes.preferredRefreshRate = saved.preferredRefreshRate;
            window.setAttributes(attributes);
        } catch (RuntimeException error) {
            restored = false;
            Log.w("GPUI", "Cannot restore window refresh preference", error);
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
        final SurfaceHolder surfaceHolder;
        final int orientation;
        final boolean keepScreenOn;
        final int preferredDisplayModeId;
        final float preferredRefreshRate;
        final View decor;
        final int decorFlags;
        final int behavior;
        final int visibleBars;
        final WindowInsetsControllerCompat controller;

        @SuppressWarnings("deprecation")
        Snapshot(AndroidImeBridge activity) {
            SurfaceView surface = activity.remoteSessionSurfaceView();
            surfaceHolder = surface != null ? surface.getHolder() : null;
            orientation = activity.getRequestedOrientation();
            Window window = activity.getWindow();
            WindowManager.LayoutParams attributes = window.getAttributes();
            keepScreenOn = (attributes.flags & WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) != 0;
            preferredDisplayModeId = attributes.preferredDisplayModeId;
            preferredRefreshRate = attributes.preferredRefreshRate;
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

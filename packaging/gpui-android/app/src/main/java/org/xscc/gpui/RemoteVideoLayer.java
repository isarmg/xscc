package org.xscc.gpui;

import android.graphics.Color;
import android.graphics.PixelFormat;
import android.graphics.drawable.Drawable;
import android.util.Log;
import android.view.Display;
import android.view.Gravity;
import android.view.Surface;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.widget.FrameLayout;

/** Hosts the codec's Surface below the GPUI Surface. */
public final class RemoteVideoLayer {
    private final AndroidImeBridge activity;
    private long owner;
    private int frameRate;
    private int displayLimit;
    private SurfaceView view;
    private FrameLayout parent;
    private Drawable previousBackground;

    public RemoteVideoLayer(AndroidImeBridge activity) {
        this.activity = activity;
    }

    public static void prepareOverlay(SurfaceView view) {
        view.setZOrderMediaOverlay(true);
        view.getHolder().setFormat(PixelFormat.TRANSLUCENT);
    }

    public void place(long owner, int left, int top, int width, int height, int fps) {
        if (width <= 0 || height <= 0) {
            if (this.owner == owner) clear();
            return;
        }
        SurfaceView overlay = activity.remoteSessionSurfaceView();
        if (overlay == null || !(overlay.getParent() instanceof FrameLayout)) {
            Log.w("GPUI", "Cannot place remote video SurfaceView");
            return;
        }
        FrameLayout root = (FrameLayout) overlay.getParent();
        FrameLayout.LayoutParams bounds = new FrameLayout.LayoutParams(
                width, height, Gravity.TOP | Gravity.LEFT);
        bounds.leftMargin = overlay.getLeft() + left;
        bounds.topMargin = overlay.getTop() + top;
        if (this.owner == owner && view != null) {
            if (frameRate != fps) {
                frameRate = fps;
                setFrameRate(view.getHolder().getSurface());
            }
            view.setLayoutParams(bounds);
            return;
        }
        clear();
        this.owner = owner;
        frameRate = fps;
        Display display = overlay.getDisplay();
        displayLimit = display == null ? 0 : Math.round(
                RemoteSessionPresentation.fastestModeAtCurrentSize(display).getRefreshRate());
        parent = root;
        previousBackground = root.getBackground();
        root.setBackgroundColor(Color.BLACK);
        view = new SurfaceView(activity);
        view.setImportantForAccessibility(SurfaceView.IMPORTANT_FOR_ACCESSIBILITY_NO);
        view.getHolder().addCallback(new SurfaceHolder.Callback() {
            @Override
            public void surfaceCreated(SurfaceHolder holder) {
                Surface surface = holder.getSurface();
                setFrameRate(surface);
                nativeSurfaceChanged(owner, surface, displayLimit);
            }

            @Override
            public void surfaceChanged(SurfaceHolder holder, int format, int width, int height) {}

            @Override
            public void surfaceDestroyed(SurfaceHolder holder) {
                nativeSurfaceChanged(owner, null, 0);
            }
        });
        parent.addView(view, root.indexOfChild(overlay), bounds);
    }

    private void setFrameRate(Surface surface) {
        if (!surface.isValid()) return;
        try {
            int rate = displayLimit > 0 ? Math.min(frameRate, displayLimit) : frameRate;
            surface.setFrameRate(rate, Surface.FRAME_RATE_COMPATIBILITY_FIXED_SOURCE);
        } catch (IllegalArgumentException | IllegalStateException error) {
            Log.w("GPUI", "Cannot set remote video Surface frame rate", error);
        }
    }

    public void clear() {
        if (view == null) return;
        parent.removeView(view);
        parent.setBackground(previousBackground);
        view = null;
        parent = null;
        previousBackground = null;
        owner = 0;
    }

    private static native void nativeSurfaceChanged(long owner, Surface surface, int displayLimit);
}

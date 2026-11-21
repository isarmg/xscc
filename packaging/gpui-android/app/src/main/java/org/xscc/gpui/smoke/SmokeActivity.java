package org.xscc.gpui.smoke;

import android.media.AudioManager;
import android.os.Bundle;
import android.view.SurfaceView;

import org.xscc.gpui.AndroidImeBridge;
import org.xscc.gpui.RemoteSessionPresentation;
import org.xscc.gpui.RemoteVideoLayer;

public final class SmokeActivity extends AndroidImeBridge {
    private final RemoteSessionPresentation sessionPresentation = new RemoteSessionPresentation(this);
    private final RemoteVideoLayer videoLayer = new RemoteVideoLayer(this);

    @Override
    protected void prepareRenderSurface(SurfaceView surface) {
        if ("management".equals(BuildConfig.FLAVOR) || "landirect".equals(BuildConfig.FLAVOR)) {
            RemoteVideoLayer.prepareOverlay(surface);
        }
    }

    public final void placeRemoteVideoSurface(long owner, int x, int y, int width, int height, int fps) {
        videoLayer.place(owner, x, y, width, height, fps);
    }

    public final void setRemoteSessionPresentation(boolean enabled) {
        sessionPresentation.setEnabled(enabled);
    }

    public final void rotateRemoteSession() {
        sessionPresentation.rotate();
    }

    @Override
    protected void onStop() {
        sessionPresentation.setEnabled(false);
        super.onStop();
    }

    @Override
    protected void onDestroy() {
        videoLayer.clear();
        sessionPresentation.setEnabled(false);
        super.onDestroy();
    }
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        if ("management".equals(BuildConfig.FLAVOR) || "landirect".equals(BuildConfig.FLAVOR)) {
            setVolumeControlStream(AudioManager.STREAM_MUSIC);
        }
    }

    static {
        System.loadLibrary(BuildConfig.NATIVE_LIBRARY);
    }
}

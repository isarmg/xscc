package org.xscc.gpui.smoke;

import android.media.AudioManager;
import android.net.wifi.WifiManager;
import android.os.Bundle;
import android.util.Log;
import android.view.SurfaceView;

import org.xscc.gpui.AndroidImeBridge;
import org.xscc.gpui.RemoteSessionPresentation;
import org.xscc.gpui.RemoteVideoLayer;

public final class SmokeActivity extends AndroidImeBridge {
    private final RemoteSessionPresentation sessionPresentation = new RemoteSessionPresentation(this);
    private final RemoteVideoLayer videoLayer = new RemoteVideoLayer(this);
    private WifiManager.WifiLock lowLatencyWifi;

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
        setWifiLowLatency(enabled);
    }

    private void setWifiLowLatency(boolean enabled) {
        if (enabled == (lowLatencyWifi != null)) {
            return;
        }
        try {
            if (enabled) {
                WifiManager wifi = getApplicationContext().getSystemService(WifiManager.class);
                if (wifi != null) {
                    WifiManager.WifiLock lock = wifi.createWifiLock(
                            WifiManager.WIFI_MODE_FULL_LOW_LATENCY, "XSCC remote session");
                    lock.setReferenceCounted(false);
                    lock.acquire();
                    lowLatencyWifi = lock;
                }
            } else {
                WifiManager.WifiLock lock = lowLatencyWifi;
                lowLatencyWifi = null;
                lock.release();
            }
        } catch (RuntimeException error) {
            Log.w("GPUI", "Cannot update the remote session Wi-Fi latency mode", error);
        }
    }

    public final void rotateRemoteSession() {
        sessionPresentation.rotate();
    }

    @Override
    protected void onStop() {
        setRemoteSessionPresentation(false);
        super.onStop();
    }

    @Override
    protected void onDestroy() {
        videoLayer.clear();
        setRemoteSessionPresentation(false);
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

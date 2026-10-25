package org.xscc.gpui.smoke;

import android.media.AudioManager;
import android.os.Bundle;

import org.xscc.gpui.AndroidImeBridge;
import org.xscc.gpui.RemoteSessionPresentation;

public final class SmokeActivity extends AndroidImeBridge {
    private final RemoteSessionPresentation sessionPresentation = new RemoteSessionPresentation(this);

    public final void setRemoteSessionPresentation(boolean enabled) {
        sessionPresentation.setEnabled(enabled);
    }

    @Override
    protected void onStop() {
        sessionPresentation.setEnabled(false);
        super.onStop();
    }

    @Override
    protected void onDestroy() {
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

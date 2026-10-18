package org.xscc.gpui.smoke;

import android.media.AudioManager;
import android.os.Bundle;

import org.xscc.gpui.AndroidImeBridge;

public final class SmokeActivity extends AndroidImeBridge {
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

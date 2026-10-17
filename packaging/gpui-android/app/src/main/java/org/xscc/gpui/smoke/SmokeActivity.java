package org.xscc.gpui.smoke;

import com.google.androidgamesdk.GameActivity;

public final class SmokeActivity extends GameActivity {
    static {
        System.loadLibrary("xscc_gpui_android_smoke");
    }
}

package org.xscc.gpui.smoke;

import org.xscc.gpui.AndroidImeBridge;

public final class SmokeActivity extends AndroidImeBridge {
    static {
        System.loadLibrary("xscc_gpui_android_smoke");
    }
}

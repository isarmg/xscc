# Replace AndroidX annotation consumer rules, retaining their semantics outside
# GameActivity. Its @Keep JNI classes need body optimization to remove text logs.
-keep,allowobfuscation @interface androidx.annotation.Keep
-keep @androidx.annotation.Keep class !com.google.androidgamesdk.**,** { *; }
-keepclasseswithmembers class !com.google.androidgamesdk.**,** {
    @androidx.annotation.Keep <methods>;
}
-keepclasseswithmembers class !com.google.androidgamesdk.**,** {
    @androidx.annotation.Keep <fields>;
}
-keepclasseswithmembers class !com.google.androidgamesdk.**,** {
    @androidx.annotation.Keep <init>(...);
}
-keepclassmembers,allowobfuscation class * {
    @androidx.annotation.DoNotInline <methods>;
}

# JNI uses string-based class/member lookup. Keep names and every member, but
# allow method body optimization; do not allow shrinking or obfuscation.
-keep,allowoptimization class com.google.androidgamesdk.** { *; }

# Upstream GameTextInput logs plaintext at debug level. Retain info/warn/error
# diagnostics; the final DEX gate also rejects surviving sensitive log formats.
-assumenosideeffects class android.util.Log {
    public static int d(...);
    public static int v(...);
}

# Rust resolves these methods and inset fields by their Java names.
-keep,allowoptimization class org.xscc.gpui.AndroidImeBridge** { *; }
-keep class androidx.core.view.WindowInsetsCompat$Type {
    public static int systemBars();
    public static int displayCutout();
    public static int ime();
}
-keep class androidx.core.graphics.Insets {
    public int left;
    public int top;
    public int right;
    public int bottom;
}

package org.xscc.gpui;

import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.media.AudioAttributes;
import android.media.AudioFocusRequest;
import android.media.AudioManager;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;

import androidx.annotation.Keep;

/** Output-only foreground audio focus, owned by one native playback generation. */
@Keep
public final class AndroidPlaybackAudio implements AudioManager.OnAudioFocusChangeListener {
    private final Context context;
    private final AudioManager manager;
    private final AudioFocusRequest request;
    private long generation;
    private boolean receiverRegistered;
    private boolean closed;

    private final BroadcastReceiver noisyReceiver = new BroadcastReceiver() {
        @Override
        public void onReceive(Context context, Intent intent) {
            if (AudioManager.ACTION_AUDIO_BECOMING_NOISY.equals(intent.getAction())) {
                stop(2);
            }
        }
    };

    private static native void nativeStopped(long generation, int reason);

    public AndroidPlaybackAudio(Context application, long generation) {
        context = application.getApplicationContext();
        manager = (AudioManager) context.getSystemService(Context.AUDIO_SERVICE);
        if (manager == null) {
            throw new IllegalStateException("Android audio service is unavailable");
        }
        this.generation = generation;
        request = new AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
                .setAudioAttributes(new AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_MEDIA)
                        .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                        .build())
                .setAcceptsDelayedFocusGain(false)
                .setWillPauseWhenDucked(true)
                .setOnAudioFocusChangeListener(this, new Handler(Looper.getMainLooper()))
                .build();
    }

    public synchronized boolean activate() {
        if (closed) {
            return false;
        }
        IntentFilter filter = new IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY);
        if (Build.VERSION.SDK_INT >= 33) {
            context.registerReceiver(noisyReceiver, filter, Context.RECEIVER_NOT_EXPORTED);
        } else {
            context.registerReceiver(noisyReceiver, filter);
        }
        receiverRegistered = true;
        return manager.requestAudioFocus(request) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED;
    }

    @Override
    public void onAudioFocusChange(int change) {
        if (change == AudioManager.AUDIOFOCUS_LOSS
                || change == AudioManager.AUDIOFOCUS_LOSS_TRANSIENT
                || change == AudioManager.AUDIOFOCUS_LOSS_TRANSIENT_CAN_DUCK) {
            stop(1);
        }
        // Only a new user request may resume playback after any interruption.
    }

    private synchronized void stop(int reason) {
        if (!closed && generation != 0) {
            nativeStopped(generation, reason);
            generation = 0;
        }
    }

    public synchronized void close() {
        if (closed) {
            return;
        }
        closed = true;
        generation = 0;
        try {
            manager.abandonAudioFocusRequest(request);
        } finally {
            if (receiverRegistered) {
                receiverRegistered = false;
                context.unregisterReceiver(noisyReceiver);
            }
        }
    }
}

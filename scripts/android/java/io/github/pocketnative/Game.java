package io.github.pocketnative;

import android.app.NativeActivity;
import android.content.Intent;
import android.content.pm.ActivityInfo;
import android.os.Bundle;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.TimeUnit;

/**
 * A game, in a process of its own: the NativeActivity libpocketnative.so runs,
 * full screen and in the orientation the launcher asks for. The game's folder
 * comes in the "game" extra, which android_main reads.
 */
public final class Game extends NativeActivity {
    @Override
    protected void onCreate(Bundle state) {
        String orientation = getIntent().getStringExtra("orientation");
        if ("portrait".equals(orientation)) {
            setRequestedOrientation(ActivityInfo.SCREEN_ORIENTATION_SENSOR_PORTRAIT);
        } else if ("landscape".equals(orientation)) {
            setRequestedOrientation(ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE);
        } else {
            // turning with the phone, the screens laid out again each time
            setRequestedOrientation(ActivityInfo.SCREEN_ORIENTATION_FULL_SENSOR);
        }
        getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        // which game this process plays, for the launcher to bring it back
        // rather than start another over it
        try (FileOutputStream out = new FileOutputStream(new File(getFilesDir(), "playing"))) {
            out.write(String.valueOf(getIntent().getStringExtra("game")).getBytes(StandardCharsets.UTF_8));
        } catch (IOException ignored) {
        }
        super.onCreate(state);
        hideBars();
    }

    @Override
    public void onWindowFocusChanged(boolean focused) {
        super.onWindowFocusChanged(focused);
        if (focused) {
            hideBars();
        }
    }

    private void hideBars() {
        WindowInsetsController bars = getWindow().getInsetsController();
        if (bars != null) {
            bars.hide(WindowInsets.Type.systemBars());
            bars.setSystemBarsBehavior(WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
        }
    }

    /**
     * ends the game, from the game's thread, its exit code 3 asking for it
     * again: waits until the activity is finishing, so that the process may
     * end right after.
     */
    public void finishWith(final int code) {
        final CountDownLatch done = new CountDownLatch(1);
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                // Reset: the launcher starts the game again once this
                // process has ended
                if (code == 3) {
                    startActivity(new Intent("io.github.pocketnative.PLAY").setClass(Game.this, Launcher.class)
                            .putExtra("game", getIntent().getStringExtra("game")).putExtra("again", true));
                }
                finish();
                done.countDown();
            }
        });
        try {
            done.await(1, TimeUnit.SECONDS);
        } catch (InterruptedException ignored) {
        }
    }
}

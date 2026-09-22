package dev.androidsimulator.agent;

import android.app.Service;
import android.content.Intent;
import android.os.IBinder;

public final class AndroidSimulatorAgentService extends Service {
    public static final String ACTION_PING = "dev.androidsimulator.agent.PING";

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        stopSelf(startId);
        return START_NOT_STICKY;
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }
}

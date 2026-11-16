package com.kobaproduction.vpnclient;

import android.app.job.JobInfo;
import android.app.job.JobParameters;
import android.app.job.JobScheduler;
import android.app.job.JobService;
import android.content.ComponentName;
import android.content.Context;
import android.os.PersistableBundle;
import android.util.Log;
import org.json.JSONException;
import org.json.JSONObject;

public final class SubscriptionRefreshJob extends JobService {
    private static final int JOB_ID = 0x53554252;
    private static final String DEADLINE = "deadline";
    private static final long DEFERRED_RETRY_MILLIS = 15 * 60 * 1000;
    private static Context application;
    private static SubscriptionRefreshJob running;
    private JobParameters parameters;
    private long request;

    static void attach(Context context) {
        application = context.getApplicationContext();
    }

    @Override public boolean onStartJob(JobParameters params) {
        parameters = params;
        running = this;
        if (NativeSession.openSession(this, false)) {
            request = NativeSession.startPeriodicRefresh();
        }
        NativeSession.onNativeUpdate();
        return true;
    }

    @Override public boolean onStopJob(JobParameters params) {
        if (parameters == null || parameters.getJobId() != params.getJobId()) return false;
        release();
        return true;
    }

    static void onNativeUpdate() {
        if (application == null && running == null) return;
        try {
            apply(new JSONObject(NativeSession.refreshSchedule()));
        } catch (JSONException failure) {
            if (running != null) running.finish(true);
        }
    }

    private static void apply(JSONObject state) {
        long deadline = state.optLong(DEADLINE, 0);
        if (running == null) {
            if (state.optBoolean("ready")) schedule(deadline, false);
            return;
        }
        if (running.request == 0 || state.optBoolean("failed")) {
            running.finish(true);
        } else if (state.optLong("completed", 0) >= running.request) {
            running.finish(false);
            if (deadline > 0) schedule(deadline, true);
        }
    }

    private void finish(boolean reschedule) {
        JobParameters completed = parameters;
        release();
        jobFinished(completed, reschedule);
    }

    private void release() {
        if (request != 0) NativeSession.cancelPeriodicRefresh(request);
        request = 0;
        parameters = null;
        if (running == this) running = null;
    }

    private static void schedule(long deadline, boolean afterRun) {
        JobScheduler scheduler = application.getSystemService(JobScheduler.class);
        if (scheduler == null) return;
        JobInfo pending = scheduler.getPendingJob(JOB_ID);
        if (deadline == 0) {
            if (pending != null) scheduler.cancel(JOB_ID);
            return;
        }
        if (!afterRun && pending != null && pending.getExtras().getLong(DEADLINE) == deadline) return;
        long delay = Math.max(0, deadline - System.currentTimeMillis() / 1000) * 1000;
        if (afterRun && delay == 0) delay = DEFERRED_RETRY_MILLIS;
        PersistableBundle extras = new PersistableBundle();
        extras.putLong(DEADLINE, deadline);
        JobInfo job = new JobInfo.Builder(JOB_ID, new ComponentName(application, SubscriptionRefreshJob.class))
            .setExtras(extras)
            .setMinimumLatency(delay)
            .setRequiredNetworkType(JobInfo.NETWORK_TYPE_ANY)
            .setPersisted(true)
            .build();
        if (scheduler.schedule(job) != JobScheduler.RESULT_SUCCESS) {
            Log.w("VpnClient", "Subscription refresh job was rejected");
        }
    }
}

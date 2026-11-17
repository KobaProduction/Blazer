package com.kobaproduction.blazer;

import android.app.Activity;
import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.net.Uri;
import android.os.Build;

enum UpdateChannel {
    DEVELOPMENT, DIRECT, GOOGLE_PLAY;

    private static final String DISTRIBUTION = "com.kobaproduction.blazer.update_channel";
    private static final String PLAY_PACKAGE = "com.android.vending";
    private static final String RELEASES = "https://github.com/KobaProduction/Blazer/releases/latest";

    static UpdateChannel installed(Activity activity) {
        PackageManager packages = activity.getPackageManager();
        String packageName = activity.getPackageName();
        try {
            var info = packages.getApplicationInfo(packageName, PackageManager.GET_META_DATA);
            String distribution = info.metaData == null ? null : info.metaData.getString(DISTRIBUTION);
            String installer = Build.VERSION.SDK_INT >= 30
                ? packages.getInstallSourceInfo(packageName).getInstallingPackageName()
                : packages.getInstallerPackageName(packageName);
            return resolve(distribution, installer);
        } catch (PackageManager.NameNotFoundException | IllegalArgumentException ignored) {
            return DEVELOPMENT;
        }
    }

    static UpdateChannel resolve(String distribution, String installer) {
        if ("google-play".equals(distribution) || PLAY_PACKAGE.equals(installer)) return GOOGLE_PLAY;
        if ("direct".equals(distribution)) return DIRECT;
        return DEVELOPMENT;
    }

    boolean open(Activity activity) {
        if (this == DIRECT) return launch(activity, RELEASES, null);
        if (this != GOOGLE_PLAY) return false;
        String id = Uri.encode(activity.getPackageName());
        return launch(activity, "market://details?id=" + id, PLAY_PACKAGE)
            || launch(activity, "https://play.google.com/store/apps/details?id=" + id, null);
    }

    private static boolean launch(Activity activity, String address, String packageName) {
        Intent intent = new Intent(Intent.ACTION_VIEW, Uri.parse(address));
        if (packageName != null) intent.setPackage(packageName);
        try {
            activity.startActivity(intent);
            return true;
        } catch (ActivityNotFoundException | SecurityException ignored) {
            return false;
        }
    }
}

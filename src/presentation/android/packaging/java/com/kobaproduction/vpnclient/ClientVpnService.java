package com.kobaproduction.vpnclient;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.net.ConnectivityManager;
import android.net.LinkProperties;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.NetworkRequest;
import android.net.VpnService;
import android.os.ParcelFileDescriptor;
import com.shoesproxy.ShoesNative;
import java.io.IOException;

public final class ClientVpnService extends VpnService {
    private static final String CHANNEL = "vpn";
    private static final String TUN_ADDRESS = "10.233.0.1";
    private static volatile ClientVpnService active;
    private volatile boolean revoked;
    private ParcelFileDescriptor descriptor;
    private long engineHandle;
    private long startRequest;
    private RouteObserver routeObserver;

    static void request(Context context) {
        context.startForegroundService(new Intent(context, ClientVpnService.class));
    }

    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        startForeground(1, notification());
        if (active != null) return START_NOT_STICKY;
        active = this;
        startRequest = NativeSession.requestTunnelStart();
        if (startRequest == 0) abandonStart();
        return START_NOT_STICKY;
    }

    static void onNativeUpdate() {
        ClientVpnService service = active;
        if (service == null || service.startRequest == 0) return;
        int outcome = NativeSession.tunnelStartOutcome(service.startRequest);
        if (outcome == 0) return;
        service.startRequest = 0;
        if (outcome < 0) service.abandonStart();
    }

    private void abandonStart() {
        if (active != this) return;
        active = null;
        stopSelf();
    }

    @Override public void onRevoke() {
        disconnect();
    }

    @Override public void onDestroy() {
        if (active == this) disconnect();
        super.onDestroy();
    }

    private void disconnect() {
        revoked = true;
        NativeSession.dispatch("{\"type\":\"disconnect\"}");
    }

    static int openTunnel() {
        ClientVpnService service = active;
        return service == null ? -1 : service.establishTunnel();
    }

    private int establishTunnel() {
        if (revoked || descriptor != null) return -1;
        try {
            RouteObserver observer = new RouteObserver();
            NetworkRequest vpn = new NetworkRequest.Builder()
                .addTransportType(NetworkCapabilities.TRANSPORT_VPN)
                .removeCapability(NetworkCapabilities.NET_CAPABILITY_NOT_VPN)
                .build();
            getSystemService(ConnectivityManager.class).registerNetworkCallback(vpn, observer);
            routeObserver = observer;
            String sessionName = getApplicationInfo().loadLabel(getPackageManager()).toString();
            descriptor = new Builder().setSession(sessionName).setMtu(1500)
                .addAddress(TUN_ADDRESS, 30).addAddress("fd73:686f:6573::1", 126)
                .addRoute("0.0.0.0", 0).addRoute("::", 0)
                .addDnsServer("10.233.0.2").establish();
            return descriptor == null ? -1 : descriptor.getFd();
        } catch (IllegalArgumentException | IllegalStateException | SecurityException failure) {
            return -1;
        }
    }

    static boolean startTunnel(String config) {
        ClientVpnService service = active;
        if (service == null || service.revoked) return false;
        service.engineHandle = ShoesNative.start(config, service);
        return service.engineHandle > 0;
    }

    static boolean tunnelRunning() {
        ClientVpnService service = active;
        return service != null && !service.revoked && service.engineHandle > 0 && ShoesNative.isRunning();
    }

    static boolean tunnelRouted() {
        ClientVpnService service = active;
        return service != null && !service.revoked && service.routeObserver != null
            && service.routeObserver.ready();
    }

    static boolean stopTunnel() {
        ClientVpnService service = active;
        if (service == null) return true;
        if (service.engineHandle > 0) {
            ShoesNative.stop(service.engineHandle);
            if (ShoesNative.isRunning()) return false;
            service.engineHandle = 0;
        }
        active = null;
        service.closeTunnel();
        service.stopSelf();
        return true;
    }

    private void closeTunnel() {
        if (routeObserver != null) {
            getSystemService(ConnectivityManager.class).unregisterNetworkCallback(routeObserver);
            routeObserver = null;
        }
        if (descriptor == null) return;
        try {
            descriptor.close();
        } catch (IOException failure) {
            // ParcelFileDescriptor closes its handle even if the close reports an error.
        } finally {
            descriptor = null;
        }
    }

    private static final class RouteObserver extends ConnectivityManager.NetworkCallback {
        private volatile Network tunnel;

        @Override public void onLinkPropertiesChanged(Network network, LinkProperties links) {
            boolean assigned = links.getLinkAddresses().stream()
                .anyMatch(link -> TUN_ADDRESS.equals(link.getAddress().getHostAddress()));
            boolean defaultRoute = links.getRoutes().stream().anyMatch(route -> route.isDefaultRoute());
            if (assigned && defaultRoute) {
                tunnel = network;
            } else {
                onLost(network);
            }
        }

        @Override public void onLost(Network network) {
            if (network.equals(tunnel)) tunnel = null;
        }

        boolean ready() { return tunnel != null; }
    }

    private Notification notification() {
        NotificationManager manager = getSystemService(NotificationManager.class);
        CharSequence title = getApplicationInfo().loadLabel(getPackageManager());
        manager.createNotificationChannel(new NotificationChannel(CHANNEL, title, NotificationManager.IMPORTANCE_LOW));
        PendingIntent open = PendingIntent.getActivity(this, 0, new Intent(this, MainActivity.class),
            PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT);
        return new Notification.Builder(this, CHANNEL).setContentTitle(title)
            .setSmallIcon(android.R.drawable.ic_lock_lock).setContentIntent(open).setOngoing(true).build();
    }
}
